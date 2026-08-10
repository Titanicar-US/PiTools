use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    db::Database,
    github::events::{CheckRecord, DeliveryEnvelope, EventParseError, FeedbackRecord},
    models::{PullRequestSnapshot, PullRequestState, ReadinessSnapshot},
    policy::Policy,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Inserted,
    Duplicate,
    Conflict,
}

#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("policy serialization error: {0}")]
    PolicySerialization(String),
    #[error("event parsing error: {0}")]
    Event(#[from] EventParseError),
}

#[derive(Clone)]
pub struct Repositories {
    database: Database,
}

impl Repositories {
    pub fn new(database: Database) -> Self {
        Self { database }
    }

    pub async fn record_delivery(
        &self,
        delivery: &DeliveryEnvelope,
    ) -> Result<DeliveryOutcome, RepositoryError> {
        let payload_hash = format!("sha256:{}", hex::encode(Sha256::digest(&delivery.raw_body)));
        let result = sqlx::query(
            "INSERT INTO event_deliveries
                (delivery_id, event_name, action, installation_id, repository_id,
                 pull_request_number, payload_hash, payload, raw_payload,
                 raw_payload_expires_at, supported)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW() + INTERVAL '7 days', $10)
             ON CONFLICT (delivery_id) DO NOTHING",
        )
        .bind(&delivery.delivery_id)
        .bind(&delivery.event_name)
        .bind(&delivery.action)
        .bind(delivery.installation_id)
        .bind(delivery.repository_id)
        .bind(delivery.pull_request_number)
        .bind(&payload_hash)
        .bind(&delivery.payload)
        .bind(&delivery.raw_body)
        .bind(delivery.is_supported())
        .execute(self.database.pool())
        .await?;
        if result.rows_affected() == 1 {
            return Ok(DeliveryOutcome::Inserted);
        }
        let existing_hash: Option<String> =
            sqlx::query_scalar("SELECT payload_hash FROM event_deliveries WHERE delivery_id = $1")
                .bind(&delivery.delivery_id)
                .fetch_optional(self.database.pool())
                .await?;
        Ok(match existing_hash.as_deref() {
            Some(hash) if hash == payload_hash => DeliveryOutcome::Duplicate,
            Some(_) => DeliveryOutcome::Conflict,
            None => DeliveryOutcome::Conflict,
        })
    }

    pub async fn process_delivery(
        &self,
        delivery: &DeliveryEnvelope,
    ) -> Result<DeliveryOutcome, RepositoryError> {
        let outcome = self.record_delivery(delivery).await?;
        if outcome == DeliveryOutcome::Conflict {
            return Ok(outcome);
        }

        // The delivery insert is intentionally separate so conflicting
        // delivery IDs can be rejected without taking a long-lived lock. A
        // transaction-scoped advisory lock closes the duplicate-processing
        // race between retries while apply_delivery uses idempotent upserts.
        let mut transaction = self.database.pool().begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(&delivery.delivery_id)
            .execute(&mut *transaction)
            .await?;
        let processed_at: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT processed_at FROM event_deliveries WHERE delivery_id = $1")
                .bind(&delivery.delivery_id)
                .fetch_one(&mut *transaction)
                .await?;
        if processed_at.is_some() {
            transaction.commit().await?;
            return Ok(outcome);
        }

        let result = self.apply_delivery(delivery, transaction.as_mut()).await;
        match result {
            Ok(()) => {
                sqlx::query(
                    "UPDATE event_deliveries
                     SET processed_at = NOW(), processing_error = NULL
                     WHERE delivery_id = $1",
                )
                .bind(&delivery.delivery_id)
                .execute(&mut *transaction)
                .await?;
                transaction.commit().await?;
                Ok(outcome)
            }
            Err(error) => {
                transaction.rollback().await?;
                let message = error.to_string();
                let _ = self
                    .mark_delivery_processed(&delivery.delivery_id, Some(&message))
                    .await;
                Err(error)
            }
        }
    }

    async fn apply_delivery(
        &self,
        delivery: &DeliveryEnvelope,
        connection: &mut sqlx::PgConnection,
    ) -> Result<(), RepositoryError> {
        self.upsert_installation_from_payload(delivery, connection)
            .await?;
        self.upsert_repository_from_payload(delivery, connection)
            .await?;
        if let Some(snapshot) = delivery.pull_request_snapshot()? {
            self.upsert_pull_request_on(connection, &snapshot).await?;
        }
        if let Some(feedback) = delivery.feedback_record()?
            && let (Some(repository_id), Some(pull_request_number)) =
                (delivery.repository_id, delivery.pull_request_number)
        {
            self.upsert_feedback_on(connection, repository_id, pull_request_number, &feedback)
                .await?;
        }
        if let Some(check) = delivery.check_record()?
            && let (Some(repository_id), Some(pull_request_number)) =
                (delivery.repository_id, delivery.pull_request_number)
        {
            self.upsert_check_on(connection, repository_id, pull_request_number, &check)
                .await?;
        }
        Ok(())
    }

    async fn upsert_feedback(
        &self,
        repository_id: i64,
        pull_request_number: i32,
        feedback: &FeedbackRecord,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO feedback_items
             (id, repository_id, pull_request_number, actor_login, actor_type, body, resolved, is_automation)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (id) DO UPDATE SET actor_login = EXCLUDED.actor_login,
               actor_type = EXCLUDED.actor_type, body = EXCLUDED.body,
               resolved = EXCLUDED.resolved
                 OR (feedback_items.repair_state = 'applied' AND feedback_items.resolution_eligible),
               is_automation = EXCLUDED.is_automation,
               updated_at = NOW()",
        )
        .bind(&feedback.id)
        .bind(repository_id)
        .bind(pull_request_number)
        .bind(&feedback.actor_login)
        .bind(&feedback.actor_type)
        .bind(&feedback.body)
        .bind(feedback.resolved)
        .bind(feedback.is_automation)
        .execute(self.database.pool())
        .await?;
        Ok(())
    }

    async fn upsert_feedback_on(
        &self,
        connection: &mut sqlx::PgConnection,
        repository_id: i64,
        pull_request_number: i32,
        feedback: &FeedbackRecord,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO feedback_items
             (id, repository_id, pull_request_number, actor_login, actor_type, body, resolved, is_automation)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (id) DO UPDATE SET actor_login = EXCLUDED.actor_login,
               actor_type = EXCLUDED.actor_type, body = EXCLUDED.body,
               resolved = EXCLUDED.resolved
                 OR (feedback_items.repair_state = 'applied' AND feedback_items.resolution_eligible),
               is_automation = EXCLUDED.is_automation,
               updated_at = NOW()",
        )
        .bind(&feedback.id)
        .bind(repository_id)
        .bind(pull_request_number)
        .bind(&feedback.actor_login)
        .bind(&feedback.actor_type)
        .bind(&feedback.body)
        .bind(feedback.resolved)
        .bind(feedback.is_automation)
        .execute(&mut *connection)
        .await?;
        Ok(())
    }

    pub async fn mark_feedback_resolved(&self, feedback_id: &str) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE feedback_items
             SET resolved = TRUE,
                 repair_state = 'applied',
                 repair_applied_at = COALESCE(repair_applied_at, NOW()),
                 resolution_eligible = TRUE,
                 updated_at = NOW()
             WHERE id = $1",
        )
        .bind(feedback_id)
        .execute(self.database.pool())
        .await?;
        Ok(())
    }

    pub async fn feedback_resolution_eligible(
        &self,
        feedback_id: &str,
    ) -> Result<bool, RepositoryError> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT resolution_eligible FROM feedback_items WHERE id = $1",
        )
        .bind(feedback_id)
        .fetch_optional(self.database.pool())
        .await?
        .unwrap_or(false))
    }

    async fn upsert_check(
        &self,
        repository_id: i64,
        pull_request_number: i32,
        check: &CheckRecord,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO checks
             (repository_id, pull_request_number, external_id, name, status, conclusion, details_url)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (repository_id, pull_request_number, external_id) DO UPDATE SET
               name = EXCLUDED.name, status = EXCLUDED.status,
               conclusion = EXCLUDED.conclusion, details_url = EXCLUDED.details_url,
               updated_at = NOW()",
        )
        .bind(repository_id)
        .bind(pull_request_number)
        .bind(&check.external_id)
        .bind(&check.name)
        .bind(&check.status)
        .bind(&check.conclusion)
        .bind(&check.details_url)
        .execute(self.database.pool())
        .await?;
        Ok(())
    }

    async fn upsert_check_on(
        &self,
        connection: &mut sqlx::PgConnection,
        repository_id: i64,
        pull_request_number: i32,
        check: &CheckRecord,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO checks
             (repository_id, pull_request_number, external_id, name, status, conclusion, details_url)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (repository_id, pull_request_number, external_id) DO UPDATE SET
               name = EXCLUDED.name, status = EXCLUDED.status,
               conclusion = EXCLUDED.conclusion, details_url = EXCLUDED.details_url,
               updated_at = NOW()",
        )
        .bind(repository_id)
        .bind(pull_request_number)
        .bind(&check.external_id)
        .bind(&check.name)
        .bind(&check.status)
        .bind(&check.conclusion)
        .bind(&check.details_url)
        .execute(&mut *connection)
        .await?;
        Ok(())
    }

    async fn upsert_installation_from_payload(
        &self,
        delivery: &DeliveryEnvelope,
        connection: &mut sqlx::PgConnection,
    ) -> Result<(), RepositoryError> {
        let Some(installation_id) = delivery.installation_id else {
            return Ok(());
        };
        let account_login = delivery
            .payload
            .pointer("/installation/account/login")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                delivery
                    .payload
                    .pointer("/organization/login")
                    .and_then(serde_json::Value::as_str)
            })
            .unwrap_or("unknown");
        let account_type = delivery
            .payload
            .pointer("/installation/account/type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Unknown");
        sqlx::query(
            "INSERT INTO installations (id, account_login, account_type)
             VALUES ($1, $2, $3)
             ON CONFLICT (id) DO UPDATE SET account_login = EXCLUDED.account_login,
             account_type = EXCLUDED.account_type, updated_at = NOW()",
        )
        .bind(installation_id)
        .bind(account_login)
        .bind(account_type)
        .execute(&mut *connection)
        .await?;
        Ok(())
    }

    async fn upsert_repository_from_payload(
        &self,
        delivery: &DeliveryEnvelope,
        connection: &mut sqlx::PgConnection,
    ) -> Result<(), RepositoryError> {
        let Some(installation_id) = delivery.installation_id else {
            return Ok(());
        };
        let mut repositories = Vec::new();
        if let Some(repository) = delivery.payload.get("repository") {
            repositories.push(repository);
        }
        for key in ["repositories_added", "repositories"] {
            if let Some(values) = delivery
                .payload
                .get(key)
                .and_then(serde_json::Value::as_array)
            {
                repositories.extend(values);
            }
        }
        for repository in repositories {
            let Some(repository_id) = repository.get("id").and_then(serde_json::Value::as_i64)
            else {
                continue;
            };
            let owner = repository
                .pointer("/owner/login")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let name = repository
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let default_branch = repository
                .get("default_branch")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("main");
            sqlx::query(
                "INSERT INTO repositories (id, installation_id, owner, name, default_branch)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (id) DO UPDATE SET owner = EXCLUDED.owner,
                 name = EXCLUDED.name, default_branch = EXCLUDED.default_branch,
                 updated_at = NOW()",
            )
            .bind(repository_id)
            .bind(installation_id)
            .bind(owner)
            .bind(name)
            .bind(default_branch)
            .execute(&mut *connection)
            .await?;
        }
        Ok(())
    }

    pub async fn upsert_pull_request(
        &self,
        snapshot: &PullRequestSnapshot,
    ) -> Result<(), RepositoryError> {
        let state = match &snapshot.state {
            PullRequestState::Open => "open",
            PullRequestState::Closed => "closed",
            PullRequestState::Merged => "merged",
        };
        sqlx::query(
            "INSERT INTO pull_requests
                (repository_id, number, github_id, title, url, state, draft, merged,
                 head_sha, base_sha, head_branch, base_branch, author_login, watched, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
             ON CONFLICT (repository_id, number) DO UPDATE SET
                 github_id = EXCLUDED.github_id,
                 title = EXCLUDED.title,
                 url = EXCLUDED.url,
                 state = EXCLUDED.state,
                 draft = EXCLUDED.draft,
                 merged = EXCLUDED.merged,
                 head_sha = EXCLUDED.head_sha,
                 base_sha = EXCLUDED.base_sha,
                 head_branch = EXCLUDED.head_branch,
                 base_branch = EXCLUDED.base_branch,
                 author_login = EXCLUDED.author_login,
                 watched = EXCLUDED.watched,
                 updated_at = EXCLUDED.updated_at
             WHERE pull_requests.updated_at <= EXCLUDED.updated_at",
        )
        .bind(snapshot.repository_id)
        .bind(snapshot.number)
        .bind(snapshot.github_id)
        .bind(&snapshot.title)
        .bind(&snapshot.url)
        .bind(state)
        .bind(snapshot.draft)
        .bind(snapshot.merged)
        .bind(&snapshot.head_sha)
        .bind(&snapshot.base_sha)
        .bind(&snapshot.head_branch)
        .bind(&snapshot.base_branch)
        .bind(&snapshot.author_login)
        .bind(snapshot.state == PullRequestState::Open)
        .bind(snapshot.updated_at)
        .execute(self.database.pool())
        .await?;
        Ok(())
    }

    async fn upsert_pull_request_on(
        &self,
        connection: &mut sqlx::PgConnection,
        snapshot: &PullRequestSnapshot,
    ) -> Result<(), RepositoryError> {
        let state = match &snapshot.state {
            PullRequestState::Open => "open",
            PullRequestState::Closed => "closed",
            PullRequestState::Merged => "merged",
        };
        sqlx::query(
            "INSERT INTO pull_requests
                (repository_id, number, github_id, title, url, state, draft, merged,
                 head_sha, base_sha, head_branch, base_branch, author_login, watched, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
             ON CONFLICT (repository_id, number) DO UPDATE SET
                 github_id = EXCLUDED.github_id,
                 title = EXCLUDED.title,
                 url = EXCLUDED.url,
                 state = EXCLUDED.state,
                 draft = EXCLUDED.draft,
                 merged = EXCLUDED.merged,
                 head_sha = EXCLUDED.head_sha,
                 base_sha = EXCLUDED.base_sha,
                 head_branch = EXCLUDED.head_branch,
                 base_branch = EXCLUDED.base_branch,
                 author_login = EXCLUDED.author_login,
                 watched = EXCLUDED.watched,
                 updated_at = EXCLUDED.updated_at
             WHERE pull_requests.updated_at <= EXCLUDED.updated_at",
        )
        .bind(snapshot.repository_id)
        .bind(snapshot.number)
        .bind(snapshot.github_id)
        .bind(&snapshot.title)
        .bind(&snapshot.url)
        .bind(state)
        .bind(snapshot.draft)
        .bind(snapshot.merged)
        .bind(&snapshot.head_sha)
        .bind(&snapshot.base_sha)
        .bind(&snapshot.head_branch)
        .bind(&snapshot.base_branch)
        .bind(&snapshot.author_login)
        .bind(snapshot.state == PullRequestState::Open)
        .bind(snapshot.updated_at)
        .execute(&mut *connection)
        .await?;
        Ok(())
    }

    pub async fn save_readiness(
        &self,
        repository_id: i64,
        pull_request_number: i32,
        snapshot: &ReadinessSnapshot,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO readiness_snapshots (id, repository_id, pull_request_number, ready, reason_codes, evaluated_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(snapshot.snapshot_id)
        .bind(repository_id)
        .bind(pull_request_number)
        .bind(snapshot.ready)
        .bind(serde_json::to_value(&snapshot.reason_codes)?)
        .bind(snapshot.evaluated_at)
        .execute(self.database.pool())
        .await?;
        sqlx::query(
            "UPDATE pull_requests SET readiness = $1, updated_at = NOW()
             WHERE repository_id = $2 AND number = $3",
        )
        .bind(serde_json::to_value(snapshot)?)
        .bind(repository_id)
        .bind(pull_request_number)
        .execute(self.database.pool())
        .await?;
        Ok(())
    }

    pub async fn save_policy(
        &self,
        repository_id: i64,
        policy: &Policy,
    ) -> Result<(), RepositoryError> {
        let yaml = serde_yml::to_string(policy)
            .map_err(|error| RepositoryError::PolicySerialization(error.to_string()))?;
        sqlx::query(
            "UPDATE repositories SET policy_yaml = $1, policy_revision = $2, updated_at = NOW()
             WHERE id = $3",
        )
        .bind(yaml)
        .bind(policy.revision())
        .bind(repository_id)
        .execute(self.database.pool())
        .await?;
        Ok(())
    }

    pub async fn save_reconciliation(
        &self,
        snapshot: &PullRequestSnapshot,
        readiness: &ReadinessSnapshot,
        feedback: &[FeedbackRecord],
        checks: &[CheckRecord],
    ) -> Result<(), RepositoryError> {
        self.upsert_pull_request(snapshot).await?;
        for item in feedback {
            self.upsert_feedback(snapshot.repository_id, snapshot.number, item)
                .await?;
        }
        for check in checks {
            self.upsert_check(snapshot.repository_id, snapshot.number, check)
                .await?;
        }
        self.save_readiness(snapshot.repository_id, snapshot.number, readiness)
            .await
    }

    pub async fn upsert_living_comment(
        &self,
        job_id: Uuid,
        github_comment_id: i64,
        marker: &str,
        body_hash: &str,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO comments (id, job_id, github_comment_id, kind, marker, body_hash, immutable)
             VALUES ($1, $2, $3, 'work_plan', $4, $5, FALSE)
             ON CONFLICT (job_id, kind) DO UPDATE SET
               github_comment_id = EXCLUDED.github_comment_id,
               marker = EXCLUDED.marker,
               body_hash = EXCLUDED.body_hash,
               immutable = FALSE,
               updated_at = NOW()",
        )
        .bind(Uuid::now_v7())
        .bind(job_id)
        .bind(github_comment_id)
        .bind(marker)
        .bind(body_hash)
        .execute(self.database.pool())
        .await?;
        Ok(())
    }

    pub async fn link_job_check_run(
        &self,
        job_id: Uuid,
        check_run_id: i64,
        check_run_url: &str,
        current_item: Option<&str>,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE jobs SET check_run_id = $1, check_run_url = $2,
             current_item = $3, updated_at = NOW() WHERE id = $4",
        )
        .bind(check_run_id)
        .bind(check_run_url)
        .bind(current_item)
        .bind(job_id)
        .execute(self.database.pool())
        .await?;
        Ok(())
    }

    pub async fn existing_work_handle(
        &self,
        job_id: Uuid,
    ) -> Result<Option<ExistingWorkHandle>, RepositoryError> {
        let row = sqlx::query(
            "SELECT jobs.check_run_id, jobs.check_run_url, comments.github_comment_id
             FROM jobs
             JOIN comments ON comments.job_id = jobs.id AND comments.kind = 'work_plan'
             WHERE jobs.id = $1
               AND jobs.check_run_id IS NOT NULL
               AND jobs.check_run_url IS NOT NULL
               AND comments.github_comment_id IS NOT NULL
             LIMIT 1",
        )
        .bind(job_id)
        .fetch_optional(self.database.pool())
        .await?;
        Ok(row.map(|row| ExistingWorkHandle {
            check_run_id: row.get("check_run_id"),
            check_run_url: row.get("check_run_url"),
            comment_id: row.get("github_comment_id"),
        }))
    }

    pub async fn insert_final_comment(
        &self,
        job_id: Uuid,
        github_comment_id: i64,
        marker: &str,
        body_hash: &str,
    ) -> Result<bool, RepositoryError> {
        let result = sqlx::query(
            "INSERT INTO comments (id, job_id, github_comment_id, kind, marker, body_hash, immutable)
             VALUES ($1, $2, $3, 'final_summary', $4, $5, TRUE)
             ON CONFLICT (job_id, kind) DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(job_id)
        .bind(github_comment_id)
        .bind(marker)
        .bind(body_hash)
        .execute(self.database.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn record_job_control(
        &self,
        job_id: Uuid,
        request_id: &str,
        action: &str,
        actor_login: &str,
        target_item_id: Option<&str>,
        decision: &str,
    ) -> Result<bool, RepositoryError> {
        let result = sqlx::query(
            "INSERT INTO job_controls (id, job_id, request_id, action, actor_login, target_item_id, decision)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (job_id, request_id) DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(job_id)
        .bind(request_id)
        .bind(action)
        .bind(actor_login)
        .bind(target_item_id)
        .bind(decision)
        .execute(self.database.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn record_audit(&self, entry: AuditEntry) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO audit_entries
             (id, repository_id, pull_request_number, job_id, actor_login, event_type, summary, evidence)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(Uuid::now_v7())
        .bind(entry.repository_id)
        .bind(entry.pull_request_number)
        .bind(entry.job_id)
        .bind(entry.actor_login)
        .bind(entry.event_type)
        .bind(entry.summary)
        .bind(entry.evidence)
        .execute(self.database.pool())
        .await?;
        Ok(())
    }

    pub async fn control_context(
        &self,
        check_run_id: i64,
    ) -> Result<Option<ControlContext>, RepositoryError> {
        let row = sqlx::query(
            "SELECT jobs.id, jobs.repository_id, jobs.pull_request_number,
                    pull_requests.author_login, repositories.policy_yaml
             FROM jobs
             JOIN pull_requests ON pull_requests.repository_id = jobs.repository_id
                AND pull_requests.number = jobs.pull_request_number
             JOIN repositories ON repositories.id = jobs.repository_id
             WHERE jobs.check_run_id = $1
             ORDER BY jobs.created_at DESC
             LIMIT 1",
        )
        .bind(check_run_id)
        .fetch_optional(self.database.pool())
        .await?;
        Ok(row.map(|row| ControlContext {
            job_id: row.get("id"),
            repository_id: row.get("repository_id"),
            pull_request_number: row.get("pull_request_number"),
            pr_author: row.get("author_login"),
            policy_yaml: row.get("policy_yaml"),
        }))
    }

    pub async fn mark_delivery_processed(
        &self,
        delivery_id: &str,
        error: Option<&str>,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE event_deliveries
             SET processed_at = CASE WHEN $2::TEXT IS NULL THEN $1 ELSE NULL END,
                 processing_error = $2
             WHERE delivery_id = $3",
        )
        .bind(Utc::now())
        .bind(error)
        .bind(delivery_id)
        .execute(self.database.pool())
        .await?;
        Ok(())
    }

    pub async fn purge_expired_raw_payloads(&self) -> Result<u64, RepositoryError> {
        let result = sqlx::query(
            "UPDATE event_deliveries SET raw_payload = NULL
             WHERE raw_payload IS NOT NULL AND raw_payload_expires_at < NOW()",
        )
        .execute(self.database.pool())
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn ping(&self) -> Result<(), RepositoryError> {
        self.database
            .ping()
            .await
            .map_err(RepositoryError::Database)
    }

    pub async fn open_pull_requests(&self) -> Result<Vec<PullRequestRow>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT repository_id, number, title, url, state, updated_at
             FROM pull_requests WHERE watched = TRUE ORDER BY updated_at DESC",
        )
        .fetch_all(self.database.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| PullRequestRow {
                repository_id: row.get("repository_id"),
                number: row.get("number"),
                title: row.get("title"),
                url: row.get("url"),
                state: row.get("state"),
                updated_at: row.get("updated_at"),
            })
            .collect())
    }

    pub async fn pull_request_detail(
        &self,
        repository_id: i64,
        pull_request_number: i32,
    ) -> Result<Option<PullRequestDetail>, RepositoryError> {
        let row = sqlx::query(
            "SELECT repository_id, number, github_id, title, url, state, draft, merged,
                    head_sha, base_sha, head_branch, base_branch, author_login, readiness
             FROM pull_requests WHERE repository_id = $1 AND number = $2",
        )
        .bind(repository_id)
        .bind(pull_request_number)
        .fetch_optional(self.database.pool())
        .await?;
        Ok(row.map(|row| PullRequestDetail {
            repository_id: row.get("repository_id"),
            number: row.get("number"),
            github_id: row.get("github_id"),
            title: row.get("title"),
            url: row.get("url"),
            state: row.get("state"),
            draft: row.get("draft"),
            merged: row.get("merged"),
            head_sha: row.get("head_sha"),
            base_sha: row.get("base_sha"),
            head_branch: row.get("head_branch"),
            base_branch: row.get("base_branch"),
            author_login: row.get("author_login"),
            readiness: row.get("readiness"),
        }))
    }

    pub async fn recent_events(
        &self,
        limit: i64,
    ) -> Result<Vec<EventDeliveryRow>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT delivery_id, event_name, action, installation_id, repository_id,
                    pull_request_number, payload_hash, supported, received_at, processed_at,
                    processing_error
             FROM event_deliveries
             ORDER BY received_at DESC, delivery_id DESC
             LIMIT $1",
        )
        .bind(limit.clamp(1, 100))
        .fetch_all(self.database.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| EventDeliveryRow {
                delivery_id: row.get("delivery_id"),
                event_name: row.get("event_name"),
                action: row.get("action"),
                installation_id: row.get("installation_id"),
                repository_id: row.get("repository_id"),
                pull_request_number: row.get("pull_request_number"),
                payload_hash: row.get("payload_hash"),
                supported: row.get("supported"),
                received_at: row.get("received_at"),
                processed_at: row.get("processed_at"),
                processing_error: row.get("processing_error"),
            })
            .collect())
    }

    pub async fn recent_audit_entries(&self, limit: i64) -> Result<Vec<AuditRow>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, repository_id, pull_request_number, job_id, actor_login,
                    event_type, summary, evidence, created_at
             FROM audit_entries
             ORDER BY created_at DESC, id DESC
             LIMIT $1",
        )
        .bind(limit.clamp(1, 100))
        .fetch_all(self.database.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| AuditRow {
                id: row.get("id"),
                repository_id: row.get("repository_id"),
                pull_request_number: row.get("pull_request_number"),
                job_id: row.get("job_id"),
                actor_login: row.get("actor_login"),
                event_type: row.get("event_type"),
                summary: row.get("summary"),
                evidence: row.get("evidence"),
                created_at: row.get("created_at"),
            })
            .collect())
    }

    pub async fn open_pull_requests_for_head(
        &self,
        repository_id: i64,
        head_sha: &str,
    ) -> Result<Vec<i32>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT number FROM pull_requests
             WHERE repository_id = $1 AND head_sha = $2 AND watched = TRUE
             ORDER BY number",
        )
        .bind(repository_id)
        .bind(head_sha)
        .fetch_all(self.database.pool())
        .await?;
        Ok(rows.into_iter().map(|row| row.get("number")).collect())
    }

    pub async fn repository_context(
        &self,
        repository_id: i64,
    ) -> Result<Option<RepositoryContext>, RepositoryError> {
        let row = sqlx::query(
            "SELECT id, installation_id, owner, name, default_branch, policy_yaml
             FROM repositories WHERE id = $1 AND active = TRUE",
        )
        .bind(repository_id)
        .fetch_optional(self.database.pool())
        .await?;
        Ok(row.map(|row| RepositoryContext {
            id: row.get("id"),
            installation_id: row.get("installation_id"),
            owner: row.get("owner"),
            name: row.get("name"),
            default_branch: row.get("default_branch"),
            policy_yaml: row.get("policy_yaml"),
        }))
    }

    pub async fn active_repository_contexts(
        &self,
    ) -> Result<Vec<RepositoryContext>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, installation_id, owner, name, default_branch, policy_yaml
             FROM repositories WHERE active = TRUE ORDER BY id",
        )
        .fetch_all(self.database.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| RepositoryContext {
                id: row.get("id"),
                installation_id: row.get("installation_id"),
                owner: row.get("owner"),
                name: row.get("name"),
                default_branch: row.get("default_branch"),
                policy_yaml: row.get("policy_yaml"),
            })
            .collect())
    }

    pub async fn pull_request_context(
        &self,
        repository_id: i64,
        pull_request_number: i32,
    ) -> Result<Option<PullRequestContext>, RepositoryError> {
        let row = sqlx::query(
            "SELECT repository_id, number, github_id, title, url, state, draft, merged,
                    head_sha, base_sha, head_branch, base_branch, author_login
             FROM pull_requests WHERE repository_id = $1 AND number = $2",
        )
        .bind(repository_id)
        .bind(pull_request_number)
        .fetch_optional(self.database.pool())
        .await?;
        Ok(row.map(|row| PullRequestContext {
            repository_id: row.get("repository_id"),
            number: row.get("number"),
            github_id: row.get("github_id"),
            title: row.get("title"),
            url: row.get("url"),
            state: row.get("state"),
            draft: row.get("draft"),
            merged: row.get("merged"),
            head_sha: row.get("head_sha"),
            base_sha: row.get("base_sha"),
            head_branch: row.get("head_branch"),
            base_branch: row.get("base_branch"),
            author_login: row.get("author_login"),
        }))
    }

    pub async fn open_pull_request_contexts(
        &self,
        repository_id: i64,
    ) -> Result<Vec<PullRequestContext>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT repository_id, number, github_id, title, url, state, draft, merged,
                    head_sha, base_sha, head_branch, base_branch, author_login
             FROM pull_requests
             WHERE repository_id = $1 AND watched = TRUE AND state = 'open'
             ORDER BY number",
        )
        .bind(repository_id)
        .fetch_all(self.database.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| PullRequestContext {
                repository_id: row.get("repository_id"),
                number: row.get("number"),
                github_id: row.get("github_id"),
                title: row.get("title"),
                url: row.get("url"),
                state: row.get("state"),
                draft: row.get("draft"),
                merged: row.get("merged"),
                head_sha: row.get("head_sha"),
                base_sha: row.get("base_sha"),
                head_branch: row.get("head_branch"),
                base_branch: row.get("base_branch"),
                author_login: row.get("author_login"),
            })
            .collect())
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PullRequestRow {
    pub repository_id: i64,
    pub number: i32,
    pub title: String,
    pub url: String,
    pub state: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PullRequestDetail {
    pub repository_id: i64,
    pub number: i32,
    pub github_id: i64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub draft: bool,
    pub merged: bool,
    pub head_sha: String,
    pub base_sha: String,
    pub head_branch: String,
    pub base_branch: String,
    pub author_login: String,
    pub readiness: Option<serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EventDeliveryRow {
    pub delivery_id: String,
    pub event_name: String,
    pub action: Option<String>,
    pub installation_id: Option<i64>,
    pub repository_id: Option<i64>,
    pub pull_request_number: Option<i32>,
    pub payload_hash: String,
    pub supported: bool,
    pub received_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
    pub processing_error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AuditRow {
    pub id: Uuid,
    pub repository_id: Option<i64>,
    pub pull_request_number: Option<i32>,
    pub job_id: Option<Uuid>,
    pub actor_login: Option<String>,
    pub event_type: String,
    pub summary: String,
    pub evidence: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct RepositoryContext {
    pub id: i64,
    pub installation_id: i64,
    pub owner: String,
    pub name: String,
    pub default_branch: String,
    pub policy_yaml: String,
}

#[derive(Debug, Clone)]
pub struct PullRequestContext {
    pub repository_id: i64,
    pub number: i32,
    pub github_id: i64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub draft: bool,
    pub merged: bool,
    pub head_sha: String,
    pub base_sha: String,
    pub head_branch: String,
    pub base_branch: String,
    pub author_login: String,
}

#[derive(Debug, Clone)]
pub struct ControlContext {
    pub job_id: Uuid,
    pub repository_id: i64,
    pub pull_request_number: i32,
    pub pr_author: String,
    pub policy_yaml: String,
}

#[derive(Debug, Clone)]
pub struct ExistingWorkHandle {
    pub check_run_id: i64,
    pub check_run_url: String,
    pub comment_id: i64,
}

#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub repository_id: Option<i64>,
    pub pull_request_number: Option<i32>,
    pub job_id: Option<Uuid>,
    pub actor_login: Option<String>,
    pub event_type: String,
    pub summary: String,
    pub evidence: serde_json::Value,
}
