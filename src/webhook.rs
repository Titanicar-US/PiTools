use axum::{body::Bytes, extract::State, http::HeaderMap, response::IntoResponse};
use hmac::{Hmac, KeyInit, Mac};
use secrecy::ExposeSecret;
use sha2::{Digest, Sha256};

use crate::{github::events::DeliveryEnvelope, state::AppState};

type HmacSha256 = Hmac<Sha256>;

pub type WebhookState = AppState;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WebhookError {
    #[error("missing GitHub webhook signature")]
    MissingSignature,
    #[error("invalid GitHub webhook signature")]
    InvalidSignature,
    #[error("missing GitHub delivery ID")]
    MissingDeliveryId,
    #[error("invalid GitHub event payload")]
    InvalidPayload,
    #[error("payload is too large")]
    PayloadTooLarge,
    #[error("persistence is not configured")]
    PersistenceUnavailable,
    #[error("repository error: {0}")]
    Repository(String),
}

impl axum::response::IntoResponse for WebhookError {
    fn into_response(self) -> axum::response::Response {
        let status = match self {
            Self::MissingSignature | Self::InvalidSignature | Self::MissingDeliveryId => {
                axum::http::StatusCode::UNAUTHORIZED
            }
            Self::InvalidPayload => axum::http::StatusCode::BAD_REQUEST,
            Self::PayloadTooLarge => axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            Self::PersistenceUnavailable => axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Self::Repository(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, self.to_string()).into_response()
    }
}

pub fn verify_signature(secret: &str, body: &[u8], header: &str) -> Result<(), WebhookError> {
    let signature = header
        .strip_prefix("sha256=")
        .ok_or(WebhookError::InvalidSignature)?;
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(secret.as_bytes())
        .map_err(|_| WebhookError::InvalidSignature)?;
    mac.update(body);
    let expected = hex::encode(mac.finalize().into_bytes());
    if signature.len() != expected.len()
        || !constant_time_equal(signature.as_bytes(), expected.as_bytes())
    {
        return Err(WebhookError::InvalidSignature);
    }
    Ok(())
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

pub async fn receive(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, WebhookError> {
    state.metrics.record_webhook_received();
    if body.len() > 2 * 1024 * 1024 {
        state.metrics.record_webhook_rejected();
        return Err(WebhookError::PayloadTooLarge);
    }
    let signature = match headers
        .get("x-hub-signature-256")
        .and_then(|value| value.to_str().ok())
        .ok_or(WebhookError::MissingSignature)
    {
        Ok(signature) => signature,
        Err(error) => {
            state.metrics.record_webhook_rejected();
            return Err(error);
        }
    };
    if let Err(error) = verify_signature(state.webhook_secret.expose_secret(), &body, signature) {
        state.metrics.record_webhook_rejected();
        return Err(error);
    }
    let delivery_id = match headers
        .get("x-github-delivery")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or(WebhookError::MissingDeliveryId)
    {
        Ok(delivery_id) => delivery_id.to_string(),
        Err(error) => {
            state.metrics.record_webhook_rejected();
            return Err(error);
        }
    };
    let event_name = headers
        .get("x-github-event")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("unknown")
        .to_string();
    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(_) => {
            state.metrics.record_webhook_rejected();
            return Err(WebhookError::InvalidPayload);
        }
    };
    let envelope = DeliveryEnvelope::from_payload(delivery_id, event_name, payload, body.to_vec());
    let repositories = state.repositories.ok_or_else(|| {
        state.metrics.record_webhook_rejected();
        WebhookError::PersistenceUnavailable
    })?;
    let outcome = repositories
        .process_delivery(&envelope)
        .await
        .map_err(|error| {
            state.metrics.record_webhook_rejected();
            WebhookError::Repository(error.to_string())
        })?;
    if matches!(
        outcome,
        crate::repository::DeliveryOutcome::Inserted
            | crate::repository::DeliveryOutcome::Duplicate
    ) && let Some(queue) = state.queue.as_ref()
    {
        let control = envelope
            .check_run_control()
            .map_err(|error| WebhookError::Repository(error.to_string()))?;
        crate::workflow::process_check_run_control(&repositories, queue, &envelope)
            .await
            .map_err(|error| WebhookError::Repository(error.to_string()))?;
        if control.is_none() && envelope.is_supported() {
            let mut targets = Vec::new();
            if let (Some(repository_id), Some(pull_request_number)) =
                (envelope.repository_id, envelope.pull_request_number)
            {
                targets.push((repository_id, pull_request_number));
            } else if let (Some(repository_id), Some(head_sha)) =
                (envelope.repository_id, envelope.head_sha())
            {
                let pull_requests = repositories
                    .open_pull_requests_for_head(repository_id, head_sha)
                    .await
                    .map_err(|error| WebhookError::Repository(error.to_string()))?;
                targets.extend(
                    pull_requests
                        .into_iter()
                        .map(|pull_request_number| (repository_id, pull_request_number)),
                );
            }
            for (repository_id, pull_request_number) in targets {
                queue
                    .enqueue(crate::queue::JobSpec {
                        repository_id,
                        pull_request_number,
                        kind: crate::queue::JobKind::Reconcile,
                        plan: serde_json::json!({
                            "source": "webhook",
                            "event": envelope.event_name.clone(),
                            "action": envelope.action.clone(),
                        }),
                    })
                    .await
                    .map_err(|error| WebhookError::Repository(error.to_string()))?;
            }
        }
    }
    let response = match outcome {
        crate::repository::DeliveryOutcome::Inserted => {
            state.metrics.record_webhook_accepted();
            (axum::http::StatusCode::ACCEPTED, "accepted")
        }
        crate::repository::DeliveryOutcome::Duplicate => {
            state.metrics.record_webhook_duplicate();
            (axum::http::StatusCode::ALREADY_REPORTED, "duplicate")
        }
        crate::repository::DeliveryOutcome::Conflict => {
            state.metrics.record_webhook_conflict();
            (axum::http::StatusCode::CONFLICT, "delivery conflict")
        }
    };
    Ok(response)
}

pub fn payload_hash(body: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(body)))
}
