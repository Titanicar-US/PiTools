use std::{
    collections::BTreeMap,
    io::{self, Read},
    net::SocketAddr,
    sync::Arc,
};

use anyhow::Result;
use argon2::{Argon2, PasswordHasher};
use clap::{Parser, Subcommand};
use pitools::{
    AppConfig,
    db::Database,
    github::{
        auth::{GitHubAppAuth, InstallationTokenScope},
        client::GitHubClient,
    },
    metrics::Metrics,
    models::{PullRequestSnapshot, PullRequestState},
    pi::NatsPiWorker,
    queue::{JobKind, JobQueue, JobSpec},
    repository::Repositories,
    state::AppState,
    web,
};
use secrecy::ExposeSecret;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use url::Url;

#[derive(Debug, Parser)]
#[command(
    name = "pitools",
    version,
    about = "GitHub pull request readiness control plane"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the webhook receiver and operator API.
    Server,
    /// Run the bounded job worker.
    Worker,
    /// Validate local configuration and service dependencies.
    Doctor,
    /// Request reconciliation of watched pull requests.
    Reconcile {
        /// GitHub repository database ID.
        #[arg(long)]
        repository_id: i64,
        /// Pull request number within the repository.
        #[arg(long)]
        pull_request_number: i32,
    },
    /// List watched pull requests from the durable state store.
    Watchlist {
        /// Render machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// List recent webhook delivery metadata without raw payloads.
    Events {
        /// Maximum number of rows to return, capped at 100.
        #[arg(long, default_value_t = 50)]
        limit: i64,
        /// Render machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// List recent redacted audit entries.
    Audit {
        /// Maximum number of rows to return, capped at 100.
        #[arg(long, default_value_t = 50)]
        limit: i64,
        /// Render machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Request cancellation of a queued or active job.
    Cancel {
        /// Job UUID returned by reconcile or the operator API.
        job_id: uuid::Uuid,
    },
    /// Read a bearer token from stdin and print its Argon2id PHC hash.
    HashToken,
    /// Render the GitHub App Manifest JSON for an HTTPS public base URL.
    Manifest {
        /// Public HTTPS URL that fronts PiTools.
        #[arg(long)]
        base_url: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::var_os("PITOOLS_GIT_ASKPASS").is_some() {
        return run_git_askpass();
    }
    let cli = Cli::parse();
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    match cli.command {
        Command::Server => {
            let config = AppConfig::from_env()?;
            let github_auth = GitHubAppAuth::new(
                config.github_app_id,
                config.github_private_key.clone(),
                Url::parse("https://api.github.com/")?,
            )?;
            let database = Database::connect(config.database_url.expose_secret()).await?;
            database.migrate().await?;
            let nats = match async_nats::connect(&config.nats_url).await {
                Ok(client) => Some(client),
                Err(error) => {
                    tracing::warn!(error = %error, "NATS unavailable; falling back to Postgres polling");
                    None
                }
            };
            let repositories = Repositories::new(database.clone());
            let queue = JobQueue::new(database, nats);
            tokio::spawn(periodic_reconcile_loop(
                repositories.clone(),
                queue.clone(),
                github_auth,
                config.reconcile_interval_seconds,
            ));
            let state = AppState {
                webhook_secret: config.github_webhook_secret.clone(),
                repositories: Some(repositories),
                queue: Some(queue),
                admin_bearer_token_hash: config.admin_bearer_token_hash.clone(),
                metrics: Arc::new(Metrics::default()),
            };
            let address: SocketAddr = config.bind_address.parse()?;
            let listener = tokio::net::TcpListener::bind(address).await?;
            tracing::info!(%address, "starting PiTools server");
            axum::serve(listener, web::router_with_state(state)).await?;
        }
        Command::Worker => {
            let config = AppConfig::from_env()?;
            let github_auth = GitHubAppAuth::new(
                config.github_app_id,
                config.github_private_key.clone(),
                Url::parse("https://api.github.com/")?,
            )?;
            let database = Database::connect(config.database_url.expose_secret()).await?;
            database.migrate().await?;
            let nats = match async_nats::connect(&config.nats_url).await {
                Ok(client) => Some(client),
                Err(error) => {
                    tracing::warn!(error = %error, "NATS unavailable; worker will poll Postgres");
                    None
                }
            };
            let repositories = Repositories::new(database.clone());
            let pi_worker = nats.clone().map(NatsPiWorker::new);
            let queue = JobQueue::new(database, nats);
            pitools::worker::WorkerRuntime::new(queue, repositories, github_auth, pi_worker)
                .run()
                .await?;
        }
        Command::Doctor => {
            let config = AppConfig::from_env()?;
            GitHubAppAuth::new(
                config.github_app_id,
                config.github_private_key.clone(),
                Url::parse("https://api.github.com/")?,
            )?;
            let database = Database::connect(config.database_url.expose_secret()).await?;
            database.migrate().await?;
            database.ping().await?;
            let nats_status = match async_nats::connect(&config.nats_url).await {
                Ok(_) => "ok",
                Err(error) => {
                    eprintln!("warning: NATS unavailable; Postgres polling will be used: {error}");
                    "degraded"
                }
            };
            println!(
                "configuration ok: bind_address={}, database=ok, github_key=ok, nats={nats_status}",
                config.bind_address
            );
        }
        Command::Reconcile {
            repository_id,
            pull_request_number,
        } => {
            let config = AppConfig::from_env()?;
            let database = Database::connect(config.database_url.expose_secret()).await?;
            database.migrate().await?;
            let nats = async_nats::connect(&config.nats_url).await.ok();
            let queue = JobQueue::new(database, nats);
            let job_id = queue
                .enqueue(JobSpec {
                    repository_id,
                    pull_request_number,
                    kind: JobKind::Reconcile,
                    plan: serde_json::json!({"source": "cli"}),
                })
                .await?;
            println!("reconciliation job queued: {job_id}");
        }
        Command::Watchlist { json } => {
            let config = AppConfig::from_env()?;
            let database = Database::connect(config.database_url.expose_secret()).await?;
            database.migrate().await?;
            let rows = Repositories::new(database).open_pull_requests().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for row in rows {
                    println!(
                        "{}#{} {} [{}] {}",
                        row.repository_id, row.number, row.title, row.state, row.url
                    );
                }
            }
        }
        Command::Events { limit, json } => {
            let config = AppConfig::from_env()?;
            let database = Database::connect(config.database_url.expose_secret()).await?;
            database.migrate().await?;
            let rows = Repositories::new(database).recent_events(limit).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for row in rows {
                    println!(
                        "{} {} {} {}",
                        row.received_at,
                        row.delivery_id,
                        row.event_name,
                        row.action.as_deref().unwrap_or("-")
                    );
                }
            }
        }
        Command::Audit { limit, json } => {
            let config = AppConfig::from_env()?;
            let database = Database::connect(config.database_url.expose_secret()).await?;
            database.migrate().await?;
            let rows = Repositories::new(database)
                .recent_audit_entries(limit)
                .await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for row in rows {
                    println!("{} {} {}", row.created_at, row.event_type, row.summary);
                }
            }
        }
        Command::Cancel { job_id } => {
            let config = AppConfig::from_env()?;
            let database = Database::connect(config.database_url.expose_secret()).await?;
            database.migrate().await?;
            let nats = async_nats::connect(&config.nats_url).await.ok();
            let changed = JobQueue::new(database, nats).request_cancel(job_id).await?;
            println!("job {job_id} cancellation_requested={changed}");
        }
        Command::HashToken => {
            let mut token = String::new();
            io::stdin().read_to_string(&mut token)?;
            let hash = Argon2::default().hash_password(token.trim_end().as_bytes())?;
            println!("{hash}");
        }
        Command::Manifest { base_url } => {
            let manifest = pitools::github::manifest::AppManifest::for_public_project(&base_url);
            println!("{}", serde_json::to_string_pretty(&manifest.as_json()?)?);
        }
    }

    Ok(())
}

fn run_git_askpass() -> Result<()> {
    let prompt = std::env::args().nth(1).unwrap_or_default();
    let variable = if prompt.to_ascii_lowercase().contains("username") {
        "PITOOLS_GIT_USERNAME"
    } else {
        "PITOOLS_GIT_PASSWORD"
    };
    let value = std::env::var(variable)
        .map_err(|_| anyhow::anyhow!("{variable} is unavailable to the Git askpass helper"))?;
    println!("{value}");
    Ok(())
}

async fn periodic_reconcile_loop(
    repositories: Repositories,
    queue: JobQueue,
    github_auth: GitHubAppAuth,
    interval_seconds: u64,
) {
    let interval = std::time::Duration::from_secs(interval_seconds);
    loop {
        match repositories.active_repository_contexts().await {
            Ok(repository_contexts) => {
                for repository in repository_contexts {
                    if let Err(error) = reconcile_repository_inventory(
                        &repositories,
                        &queue,
                        &github_auth,
                        &repository,
                    )
                    .await
                    {
                        tracing::warn!(
                            repository_id = repository.id,
                            owner = %repository.owner,
                            name = %repository.name,
                            error = %error,
                            "periodic repository inventory failed"
                        );
                    }
                }
            }
            Err(error) => {
                tracing::warn!(error = %error, "periodic watchlist reconciliation failed")
            }
        }
        tokio::time::sleep(interval).await;
    }
}

async fn reconcile_repository_inventory(
    repositories: &Repositories,
    queue: &JobQueue,
    github_auth: &GitHubAppAuth,
    repository: &pitools::repository::RepositoryContext,
) -> Result<(), anyhow::Error> {
    let permissions = BTreeMap::from([
        ("contents".to_string(), "read".to_string()),
        ("metadata".to_string(), "read".to_string()),
        ("pull_requests".to_string(), "read".to_string()),
    ]);
    let token = github_auth
        .installation_token_with_scope(
            repository.installation_id,
            &InstallationTokenScope {
                permissions,
                repositories: vec![repository.name.clone()],
            },
        )
        .await?;
    let github = GitHubClient::new(github_auth.api_base(), token)?;
    for pull_request in github
        .list_open_pull_requests(&repository.owner, &repository.name)
        .await?
    {
        let Some(head_branch) = pull_request.head.reference.clone() else {
            tracing::warn!(
                repository_id = repository.id,
                pull_request_number = pull_request.number,
                "periodic inventory skipped PR without a head branch"
            );
            continue;
        };
        let Some(base_branch) = pull_request.base.reference.clone() else {
            tracing::warn!(
                repository_id = repository.id,
                pull_request_number = pull_request.number,
                "periodic inventory skipped PR without a base branch"
            );
            continue;
        };
        let snapshot = PullRequestSnapshot {
            repository_id: repository.id,
            github_id: pull_request.id,
            number: pull_request.number,
            title: pull_request.title,
            url: pull_request.html_url,
            state: if pull_request.merged {
                PullRequestState::Merged
            } else if pull_request.state == "closed" {
                PullRequestState::Closed
            } else {
                PullRequestState::Open
            },
            draft: pull_request.draft,
            merged: pull_request.merged,
            head_sha: pull_request.head.sha,
            base_sha: pull_request.base.sha,
            head_branch,
            base_branch,
            author_login: pull_request.user.login,
            updated_at: pull_request.updated_at.unwrap_or_else(chrono::Utc::now),
        };
        repositories.upsert_pull_request(&snapshot).await?;
        queue
            .enqueue(JobSpec {
                repository_id: repository.id,
                pull_request_number: snapshot.number,
                kind: JobKind::Reconcile,
                plan: serde_json::json!({"source": "periodic-inventory"}),
            })
            .await?;
    }

    // Keep watched rows in the queue too, including rows that disappeared
    // from the open inventory. A fresh PR read then records close/merge state
    // and removes them from the watchlist.
    for pull_request in repositories.open_pull_requests().await? {
        queue
            .enqueue(JobSpec {
                repository_id: pull_request.repository_id,
                pull_request_number: pull_request.number,
                kind: JobKind::Reconcile,
                plan: serde_json::json!({"source": "periodic-watchlist"}),
            })
            .await?;
    }
    Ok(())
}
