//! Share links: public links to files, folders and sets of items.
//!
//! A share is identified by an unguessable token and can carry a password
//! (argon2id), an expiry time, a download limit, or be an *upload-only drop
//! link* where visitors can add files to a folder but never see its contents.
//!
//! * [`api_router`] is nested at `/api/shares`: logged-in users create, list,
//!   update and delete their own links (administrators see all of them).
//! * [`public_router`] is nested at `/api/public/shares`: anonymous access by
//!   token. The web UI renders the public page at `/s/{token}`.
//!
//! # Security model
//!
//! * A share never grants more than its creator holds. Creating a share needs
//!   the `share` permission plus `download` (or `upload` for a drop link) on
//!   every path, and the same rights are re-checked against the creator's
//!   *current* grants on every public request: deleting or disabling the user,
//!   or narrowing their permissions or scope, kills their links at once.
//! * Unknown, expired and revoked tokens all return the same 404.
//! * Visitors only ever send share-relative paths, which are validated with
//!   [`VPath::parse`](crate::storage::VPath::parse) and joined below the shared
//!   item, so they cannot climb out of it; the storage backend then applies its
//!   own root and symlink checks.
//! * A correct password yields an access grant (random, stored hashed, valid
//!   for [`store::ACCESS_TTL_SECS`]) returned in an `HttpOnly` cookie and in
//!   the body. Changing or removing the password revokes all grants. Wrong
//!   passwords are rate limited per share.
//! * Drop-link uploads never overwrite: a name that is taken gets a numbered
//!   suffix, so visitors learn nothing about existing files.
//!
//! Tables live in `migrations/0020_shares.sql`.

pub mod api;
pub mod error;
pub mod model;
pub mod public;
mod ratelimit;
pub mod store;

use axum::Router;

use crate::state::AppState;

pub use error::{ShareError, ShareResult};
pub use store::{Share, ShareStore};

/// Authenticated share management, nested at `/api/shares`.
pub fn api_router() -> Router<AppState> {
    api::router()
}

/// Anonymous share access, nested at `/api/public/shares`.
pub fn public_router() -> Router<AppState> {
    public::router()
}
