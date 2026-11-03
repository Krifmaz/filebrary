//! How the WebDAV server learns who is calling and what they may do.
//!
//! The WebDAV crate does not know about Filebrary's user database. The server
//! crate implements [`DavAuth`] (checking HTTP Basic credentials or an API
//! key against the same users, rate limits and permissions as the web UI) and
//! returns a [`DavUser`] for each request.

use std::{net::SocketAddr, sync::Arc};

use async_trait::async_trait;
use axum::http::HeaderMap;
use filebrary_storage::{Storage, VPath};

/// One permission, matching the server's per-source permissions.
///
/// How WebDAV methods map onto them:
///
/// | Method | Needs |
/// |---|---|
/// | `PROPFIND`, `OPTIONS` | view |
/// | `GET`, `HEAD`, `COPY` (source side) | download |
/// | `PUT` (new file), `MKCOL`, `COPY`/`MOVE` (new destination) | upload |
/// | `PUT` (existing file), `PROPPATCH`, `MOVE` (source side), `LOCK` on an existing file | edit |
/// | `DELETE`, overwriting `COPY`/`MOVE` destinations | delete |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DavPerm {
    /// See names and metadata.
    View,
    /// Read file contents.
    Download,
    /// Create new files and folders.
    Upload,
    /// Change, rename or move existing entries.
    Edit,
    /// Delete entries.
    Delete,
}

/// What a request is about to do with a path, so [`DavAuth::authorize`] can
/// apply the right check (symlink handling, rules deeper in a tree, quotas).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Read or inspect the resource (a final symlink is followed).
    Read,
    /// Act on a whole tree: `COPY` source (`follow: true`), `DELETE` and the
    /// `MOVE` source (`follow: false`, since a final symlink is what moves).
    Tree {
        /// Whether a final symlink is followed.
        follow: bool,
    },
    /// Create or change content (`PUT`, `MKCOL`, `COPY`/`MOVE` destination,
    /// `LOCK` on an unmapped URL). `bytes` is the size about to be written
    /// when known, for quota checks.
    Write {
        /// Bytes about to be written, if known.
        bytes: Option<u64>,
    },
}

/// Why access was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// The source or path is not visible to this user (sent as 404 so its
    /// existence is not revealed).
    NotFound,
    /// Visible, but the permission is missing (403).
    Forbidden,
    /// The write would exceed a quota (507).
    InsufficientStorage,
}

/// Why authentication failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// No or wrong credentials (401 with a Basic challenge).
    Unauthorized,
    /// Too many failed attempts from this client; retry after this many
    /// seconds (429).
    RateLimited {
        /// Seconds until the client may try again.
        retry_after: u64,
    },
    /// The credentials were valid but the account may not use WebDAV (403).
    Forbidden,
    /// The user store failed (500).
    Internal,
}

/// An authenticated caller. It is `Any` so a [`DavAuth`] can downcast to its
/// own user type.
///
/// Paths passed in are *user-visible*: relative to the user's scope on the
/// source. Storage operations run on those paths against the confined backend
/// from [`DavAuth::storage`], so a symlink can never lead out of the scope.
/// Locks and dead properties are keyed by the *real* path (the scope joined
/// with the visible path), so users with different scopes on one source see
/// each other's locks.
pub trait DavUser: std::any::Any + Send + Sync {
    /// A stable identifier, used as the lock owner. Two requests by the same
    /// user may use each other's locks; other users may not.
    fn id(&self) -> String;

    /// The sources this user can see, in display order.
    fn sources(&self) -> Vec<String>;

    /// The folder of `source` this user is confined to (the root for an
    /// unconfined user), or `None` if the source is not visible.
    fn scope(&self, source: &str) -> Option<VPath>;

    /// Maps a real path back to the user-visible one, if it is in scope.
    fn visible(&self, source: &str, real: &VPath) -> Option<VPath> {
        real.strip_prefix(&self.scope(source)?)
    }

    /// The storage quota for this user on `source`, for the RFC 4331
    /// `quota-used-bytes` and `quota-available-bytes` properties.
    fn quota(&self, _source: &str) -> Option<crate::Quota> {
        None
    }
}

/// Authenticates requests and hands out storage backends.
#[async_trait]
pub trait DavAuth: Send + Sync + 'static {
    /// Authenticates one request from its headers (normally `Authorization:
    /// Basic`) and the client address, if known.
    ///
    /// Implementations must apply the same login rate limiting as the web
    /// login, and must not accept the web session cookie without CSRF
    /// protection: a WebDAV client never sends one, and a browser would send it
    /// on cross-site requests.
    async fn authenticate(
        &self,
        headers: &HeaderMap,
        client: Option<SocketAddr>,
    ) -> Result<Arc<dyn DavUser>, AuthError>;

    /// Checks that `user` may do `perm` on the user-visible `path`, for the
    /// kind of operation in `access`.
    ///
    /// Called before every storage operation. Implementations should check
    /// per-folder rules against the symlink-resolved path, so a link cannot
    /// reach a folder a rule denies, and apply quotas for [`Access::Write`].
    ///
    /// # Errors
    ///
    /// [`Denied::NotFound`] when the user cannot view the source or path,
    /// [`Denied::Forbidden`] when `perm` is missing,
    /// [`Denied::InsufficientStorage`] when a quota would be exceeded.
    async fn authorize(
        &self,
        user: &dyn DavUser,
        source: &str,
        perm: DavPerm,
        path: &VPath,
        access: Access,
    ) -> Result<(), Denied>;

    /// The backend for `source` as `user` sees it: rooted at their scope
    /// (with [`Storage::sub`]) so every path rule, symlinks included, applies
    /// to the scope. `None` if the source does not exist or is not visible.
    async fn storage(&self, user: &dyn DavUser, source: &str) -> Option<Arc<dyn Storage>>;

    /// Called after every successful change, for the activity log and event
    /// notifications. The default does nothing.
    async fn on_change(&self, _user: &dyn DavUser, _change: &crate::Change) {}

    /// Deletes what a `DELETE` request names. The default deletes it from
    /// `storage` (the user's confined backend) at the user-visible `path`.
    /// A server with a recycle bin moves it there instead and returns an id
    /// for it, which is passed on in [`crate::Change::Deleted`].
    ///
    /// # Errors
    ///
    /// Storage failures, reported to the client like any other.
    async fn remove(
        &self,
        _user: &dyn DavUser,
        _source: &str,
        path: &VPath,
        storage: &Arc<dyn Storage>,
    ) -> Result<Option<String>, filebrary_storage::StorageError> {
        storage.delete(path).await.map(|()| None)
    }

    /// True if the entry at the real path `real` must never be shown or
    /// touched through WebDAV (for example the server's trash folder). Hidden
    /// entries are left out of listings; requests for them should also be
    /// refused by [`DavAuth::authorize`]. The default hides nothing.
    fn hidden(&self, _source: &str, _real: &VPath) -> bool {
        false
    }
}

/// Credentials from an `Authorization: Basic` header.
#[derive(Clone, PartialEq, Eq)]
pub struct BasicCredentials {
    /// The user name.
    pub username: String,
    /// The password (or an API key used as the password).
    pub password: String,
}

impl std::fmt::Debug for BasicCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BasicCredentials")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl BasicCredentials {
    /// Parses `Authorization: Basic <base64(user:pass)>`. Returns `None` when
    /// the header is missing, uses another scheme, or is malformed.
    pub fn from_headers(headers: &HeaderMap) -> Option<Self> {
        use base64::Engine as _;
        let value = headers
            .get(axum::http::header::AUTHORIZATION)?
            .to_str()
            .ok()?;
        let (scheme, rest) = value.split_once(' ')?;
        if !scheme.eq_ignore_ascii_case("basic") {
            return None;
        }
        let raw = base64::engine::general_purpose::STANDARD
            .decode(rest.trim())
            .ok()?;
        let text = String::from_utf8(raw).ok()?;
        let (username, password) = text.split_once(':')?;
        if username.is_empty() {
            return None;
        }
        Some(Self {
            username: username.to_owned(),
            password: password.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn parses_basic() {
        let mut h = HeaderMap::new();
        // "bob:pa:ss" — the password may contain colons.
        h.insert(
            "authorization",
            HeaderValue::from_static("Basic Ym9iOnBhOnNz"),
        );
        let c = BasicCredentials::from_headers(&h).unwrap();
        assert_eq!(c.username, "bob");
        assert_eq!(c.password, "pa:ss");
        assert!(!format!("{c:?}").contains("pa:ss"));
        h.insert("authorization", HeaderValue::from_static("Bearer xyz"));
        assert!(BasicCredentials::from_headers(&h).is_none());
        h.insert("authorization", HeaderValue::from_static("Basic !!!"));
        assert!(BasicCredentials::from_headers(&h).is_none());
    }
}
