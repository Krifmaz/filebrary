//! Operations: health checks, Prometheus metrics, request logging, the
//! OpenAPI document and API docs page, notification admin endpoints and the
//! glue that runs the optional FTP server.
//!
//! # Routes
//!
//! | Route                                   | What                                    |
//! |-----------------------------------------|-----------------------------------------|
//! | `GET /healthz`, `/health`, `/api/health`| Liveness: the process answers.          |
//! | `GET /readyz`                           | Readiness: database and sources respond.|
//! | `GET /metrics`                          | Prometheus text format.                 |
//! | `GET /api/openapi.json`                 | OpenAPI 3.1 description of the API.     |
//! | `GET /api/docs`                         | Built-in API reference page.            |
//! | `GET /api/admin/notifications`          | Configured notification targets.        |
//! | `POST /api/admin/notifications/{name}/test` | Send a test notification.           |
//!
//! # Events
//!
//! Code that does something worth notifying about publishes on
//! [`crate::AppState::events`]:
//!
//! ```ignore
//! state.events.publish(filebrary_events::Event::ShareOpened { .. });
//! ```
//!
//! Publishing never blocks or fails. Quota code should use
//! [`filebrary_events::quota_crossed`] with
//! `config.notifications.quota_warning_percent` so a user is warned once.

pub mod ftp;
pub mod health;
pub mod logging;
pub mod metrics;
pub mod notifications;
pub mod openapi;

use axum::{Router, routing::get};

use crate::state::AppState;

/// Routes served at the top level (outside `/api`).
pub fn root_routes() -> Router<AppState> {
    Router::new()
        .route("/readyz", get(health::ready))
        .route("/metrics", get(metrics::render))
}

/// Routes merged under `/api`.
pub fn api_routes() -> Router<AppState> {
    Router::new()
        .merge(openapi::routes())
        .merge(notifications::routes())
}
