use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderValue, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use serde::Deserialize;

use crate::{api, github::manifest, state::AppState, webhook::receive};

pub fn router() -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .route("/github/webhook", axum::routing::post(webhook_placeholder))
        .route("/github/manifest/callback", get(manifest_callback))
}

pub fn router_with_state(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(ready_with_state))
        .route("/github/webhook", axum::routing::post(receive))
        .route("/github/manifest/callback", get(manifest_callback))
        .route("/metrics", get(metrics))
        .merge(api::routes())
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct ManifestCallbackQuery {
    code: Option<String>,
}

async fn manifest_callback(
    Query(query): Query<ManifestCallbackQuery>,
) -> Result<impl IntoResponse, ManifestCallbackError> {
    let code = query.code.ok_or(ManifestCallbackError::MissingCode)?;
    let conversion = manifest::exchange_manifest_code(&code)
        .await
        .map_err(ManifestCallbackError::Conversion)?;
    Ok((
        [
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
        ],
        axum::Json(conversion),
    ))
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn ready() -> impl IntoResponse {
    (StatusCode::OK, "ready")
}

async fn ready_with_state(State(state): State<AppState>) -> impl IntoResponse {
    let Some(repositories) = state.repositories.as_ref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    match repositories.ping().await {
        Ok(()) => (StatusCode::OK, "ready"),
        Err(error) => {
            tracing::warn!(error = %error, "readiness database check failed");
            (StatusCode::SERVICE_UNAVAILABLE, "database unavailable")
        }
    }
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
        )],
        state.metrics.render_prometheus(),
    )
}

async fn webhook_placeholder() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "webhook intake is not configured",
    )
}

#[derive(Debug)]
enum ManifestCallbackError {
    MissingCode,
    Conversion(manifest::ManifestError),
}

impl IntoResponse for ManifestCallbackError {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::MissingCode => {
                (StatusCode::BAD_REQUEST, "missing manifest conversion code").into_response()
            }
            Self::Conversion(error) => (StatusCode::BAD_GATEWAY, error.to_string()).into_response(),
        }
    }
}
