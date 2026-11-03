//! HTTP API: every JSON endpoint lives under `/api`.
//!
//! # Route map (contract between work streams)
//!
//! | Prefix                         | Module                    | Owner   |
//! |--------------------------------|---------------------------|---------|
//! | `/api/health`                  | here                      | core    |
//! | `/api/sources`, `/api/sources/{id}/...` | [`files`]        | files   |
//! | `/api/sources/{id}/uploads`, `/api/uploads/{uid}` | [`uploads`] | files |
//! | `/api/auth/...`, `/api/users`, `/api/groups` | [`crate::auth::routes`] | auth |
//! | `/api/auth/methods`, `/api/auth/oidc/...`, `/api/me/totp...`, `/api/me/api-keys...`, `/api/me/quota`, `/api/users/{id}/{totp,api-keys,quota}`, `/api/api-keys/{id}` | [`crate::identity::routes`] | identity |
//! | `/api/shares`                  | [`crate::shares::api_router`]    | shares  |
//! | `/api/public/shares/{token}`   | [`crate::shares::public_router`] | shares  |
//! | `/api/trash`, `/api/tags`, `/api/stars`, `/api/annotations` | [`crate::organize::api`] | organize |
//! | `/api/activity`                | [`crate::activity`]       | organize |
//! | `/api/comments`                | [`crate::comments`]       | social  |
//! | `/api/collections`             | [`crate::collections`]    | social  |
//! | `/api/cast`, `/api/public/cast/{token}/{name}` | [`crate::cast`] | social |
//! | `/api/branding`, `/api/admin/branding`, `/api/me/theme` | [`crate::branding`] | social |
//!
//! Errors are JSON: `{"error": "<machine code>", "message": "<human text>"}`.
//! Paths are passed as `path` query parameters (or JSON fields), absolute
//! within the source and `/`-separated; they are validated by
//! [`crate::storage::VPath::parse`].

pub mod download;
pub mod files;
pub mod uploads;
pub mod zip;

use axum::{Json, Router, routing::get};
use serde_json::json;

use crate::state::AppState;

/// Builds the `/api` router (without the `/api` prefix).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .merge(files::routes())
        .merge(uploads::routes())
        .merge(crate::auth::routes::routes())
        .merge(crate::identity::routes::routes())
        .nest("/shares", crate::shares::api_router())
        .nest("/public/shares", crate::shares::public_router())
        .merge(crate::organize::api::routes())
        .nest("/activity", crate::activity::routes())
        .nest("/comments", crate::comments::router())
        .nest("/collections", crate::collections::router())
        .merge(crate::cast::router())
        .nest("/public/cast", crate::cast::public_router())
        .merge(crate::branding::router())
}

/// `GET /api/health`: liveness probe, also served at `/healthz`.
pub async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}
