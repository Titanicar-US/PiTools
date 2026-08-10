use std::sync::Arc;

use secrecy::SecretString;

use crate::{metrics::Metrics, queue::JobQueue, repository::Repositories};

#[derive(Clone)]
pub struct AppState {
    pub webhook_secret: SecretString,
    pub repositories: Option<Repositories>,
    pub queue: Option<JobQueue>,
    pub nats: Option<async_nats::Client>,
    pub admin_bearer_token_hash: SecretString,
    pub metrics: Arc<Metrics>,
}
