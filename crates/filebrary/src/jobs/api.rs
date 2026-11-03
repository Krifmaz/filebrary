//! HTTP endpoints for jobs, under `/api/jobs`.
//!
//! | Method and path               | What it does                                   |
//! |-------------------------------|------------------------------------------------|
//! | `GET /api/jobs`               | The caller's jobs, newest first (`?all=true` for administrators: everyone's; `?status=`, `?limit=`) |
//! | `POST /api/jobs`              | Submit `{"kind": "...", "params": {...}}`       |
//! | `GET /api/jobs/kinds`         | Job kinds this server offers, with availability |
//! | `GET /api/jobs/{id}`          | One job                                        |
//! | `DELETE /api/jobs/{id}`       | Remove a finished job from the list            |
//! | `POST /api/jobs/{id}/cancel`  | Cancel a queued or running job                 |
//! | `POST /api/jobs/{id}/retry`   | Queue a failed or cancelled job again          |
//!
//! Users only see their own jobs; administrators can see and act on all of
//! them, including system jobs.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Job;
use crate::{
    auth::{AuthUser, User},
    error::{AppError, AppResult},
    state::AppState,
};

/// The `/api/jobs` routes (to be merged into the `/api` router).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/jobs", get(list).post(submit))
        .route("/jobs/kinds", get(kinds))
        .route("/jobs/{id}", get(get_one).delete(delete))
        .route("/jobs/{id}/cancel", post(cancel))
        .route("/jobs/{id}/retry", post(retry))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default)]
    all: bool,
    status: Option<String>,
    limit: Option<i64>,
}

async fn list(
    State(state): State<AppState>,
    AuthUser { user, .. }: AuthUser,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Vec<Job>>> {
    if let Some(s) = &q.status
        && !["queued", "running", "succeeded", "failed", "cancelled"].contains(&s.as_str())
    {
        return Err(AppError::bad_request("unknown status"));
    }
    let owner = if q.all && user.is_admin {
        None
    } else {
        Some(user.id)
    };
    Ok(Json(
        state
            .jobs
            .list(&state, owner, q.status.as_deref(), q.limit.unwrap_or(200))
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitBody {
    kind: String,
    #[serde(default)]
    params: Value,
}

async fn submit(
    State(state): State<AppState>,
    AuthUser { user, .. }: AuthUser,
    Json(b): Json<SubmitBody>,
) -> AppResult<(StatusCode, Json<Job>)> {
    let job = state.jobs.submit(&state, &user, &b.kind, b.params).await?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn kinds(State(state): State<AppState>, AuthUser { user, .. }: AuthUser) -> Json<Vec<Value>> {
    let out = state
        .jobs
        .kinds()
        .into_iter()
        .filter(|(_, h)| user.is_admin || !h.admin_only())
        .map(|(kind, h)| {
            let avail = h.available(&state);
            json!({
                "kind": kind,
                "available": avail.is_ok(),
                "reason": avail.err(),
                "admin_only": h.admin_only(),
                "info": h.describe(),
            })
        })
        .collect();
    Json(out)
}

/// Loads a job the caller may see (their own, or any for administrators).
async fn visible(state: &AppState, user: &User, id: i64) -> AppResult<Job> {
    let job = state.jobs.get(state, id).await?;
    if user.is_admin || job.user_id == Some(user.id) {
        Ok(job)
    } else {
        Err(AppError::NotFound)
    }
}

async fn get_one(
    State(state): State<AppState>,
    AuthUser { user, .. }: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Job>> {
    Ok(Json(visible(&state, &user, id).await?))
}

async fn cancel(
    State(state): State<AppState>,
    AuthUser { user, .. }: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Job>> {
    visible(&state, &user, id).await?;
    Ok(Json(state.jobs.cancel(&state, id).await?))
}

async fn retry(
    State(state): State<AppState>,
    AuthUser { user, .. }: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Job>> {
    let job = visible(&state, &user, id).await?;
    if let Some(h) = state.jobs.handler(&job.kind) {
        h.available(&state).map_err(AppError::BadRequest)?;
    }
    Ok(Json(state.jobs.retry(&state, id).await?))
}

async fn delete(
    State(state): State<AppState>,
    AuthUser { user, .. }: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    visible(&state, &user, id).await?;
    state.jobs.delete(&state, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
