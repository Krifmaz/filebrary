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
//! * [`shares`]: share links.
//! * [`web`]: the embedded React UI.

#![warn(missing_docs)]

pub mod api;
pub mod auth;
pub mod config;
pub mod crypto;
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

/// Builds the complete application router.
pub fn app(state: AppState) -> Router {
    Router::new()
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
