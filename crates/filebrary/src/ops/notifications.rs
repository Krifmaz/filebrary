//! Admin endpoints for notification targets.
//!
//! Targets are configured in the configuration file only (never through the
//! API), so these endpoints only list them and send test notifications.
//! Secrets (tokens, passwords, webhook secrets, extra headers) are never
//! returned.

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use filebrary_events::EventKind;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    auth::AdminUser,
    error::{AppError, AppResult},
    state::AppState,
};

/// Routes merged under `/api`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/notifications", get(list))
        .route("/admin/notifications/{name}/test", post(test))
}

/// One configured target.
#[derive(Debug, Serialize, ToSchema)]
pub struct TargetInfo {
    /// The target's configured name.
    pub name: String,
    /// `webhook`, `ntfy` or `email`.
    pub kind: &'static str,
    /// Events the target receives; empty means all.
    #[schema(value_type = Vec<String>)]
    pub events: Vec<EventKind>,
}

/// Response of `GET /api/admin/notifications`.
#[derive(Debug, Serialize, ToSchema)]
pub struct TargetList {
    /// Configured targets.
    pub targets: Vec<TargetInfo>,
    /// Quota percentage at which `quota_near_full` is sent.
    pub quota_warning_percent: u8,
}

/// Lists the configured notification targets (administrators only).
#[utoipa::path(
    get,
    path = "/api/admin/notifications",
    tag = "notifications",
    responses(
        (status = 200, body = TargetList),
        (status = 401, body = crate::ops::openapi::ErrorBody),
        (status = 403, body = crate::ops::openapi::ErrorBody),
    )
)]
pub async fn list(State(state): State<AppState>, _admin: AdminUser) -> Json<TargetList> {
    let targets = state
        .notifier
        .as_ref()
        .map(|n| {
            n.sinks()
                .iter()
                .map(|s| TargetInfo {
                    name: s.name().to_owned(),
                    kind: s.kind(),
                    events: s.filter().clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    Json(TargetList {
        targets,
        quota_warning_percent: state.config.notifications.quota_warning_percent,
    })
}

/// Body of a test request.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TestBody {
    /// Text of the test notification (at most 500 characters).
    pub message: Option<String>,
}

/// Outcome of a test notification.
#[derive(Debug, Serialize, ToSchema)]
pub struct TestResult {
    /// Whether the target accepted the notification.
    pub delivered: bool,
    /// What went wrong, when it was not delivered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Sends a test notification to one target right away (administrators
/// only). One attempt, no retries; the outcome is returned.
#[utoipa::path(
    post,
    path = "/api/admin/notifications/{name}/test",
    tag = "notifications",
    params(("name" = String, Path, description = "Target name")),
    request_body(content = TestBody, description = "Optional message"),
    responses(
        (status = 200, body = TestResult),
        (status = 404, description = "No such target", body = crate::ops::openapi::ErrorBody),
    )
)]
pub async fn test(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(name): Path<String>,
    body: Option<Json<TestBody>>,
) -> AppResult<Json<TestResult>> {
    let notifier = state.notifier.as_ref().ok_or(AppError::NotFound)?;
    if !notifier.sinks().iter().any(|s| s.name() == name) {
        return Err(AppError::NotFound);
    }
    let message = body.and_then(|Json(b)| b.message).unwrap_or_else(|| {
        format!(
            "Test notification sent by {} from the admin panel.",
            admin.0.user.username
        )
    });
    if message.chars().count() > 500 {
        return Err(AppError::bad_request(
            "message is longer than 500 characters",
        ));
    }
    tracing::info!(target_name = %name, admin = %admin.0.user.username, "sending test notification");
    Ok(Json(match notifier.send_test(&name, message).await {
        Ok(()) => TestResult {
            delivered: true,
            error: None,
        },
        Err(e) => TestResult {
            delivered: false,
            error: Some(e),
        },
    }))
}
