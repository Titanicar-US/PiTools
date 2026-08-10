use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::Database;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Reconcile,
    FeedbackRepair,
    CiRepair,
    StackRebase,
}

impl JobKind {
    fn as_database_value(&self) -> &'static str {
        match self {
            Self::Reconcile => "reconcile",
            Self::FeedbackRepair => "feedback_repair",
            Self::CiRepair => "ci_repair",
            Self::StackRebase => "stack_rebase",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    WaitingApproval,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSpec {
    pub repository_id: i64,
    pub pull_request_number: i32,
    pub kind: JobKind,
    pub plan: serde_json::Value,
}

impl JobSpec {
    pub fn deduplication_key(&self) -> JobDeduplicationKey {
        JobDeduplicationKey {
            repository_id: self.repository_id,
            pull_request_number: self.pull_request_number,
            kind: self.kind.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobDeduplicationKey {
    repository_id: i64,
    pull_request_number: i32,
    kind: JobKind,
}

#[derive(Debug, Clone)]
pub struct JobQueue {
    database: Database,
    nats: Option<async_nats::Client>,
}

impl JobQueue {
    pub fn new(database: Database, nats: Option<async_nats::Client>) -> Self {
        Self { database, nats }
    }

    pub async fn enqueue(&self, spec: JobSpec) -> Result<Uuid, QueueError> {
        let proposed_id = Uuid::now_v7();
        let deduplication_key = spec.deduplication_key();
        let terminal_head_sha = spec
            .plan
            .get("head_sha")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        if matches!(
            &deduplication_key.kind,
            JobKind::CiRepair | JobKind::StackRebase
        ) && let Some(head_sha) = terminal_head_sha
            && let Some(id) = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM jobs
                 WHERE repository_id = $1 AND pull_request_number = $2
                   AND kind = $3 AND state = 'succeeded'
                   AND plan->>'head_sha' = $4
                 ORDER BY created_at DESC, id DESC LIMIT 1",
            )
            .bind(deduplication_key.repository_id)
            .bind(deduplication_key.pull_request_number)
            .bind(deduplication_key.kind.as_database_value())
            .bind(head_sha)
            .fetch_optional(self.database.pool())
            .await?
        {
            return Ok(id);
        }
        let id = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO jobs (id, repository_id, pull_request_number, kind, state, plan)
             VALUES ($1, $2, $3, $4, 'queued', $5)
             ON CONFLICT (repository_id, pull_request_number, kind)
                 WHERE state IN ('queued', 'running', 'waiting_approval')
             DO UPDATE SET updated_at = jobs.updated_at
             RETURNING jobs.id",
        )
        .bind(proposed_id)
        .bind(deduplication_key.repository_id)
        .bind(deduplication_key.pull_request_number)
        .bind(deduplication_key.kind.as_database_value())
        .bind(spec.plan)
        .fetch_one(self.database.pool())
        .await?;

        if let Some(nats) = &self.nats {
            let subject = format!("pitools.jobs.{id}");
            let payload = serde_json::to_vec(&id)?;
            if let Err(error) = nats.publish(subject, payload.into()).await {
                tracing::warn!(job_id = %id, error = %error, "NATS wakeup failed; Postgres polling will recover the job");
            }
        }
        Ok(id)
    }

    pub async fn request_cancel(&self, job_id: Uuid) -> Result<bool, QueueError> {
        let result = sqlx::query(
            "UPDATE jobs SET state = 'cancelled', cancel_requested = TRUE,
             lease_owner = NULL, lease_until = NULL, lease_token = NULL, updated_at = NOW()
             WHERE id = $1 AND state IN ('queued', 'running', 'waiting_approval')",
        )
        .bind(job_id)
        .execute(self.database.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn skip_current_item(&self, job_id: Uuid) -> Result<bool, QueueError> {
        let result = sqlx::query(
            "UPDATE jobs SET state = 'queued', current_item = NULL,
             plan = jsonb_set(
                 plan,
                 '{skipped_items}',
                 COALESCE(plan->'skipped_items', '[]'::jsonb)
                     || jsonb_build_array(current_item),
                 TRUE
             ),
             attempt = GREATEST(attempt - 1, 0), lease_owner = NULL,
             lease_until = NULL, lease_token = NULL, updated_at = NOW()
             WHERE id = $1 AND state IN ('running', 'waiting_approval')
               AND cancel_requested = FALSE AND NULLIF(current_item, '') IS NOT NULL",
        )
        .bind(job_id)
        .execute(self.database.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn mark_waiting_approval(
        &self,
        job_id: Uuid,
        worker_id: &str,
        lease_token: Uuid,
        details: serde_json::Value,
    ) -> Result<bool, QueueError> {
        let result = sqlx::query(
            "UPDATE jobs SET state = 'waiting_approval', plan = plan || $1,
             result = jsonb_build_object('state', 'waiting_approval'),
             lease_owner = NULL, lease_until = NULL, lease_token = NULL,
             updated_at = NOW()
             WHERE id = $2 AND state = 'running' AND cancel_requested = FALSE
               AND lease_owner = $3 AND lease_token = $4 AND lease_until > NOW()",
        )
        .bind(details)
        .bind(job_id)
        .bind(worker_id)
        .bind(lease_token)
        .execute(self.database.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn approve_plan(&self, job_id: Uuid) -> Result<bool, QueueError> {
        let result = sqlx::query(
            "UPDATE jobs SET state = 'queued',
             plan = jsonb_set(plan, '{approved}', 'true'::jsonb, TRUE),
             result = NULL, updated_at = NOW()
             WHERE id = $1 AND state = 'waiting_approval' AND cancel_requested = FALSE",
        )
        .bind(job_id)
        .execute(self.database.pool())
        .await?;
        let changed = result.rows_affected() == 1;
        if changed && let Some(nats) = &self.nats {
            let subject = format!("pitools.jobs.{job_id}");
            if let Err(error) = nats
                .publish(subject, serde_json::to_vec(&job_id)?.into())
                .await
            {
                tracing::warn!(job_id = %job_id, error = %error, "NATS approval wakeup failed; Postgres polling will recover the job");
            }
        }
        Ok(changed)
    }

    pub async fn lease_next(&self, worker_id: &str) -> Result<Option<LeasedJob>, QueueError> {
        let mut transaction = self.database.pool().begin().await?;
        sqlx::query(
            "UPDATE jobs
             SET state = 'failed',
                 result = jsonb_build_object(
                     'reason', 'max_attempts_exhausted',
                     'attempt', attempt,
                     'max_attempts', max_attempts
                 ),
                 lease_owner = NULL,
                 lease_until = NULL,
                 lease_token = NULL,
                 updated_at = NOW()
             WHERE cancel_requested = FALSE
               AND state IN ('queued', 'running')
               AND attempt >= max_attempts
               AND (state = 'queued' OR lease_until IS NULL OR lease_until <= NOW())",
        )
        .execute(&mut *transaction)
        .await?;
        let row = sqlx::query(
            "SELECT id, repository_id, pull_request_number, kind, plan
             FROM jobs
             WHERE cancel_requested = FALSE
               AND attempt < max_attempts
               AND (
                   state = 'queued'
                   OR (state = 'running' AND (lease_until IS NULL OR lease_until <= NOW()))
               )
             ORDER BY created_at, id
             FOR UPDATE SKIP LOCKED LIMIT 1",
        )
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(row) = row else {
            transaction.commit().await?;
            return Ok(None);
        };
        let job_id: Uuid = sqlx::Row::get(&row, "id");
        let lease_token = Uuid::now_v7();
        let lease: (chrono::DateTime<Utc>, i32, i32) = sqlx::query_as(
            "UPDATE jobs SET state = 'running', lease_owner = $1,
             lease_until = NOW() + INTERVAL '5 minutes', lease_token = $2,
             attempt = attempt + 1, updated_at = NOW() WHERE id = $3
             RETURNING lease_until, attempt, max_attempts",
        )
        .bind(worker_id)
        .bind(lease_token)
        .bind(job_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(Some(LeasedJob {
            id: job_id,
            repository_id: sqlx::Row::get(&row, "repository_id"),
            pull_request_number: sqlx::Row::get(&row, "pull_request_number"),
            kind: sqlx::Row::get(&row, "kind"),
            plan: sqlx::Row::get(&row, "plan"),
            lease_until: lease.0,
            lease_token,
            attempt: lease.1,
            max_attempts: lease.2,
        }))
    }

    pub async fn assert_lease(
        &self,
        job_id: Uuid,
        worker_id: &str,
        lease_token: Uuid,
    ) -> Result<bool, QueueError> {
        let row = sqlx::query_scalar::<_, bool>(
            "SELECT (state = 'running' AND cancel_requested = FALSE
                     AND lease_owner = $2 AND lease_token = $3 AND lease_until > NOW())
             FROM jobs WHERE id = $1",
        )
        .bind(job_id)
        .bind(worker_id)
        .bind(lease_token)
        .fetch_optional(self.database.pool())
        .await?;
        Ok(row.unwrap_or(false))
    }

    pub async fn renew_lease(
        &self,
        job_id: Uuid,
        worker_id: &str,
        lease_token: Uuid,
    ) -> Result<bool, QueueError> {
        let result = sqlx::query(
            "UPDATE jobs SET lease_until = NOW() + INTERVAL '5 minutes', updated_at = NOW()
             WHERE id = $1 AND state = 'running' AND cancel_requested = FALSE
               AND lease_owner = $2 AND lease_token = $3 AND lease_until > NOW()",
        )
        .bind(job_id)
        .bind(worker_id)
        .bind(lease_token)
        .execute(self.database.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn mark_succeeded(
        &self,
        job_id: Uuid,
        worker_id: &str,
        lease_token: Uuid,
        result: serde_json::Value,
    ) -> Result<bool, QueueError> {
        let changed = sqlx::query(
            "UPDATE jobs SET state = 'succeeded', result = $1, lease_owner = NULL,
             lease_until = NULL, lease_token = NULL, updated_at = NOW()
             WHERE id = $2 AND state = 'running' AND cancel_requested = FALSE
               AND lease_owner = $3 AND lease_token = $4 AND lease_until > NOW()",
        )
        .bind(result)
        .bind(job_id)
        .bind(worker_id)
        .bind(lease_token)
        .execute(self.database.pool())
        .await?;
        Ok(changed.rows_affected() == 1)
    }

    pub async fn mark_failed(
        &self,
        job_id: Uuid,
        worker_id: &str,
        lease_token: Uuid,
        result: serde_json::Value,
    ) -> Result<bool, QueueError> {
        let changed = sqlx::query(
            "UPDATE jobs SET state = 'failed', result = $1, lease_owner = NULL,
             lease_until = NULL, lease_token = NULL,
             updated_at = NOW()
             WHERE id = $2 AND state = 'running' AND cancel_requested = FALSE
               AND lease_owner = $3 AND lease_token = $4 AND lease_until > NOW()",
        )
        .bind(result)
        .bind(job_id)
        .bind(worker_id)
        .bind(lease_token)
        .execute(self.database.pool())
        .await?;
        Ok(changed.rows_affected() == 1)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LeasedJob {
    pub id: Uuid,
    pub repository_id: i64,
    pub pull_request_number: i32,
    pub kind: String,
    pub plan: serde_json::Value,
    pub lease_until: chrono::DateTime<Utc>,
    pub lease_token: Uuid,
    pub attempt: i32,
    pub max_attempts: i32,
}

#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
