//! Share links. Owned by the Shares work stream.
//!
//! * [`api_router`] is nested at `/api/shares` (managing one's own links).
//! * [`public_router`] is nested at `/api/public/shares` (anonymous access by
//!   token). The web UI serves the public page at `/s/{token}`.
//!
//! Tables go in `migrations/0020_shares.sql`.

use axum::Router;

use crate::state::AppState;

/// Authenticated share management, nested at `/api/shares`.
pub fn api_router() -> Router<AppState> {
    Router::new()
}

/// Anonymous share access, nested at `/api/public/shares`.
pub fn public_router() -> Router<AppState> {
    Router::new()
}
