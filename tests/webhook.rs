use axum::{body::Body, http::Request};
use hmac::{Hmac, KeyInit, Mac};
use pitools::{web, webhook::WebhookState};
use secrecy::SecretString;
use sha2::Sha256;
use std::sync::Arc;
use tower::ServiceExt;

type HmacSha256 = Hmac<Sha256>;

fn signature(secret: &str, body: &[u8]) -> String {
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(secret.as_bytes()).expect("key");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

#[tokio::test]
async fn webhook_rejects_invalid_signature_before_persistence() {
    let state = WebhookState {
        webhook_secret: SecretString::from("secret"),
        repositories: None,
        queue: None,
        admin_bearer_token_hash: SecretString::from("invalid"),
        metrics: Arc::new(pitools::metrics::Metrics::default()),
    };
    let body = br#"{"action":"opened"}"#;
    let response = web::router_with_state(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/github/webhook")
                .header("x-hub-signature-256", "sha256=invalid")
                .header("x-github-delivery", "delivery-1")
                .header("x-github-event", "pull_request")
                .body(Body::from(body.as_slice()))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn webhook_verifies_signature_before_reporting_missing_persistence() {
    let state = WebhookState {
        webhook_secret: SecretString::from("secret"),
        repositories: None,
        queue: None,
        admin_bearer_token_hash: SecretString::from("invalid"),
        metrics: Arc::new(pitools::metrics::Metrics::default()),
    };
    let body = br#"{"action":"opened"}"#;
    let response = web::router_with_state(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/github/webhook")
                .header("x-hub-signature-256", signature("secret", body))
                .header("x-github-delivery", "delivery-1")
                .header("x-github-event", "pull_request")
                .body(Body::from(body.as_slice()))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), 503);
}
