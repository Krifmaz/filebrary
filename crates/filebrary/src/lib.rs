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
//! * [`api`]: JSON endpoints under `/api`.
//! * [`dav`]: WebDAV at `/dav`, on the `filebrary-webdav` crate.
//! * [`shares`]: share links.
//! * [`web`]: the embedded React UI.

#![warn(missing_docs)]

pub mod api;
pub mod auth;
pub mod config;
pub mod crypto;
pub mod dav;
pub mod db;
pub mod error;
pub mod shares;
pub mod state;
pub use filebrary_storage as storage;
pub mod web;

use axum::{
    Router,
    http::{HeaderName, HeaderValue, header},
    routing::get,
};
use tower_http::{
    catch_panic::CatchPanicLayer, set_header::SetResponseHeaderLayer, trace::TraceLayer,
};

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
        }
    })
}

/// Builds the complete application router.
pub fn app(state: AppState) -> Router {
    let mut router = Router::new();
    if state.config.webdav {
        router = router.merge(dav::router(&state));
    }
    router
        .nest("/api", api::router())
        .route("/healthz", get(api::health))
        .route("/health", get(api::health))
        .fallback(web::serve)
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
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
}
