//! Authentication endpoints. Owned by the Auth work stream.
//!
//! Planned routes (all under `/api`):
//!
//! * `POST /auth/login` `{username, password}` → sets the session cookie,
//!   returns `{user, csrf_token}`; rate limited per client address.
//! * `POST /auth/logout`
//! * `GET  /me` → `{user, csrf_token}`
//! * `POST /me/password` `{current, new}`
//! * `GET|POST /users`, `PUT|DELETE /users/{id}` (administrators)

use axum::Router;

use crate::state::AppState;

/// Routes for this module, merged under `/api`.
pub fn routes() -> Router<AppState> {
    Router::new()
}
