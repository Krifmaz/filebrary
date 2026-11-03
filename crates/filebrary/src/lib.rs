//! Filebrary: a small, fast, self-hosted file manager.
//!
//! The crate is a library plus a thin binary (`main.rs`) so that integration
//! tests can build the full application with [`app`] and drive it in-process.
//!
//! # Module layout
//!
//! * [`config`]: settings from TOML and `FILEBRARY_*` variables.
//! * [`storage`]: the [`storage::Storage`] trait, path validation and backends.
//! * [`db`]: SQLite pool and migrations (one `migrations/` dir for the workspace).
//! * [`state`]: [`state::AppState`], shared by all handlers.
//! * [`auth`]: users, permissions, sessions and extractors.
//! * [`identity`]: OIDC, LDAP, TOTP 2FA, API keys and quotas (on top of [`auth`]).
//! * [`api`]: JSON endpoints under `/api`.
//! * [`shares`]: share links.
//! * [`ops`]: health, metrics, logging, OpenAPI, notifications admin, FTP glue.
//! * [`organize`]: trash bin, tags and stars.
//! * [`activity`]: the activity log.
//! * [`comments`]: comments on files.
//! * [`collections`]: multi-item collections that can be shared.
//! * [`cast`]: cookie-free media URLs for Chromecast/AirPlay.
//! * [`branding`]: branding, themes and the PWA manifest.
//! * [`web`]: the embedded React UI.

#![warn(missing_docs)]

/// Formats an SQL string from *trusted constants only* (table names and SQL
/// fragments defined in this crate, never user input) and marks it as audited
/// for sqlx. Values always go through `.bind()`.
macro_rules! sql_fmt {
    ($($t:tt)*) => { sqlx::AssertSqlSafe(format!($($t)*)) };
}
pub(crate) use sql_fmt;

pub mod activity;
pub mod api;
pub mod auth;
pub mod branding;
pub mod cast;
pub mod collections;
pub mod comments;
pub mod config;
pub mod crypto;
pub mod db;
pub mod error;
pub mod identity;
pub mod ops;
pub mod organize;
pub mod shares;
pub mod state;
pub use filebrary_storage as storage;
pub mod web;

use axum::{
    Router,
    http::{HeaderName, HeaderValue, header},
    routing::get,
};
use tower_http::{catch_panic::CatchPanicLayer, set_header::SetResponseHeaderLayer};

pub use state::AppState;

/// Starts periodic housekeeping: expired sessions and stale uploads are removed
/// at start-up and then hourly.
pub fn spawn_maintenance(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            tick.tick().await;
            if let Err(e) = auth::session::purge_expired(&state).await {
                tracing::warn!(error = %e, "purging sessions failed");
            }
            match api::uploads::purge_stale(&state).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(count = n, "removed stale uploads"),
                Err(e) => tracing::warn!(error = %e, "purging uploads failed"),
            }
            organize::maintain(&state).await;
        }
    })
}

/// Builds the complete application router.
pub fn app(state: AppState) -> Router {
    let router = Router::new()
        .nest("/api", api::router().merge(ops::api_routes()))
        .route("/healthz", get(api::health))
        .route("/health", get(api::health))
        .merge(ops::root_routes())
        .route("/manifest.webmanifest", get(branding::manifest))
        .fallback(web::serve)
        .layer(axum::middleware::from_fn(ops::metrics::track))
        .with_state(state)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static(
                "default-src 'self'; img-src 'self' data: blob:; media-src 'self' blob:; \
                 style-src 'self' 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
            ),
        ))
        .layer(CatchPanicLayer::new());
    ops::logging::layer(router)
}
