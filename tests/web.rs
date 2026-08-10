use axum::{body::Body, http::Request};
use pitools::{metrics::Metrics, state::AppState, web};
use secrecy::SecretString;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn health_endpoint_is_available() {
    let response = web::router()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn webhook_is_not_mutating_before_verification_is_implemented() {
    let response = web::router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/github/webhook")
                .body(Body::from("{}"))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), 501);
}

#[tokio::test]
async fn manifest_callback_rejects_missing_or_unsafe_conversion_codes() {
    for uri in [
        "/github/manifest/callback",
        "/github/manifest/callback?code=../conversion",
    ] {
        let response = web::router()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert!(matches!(response.status().as_u16(), 400 | 502));
    }
}

#[tokio::test]
async fn metrics_endpoint_returns_prometheus_text() {
    let metrics = Arc::new(Metrics::default());
    metrics.record_webhook_received();
    let state = AppState {
        webhook_secret: SecretString::from("secret"),
        repositories: None,
        queue: None,
        admin_bearer_token_hash: SecretString::from("invalid"),
        metrics,
    };
    let response = web::router_with_state(state)
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), 200);
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("body");
    assert!(String::from_utf8_lossy(&body).contains("pitools_webhook_received_total 1"));
}
