use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    admin::is_authorized,
    queue::{JobKind, JobSpec},
    state::AppState,
};

pub fn routes() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/api/v1/watchlist", axum::routing::get(list_watchlist))
        .route("/api/v1/reconcile", axum::routing::post(enqueue_reconcile))
        .route(
            "/api/v1/jobs/{job_id}/cancel",
            axum::routing::post(cancel_job),
        )
}

#[derive(Debug, Serialize)]
pub struct WatchlistResponse {
    pub pull_requests: Vec<crate::repository::PullRequestRow>,
}

async fn list_watchlist(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<WatchlistResponse>, ApiError> {
    require_admin(&headers, &state)?;
    let repositories = state
        .repositories
        .ok_or(ApiError::Unavailable("database"))?;
    let pull_requests = repositories
        .open_pull_requests()
        .await
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    Ok(Json(WatchlistResponse { pull_requests }))
}

#[derive(Debug, Deserialize)]
pub struct ReconcileRequest {
    pub repository_id: i64,
    pub pull_request_number: i32,
}

#[derive(Debug, Serialize)]
pub struct JobResponse {
    pub job_id: Uuid,
}

async fn enqueue_reconcile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ReconcileRequest>,
) -> Result<(StatusCode, Json<JobResponse>), ApiError> {
    require_admin(&headers, &state)?;
    let queue = state.queue.ok_or(ApiError::Unavailable("queue"))?;
    let job_id = queue
        .enqueue(JobSpec {
            repository_id: request.repository_id,
            pull_request_number: request.pull_request_number,
            kind: JobKind::Reconcile,
            plan: serde_json::json!({"source": "operator"}),
        })
        .await
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    Ok((StatusCode::ACCEPTED, Json(JobResponse { job_id })))
}

async fn cancel_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&headers, &state)?;
    let queue = state.queue.ok_or(ApiError::Unavailable("queue"))?;
    let changed = queue
        .request_cancel(job_id)
        .await
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    Ok(Json(
        serde_json::json!({"job_id": job_id, "cancel_requested": changed}),
    ))
}

fn require_admin(headers: &HeaderMap, state: &AppState) -> Result<(), ApiError> {
    if is_authorized(headers, &state.admin_bearer_token_hash) {
        Ok(())
    } else {
        Err(ApiError::Unauthorized)
    }
}

#[derive(Debug)]
pub enum ApiError {
    Unauthorized,
    Unavailable(&'static str),
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".into()),
            Self::Unavailable(component) => (StatusCode::SERVICE_UNAVAILABLE, component.into()),
            Self::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        (status, Json(serde_json::json!({"error": message}))).into_response()
    }
}
