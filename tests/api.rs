use std::sync::Arc;

use axum::{body::Body, http::Request};
use pitools::{metrics::Metrics, state::AppState, web};
use secrecy::SecretString;
use tower::ServiceExt;

fn state() -> AppState {
    AppState {
        webhook_secret: SecretString::from("secret"),
        repositories: None,
        queue: None,
        nats: None,
        admin_bearer_token_hash: SecretString::from("invalid"),
        metrics: Arc::new(Metrics::default()),
    }
}

#[tokio::test]
async fn operator_read_routes_require_admin_authorization() {
    for uri in ["/api/v1/events", "/api/v1/audit", "/api/v1/watchlist/42/7"] {
        let response = web::router_with_state(state())
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), 401, "route {uri} must be protected");
    }
}
