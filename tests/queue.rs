use chrono::{Duration, Utc};
use pitools::{
    db::Database,
    queue::{JobKind, JobQueue, JobSpec},
    repository::Repositories,
};
use sqlx::{AssertSqlSafe, PgPool, postgres::PgPoolOptions};
use url::Url;
use uuid::Uuid;

fn job_spec(repository_id: i64, pull_request_number: i32, kind: JobKind) -> JobSpec {
    JobSpec {
        repository_id,
        pull_request_number,
        kind,
        plan: serde_json::json!({"item": "first"}),
    }
}

#[test]
fn logical_job_identity_ignores_mutable_plan_content() {
    let first = job_spec(42, 7, JobKind::Reconcile);
    let mut repeated = job_spec(42, 7, JobKind::Reconcile);
    repeated.plan = serde_json::json!({"item": "replacement"});

    assert_eq!(first.deduplication_key(), repeated.deduplication_key());
}

#[test]
fn logical_job_identity_includes_target_and_kind() {
    let baseline = job_spec(42, 7, JobKind::Reconcile).deduplication_key();

    assert_ne!(
        baseline,
        job_spec(43, 7, JobKind::Reconcile).deduplication_key()
    );
    assert_ne!(
        baseline,
        job_spec(42, 8, JobKind::Reconcile).deduplication_key()
    );
    assert_ne!(
        baseline,
        job_spec(42, 7, JobKind::FeedbackRepair).deduplication_key()
    );
}

struct PostgresFixture {
    admin_pool: PgPool,
    database: Database,
    installation_id: i64,
    repository_id: i64,
    schema: String,
}

impl PostgresFixture {
    async fn start() -> Option<Self> {
        let Ok(database_url) = std::env::var("DATABASE_URL") else {
            eprintln!(
                "SKIPPED Postgres queue contracts: DATABASE_URL is not set; pure queue contracts still ran"
            );
            return None;
        };
        let admin_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .expect("DATABASE_URL must connect for queue contract tests");
        let schema = format!("queue_test_{}", Uuid::new_v4().simple());
        sqlx::query(AssertSqlSafe(format!(r#"CREATE SCHEMA "{schema}""#)))
            .execute(&admin_pool)
            .await
            .expect("create isolated queue test schema");

        let mut scoped_url = Url::parse(&database_url).expect("DATABASE_URL must be a URL");
        scoped_url
            .query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"));
        let database = Database::connect(scoped_url.as_str())
            .await
            .expect("connect to isolated queue test schema");
        if let Err(error) = database.migrate().await {
            database.pool().close().await;
            sqlx::query(AssertSqlSafe(format!(r#"DROP SCHEMA "{schema}" CASCADE"#)))
                .execute(&admin_pool)
                .await
                .expect("remove schema after migration failure");
            panic!("queue migrations must apply cleanly: {error}");
        }

        let installation_id = generated_database_id();
        let repository_id = generated_database_id();
        sqlx::query(
            "INSERT INTO installations (id, account_login, account_type) VALUES ($1, $2, $3)",
        )
        .bind(installation_id)
        .bind(format!("queue-test-{installation_id}"))
        .bind("Organization")
        .execute(database.pool())
        .await
        .expect("insert queue test installation");
        sqlx::query(
            "INSERT INTO repositories (id, installation_id, owner, name, default_branch)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(repository_id)
        .bind(installation_id)
        .bind("queue-tests")
        .bind(format!("repository-{repository_id}"))
        .bind("main")
        .execute(database.pool())
        .await
        .expect("insert queue test repository");

        Some(Self {
            admin_pool,
            database,
            installation_id,
            repository_id,
            schema,
        })
    }

    fn queue(&self) -> JobQueue {
        JobQueue::new(self.database.clone(), None)
    }

    fn spec(&self, pull_request_number: i32, kind: JobKind, source: &str) -> JobSpec {
        JobSpec {
            repository_id: self.repository_id,
            pull_request_number,
            kind,
            plan: serde_json::json!({"source": source}),
        }
    }

    async fn cleanup(self) {
        sqlx::query("DELETE FROM installations WHERE id = $1")
            .bind(self.installation_id)
            .execute(self.database.pool())
            .await
            .expect("delete queue test installation");
        self.database.pool().close().await;
        sqlx::query(AssertSqlSafe(format!(
            r#"DROP SCHEMA "{}" CASCADE"#,
            self.schema
        )))
        .execute(&self.admin_pool)
        .await
        .expect("drop isolated queue test schema");
        self.admin_pool.close().await;
    }
}

fn generated_database_id() -> i64 {
    (Uuid::new_v4().as_u128() & i64::MAX as u128) as i64
}

#[tokio::test]
async fn postgres_open_watchlist_query_returns_active_pull_requests() {
    let Some(fixture) = PostgresFixture::start().await else {
        return;
    };
    sqlx::query(
        "INSERT INTO pull_requests
            (repository_id, number, github_id, title, url, state, head_sha, base_sha,
             head_branch, base_branch, author_login, watched)
         VALUES ($1, 7, 1007, 'PR', 'https://github.com/acme/widgets/pull/7', 'open',
                 'head', 'base', 'feature', 'main', 'author', TRUE)",
    )
    .bind(fixture.repository_id)
    .execute(fixture.database.pool())
    .await
    .expect("insert watched pull request");

    let rows = Repositories::new(fixture.database.clone())
        .open_pull_requests()
        .await
        .expect("read active watchlist");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].repository_id, fixture.repository_id);
    assert_eq!(rows[0].number, 7);

    fixture.cleanup().await;
}

#[tokio::test]
async fn postgres_queue_hardening_contracts() {
    let Some(fixture) = PostgresFixture::start().await else {
        return;
    };
    let queue = fixture.queue();

    let first_spec = fixture.spec(101, JobKind::Reconcile, "webhook");
    let repeated_spec = fixture.spec(101, JobKind::Reconcile, "operator");
    let (first_id, repeated_id) =
        tokio::join!(queue.enqueue(first_spec), queue.enqueue(repeated_spec));
    let first_id = first_id.expect("enqueue first logical job");
    assert_eq!(
        first_id,
        repeated_id.expect("deduplicate repeated logical job")
    );
    let active_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jobs
         WHERE repository_id = $1 AND pull_request_number = 101 AND kind = 'reconcile'",
    )
    .bind(fixture.repository_id)
    .fetch_one(fixture.database.pool())
    .await
    .expect("count deduplicated jobs");
    assert_eq!(active_count, 1);
    assert!(
        queue
            .request_cancel(first_id)
            .await
            .expect("cancel first job")
    );
    let replacement_id = queue
        .enqueue(fixture.spec(101, JobKind::Reconcile, "later-webhook"))
        .await
        .expect("enqueue after terminal job");
    assert_ne!(first_id, replacement_id);
    assert!(
        queue
            .request_cancel(replacement_id)
            .await
            .expect("cancel replacement job")
    );

    let cancellation_id = queue
        .enqueue(fixture.spec(102, JobKind::CiRepair, "check-run"))
        .await
        .expect("enqueue cancellable job");
    let cancellation_lease = queue
        .lease_next("worker-cancel")
        .await
        .expect("lease cancellable job")
        .expect("cancellable job is available");
    assert_eq!(cancellation_lease.id, cancellation_id);
    assert_eq!(cancellation_lease.attempt, 1);
    assert_eq!(cancellation_lease.max_attempts, 5);
    assert!(
        queue
            .request_cancel(cancellation_id)
            .await
            .expect("request running cancellation")
    );
    assert!(
        !queue
            .assert_lease(
                cancellation_id,
                "worker-cancel",
                cancellation_lease.lease_token,
            )
            .await
            .expect("check cancelled lease fence")
    );
    assert!(
        !queue
            .mark_succeeded(
                cancellation_id,
                "worker-cancel",
                cancellation_lease.lease_token,
                serde_json::json!({"late": true}),
            )
            .await
            .expect("reject stale completion after cancellation")
    );
    let cancelled: (String, bool, Option<String>, Option<Uuid>) = sqlx::query_as(
        "SELECT state, cancel_requested, lease_owner, lease_token FROM jobs WHERE id = $1",
    )
    .bind(cancellation_id)
    .fetch_one(fixture.database.pool())
    .await
    .expect("read cancelled job");
    assert_eq!(cancelled, ("cancelled".into(), true, None, None));

    let approval_id = queue
        .enqueue(fixture.spec(105, JobKind::CiRepair, "approval"))
        .await
        .expect("enqueue approval job");
    let approval_lease = queue
        .lease_next("worker-approval")
        .await
        .expect("lease approval job")
        .expect("approval job is available");
    assert!(
        queue
            .mark_waiting_approval(
                approval_id,
                "worker-approval",
                approval_lease.lease_token,
                serde_json::json!({"diagnosis": "needs approval"}),
            )
            .await
            .expect("move job to approval state")
    );
    let waiting: (String, serde_json::Value) =
        sqlx::query_as("SELECT state, plan FROM jobs WHERE id = $1")
            .bind(approval_id)
            .fetch_one(fixture.database.pool())
            .await
            .expect("read waiting job");
    assert_eq!(waiting.0, "waiting_approval");
    assert_eq!(waiting.1["diagnosis"], "needs approval");
    assert!(queue.approve_plan(approval_id).await.expect("approve job"));
    let approved: (String, bool) =
        sqlx::query_as("SELECT state, (plan->>'approved')::BOOLEAN FROM jobs WHERE id = $1")
            .bind(approval_id)
            .fetch_one(fixture.database.pool())
            .await
            .expect("read approved job");
    assert_eq!(approved, ("queued".into(), true));

    let waiting_skip_id = queue
        .enqueue(fixture.spec(106, JobKind::CiRepair, "waiting-skip"))
        .await
        .expect("enqueue waiting skippable job");
    sqlx::query("UPDATE jobs SET current_item = 'ci_repair' WHERE id = $1")
        .bind(waiting_skip_id)
        .execute(fixture.database.pool())
        .await
        .expect("set waiting queue item");
    let waiting_skip_lease = queue
        .lease_next("worker-waiting-skip")
        .await
        .expect("lease waiting skippable job")
        .expect("waiting skippable job is available");
    assert!(
        queue
            .mark_waiting_approval(
                waiting_skip_id,
                "worker-waiting-skip",
                waiting_skip_lease.lease_token,
                serde_json::json!({"diagnosis": "waiting"}),
            )
            .await
            .expect("move skippable job to approval state")
    );
    assert!(
        queue
            .skip_current_item(waiting_skip_id)
            .await
            .expect("skip waiting queue item")
    );
    let waiting_skipped: (String, Option<String>, serde_json::Value) =
        sqlx::query_as("SELECT state, current_item, plan->'skipped_items' FROM jobs WHERE id = $1")
            .bind(waiting_skip_id)
            .fetch_one(fixture.database.pool())
            .await
            .expect("read waiting skipped job");
    assert_eq!(waiting_skipped.0, "queued");
    assert_eq!(waiting_skipped.1, None);
    assert_eq!(waiting_skipped.2, serde_json::json!(["ci_repair"]));
    assert!(
        queue
            .request_cancel(waiting_skip_id)
            .await
            .expect("cancel skipped waiting job fixture")
    );

    let skipped_id = queue
        .enqueue(fixture.spec(103, JobKind::FeedbackRepair, "review"))
        .await
        .expect("enqueue skippable job");
    sqlx::query("UPDATE jobs SET current_item = 'feedback-1' WHERE id = $1")
        .bind(skipped_id)
        .execute(fixture.database.pool())
        .await
        .expect("set current queue item");
    let skipped_lease = queue
        .lease_next("worker-skip")
        .await
        .expect("lease skippable job")
        .expect("skippable job is available");
    assert!(
        queue
            .skip_current_item(skipped_id)
            .await
            .expect("skip current queue item")
    );
    assert!(
        !queue
            .assert_lease(skipped_id, "worker-skip", skipped_lease.lease_token)
            .await
            .expect("check skipped lease fence")
    );
    let released: (String, Option<String>, Option<String>, Option<Uuid>, i32) = sqlx::query_as(
        "SELECT state, current_item, lease_owner, lease_token, attempt FROM jobs WHERE id = $1",
    )
    .bind(skipped_id)
    .fetch_one(fixture.database.pool())
    .await
    .expect("read released skipped job");
    assert_eq!(released, ("queued".into(), None, None, None, 0));
    let resumed_lease = queue
        .lease_next("worker-resume")
        .await
        .expect("re-lease skipped job")
        .expect("skipped job is available again");
    assert_eq!(resumed_lease.id, skipped_id);
    assert_eq!(resumed_lease.attempt, 1);
    assert_ne!(resumed_lease.lease_token, skipped_lease.lease_token);
    assert!(
        queue
            .mark_failed(
                skipped_id,
                "worker-resume",
                resumed_lease.lease_token,
                serde_json::json!({"test_cleanup": true}),
            )
            .await
            .expect("finish skipped job fixture")
    );

    let exhausted_id = queue
        .enqueue(fixture.spec(104, JobKind::StackRebase, "stack"))
        .await
        .expect("enqueue retry-limited job");
    sqlx::query("UPDATE jobs SET max_attempts = 2 WHERE id = $1")
        .bind(exhausted_id)
        .execute(fixture.database.pool())
        .await
        .expect("set explicit max attempts");
    let first_lease = queue
        .lease_next("worker-one")
        .await
        .expect("lease first attempt")
        .expect("first attempt is available");
    assert_eq!((first_lease.attempt, first_lease.max_attempts), (1, 2));
    sqlx::query("UPDATE jobs SET lease_until = $1 WHERE id = $2")
        .bind(Utc::now() - Duration::minutes(1))
        .bind(exhausted_id)
        .execute(fixture.database.pool())
        .await
        .expect("expire first lease");
    let final_lease = queue
        .lease_next("worker-two")
        .await
        .expect("lease final attempt")
        .expect("final attempt is available");
    assert_eq!((final_lease.attempt, final_lease.max_attempts), (2, 2));
    assert_ne!(final_lease.lease_token, first_lease.lease_token);
    sqlx::query("UPDATE jobs SET lease_until = $1 WHERE id = $2")
        .bind(Utc::now() - Duration::minutes(1))
        .bind(exhausted_id)
        .execute(fixture.database.pool())
        .await
        .expect("expire final lease");
    assert!(
        queue
            .lease_next("worker-three")
            .await
            .expect("process exhausted lease")
            .is_none()
    );
    let exhausted: (String, serde_json::Value, Option<String>, Option<Uuid>) =
        sqlx::query_as("SELECT state, result, lease_owner, lease_token FROM jobs WHERE id = $1")
            .bind(exhausted_id)
            .fetch_one(fixture.database.pool())
            .await
            .expect("read exhausted job");
    assert_eq!(exhausted.0, "failed");
    assert_eq!(exhausted.1["reason"], "max_attempts_exhausted");
    assert_eq!(exhausted.1["attempt"], 2);
    assert_eq!(exhausted.1["max_attempts"], 2);
    assert_eq!((exhausted.2, exhausted.3), (None, None));

    drop(queue);
    fixture.cleanup().await;
}
