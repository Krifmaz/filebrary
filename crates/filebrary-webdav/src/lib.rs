//! A WebDAV server (RFC 4918, classes 1 and 2) for Filebrary.
//!
//! It serves every storage source through the [`filebrary_storage::Storage`]
//! trait, so WebDAV clients see the same files, path checks and backends as
//! the web UI, and it enforces the same users and per-source permissions
//! through [`DavAuth`].
//!
//! # URL layout
//!
//! ```text
//! /dav/                 the sources this user can see, as collections
//! /dav/<source>/        the root of a source (the user's scope on it)
//! /dav/<source>/a/b.txt a file
//! ```
//!
//! # What is supported
//!
//! * `OPTIONS`, `GET`/`HEAD` (with single byte ranges, `If-None-Match`,
//!   `If-Modified-Since`, `If-Range`), `PUT` (streamed, chunked uploads work),
//!   `DELETE`, `MKCOL`, `COPY` and `MOVE` (with `Depth`, `Overwrite`, and
//!   across sources), `PROPFIND` (depth 0 and 1), `PROPPATCH` (dead properties).
//! * Class 2: exclusive and shared write locks with depth 0 or infinity,
//!   refresh, timeouts, the full `If` header, and locking unmapped URLs.
//! * RFC 4331 quota properties when [`DavUser::quota`] reports a quota.
//! * Client quirks: `MS-Author-Via` for Windows Explorer, unauthenticated
//!   `OPTIONS`, chunked `PUT` from macOS Finder, `X-Expected-Entity-Length`.
//!
//! `PROPFIND` with `Depth: infinity` is refused with `propfind-finite-depth`,
//! as RFC 4918 allows, so one request cannot walk a whole tree.
//!
//! # Example
//!
//! ```no_run
//! # use std::sync::Arc;
//! # fn demo(auth: Arc<dyn filebrary_webdav::DavAuth>) {
//! use filebrary_webdav::{DavConfig, DavServer};
//! let dav = DavServer::new(auth, DavConfig::default());
//! let app: axum::Router = axum::Router::new().merge(dav.router());
//! # }
//! ```

#![warn(missing_docs)]

pub mod auth;
pub mod ifheader;
pub mod lock;
pub mod props;
mod server;
pub mod target;
pub mod xml;

use std::{convert::Infallible, sync::Arc};

use axum::{Router, body::Body, http::Request, response::Response};
use filebrary_storage::VPath;

pub use auth::{AuthError, BasicCredentials, DavAuth, DavPerm, DavUser, Denied};
pub use props::{MemoryPropStore, PropStore};

/// Settings for a WebDAV mount.
#[derive(Debug, Clone)]
pub struct DavConfig {
    /// The URL path the server is mounted at, without a trailing slash.
    pub prefix: String,
    /// The realm sent in the Basic authentication challenge.
    pub realm: String,
}

impl Default for DavConfig {
    fn default() -> Self {
        Self {
            prefix: "/dav".into(),
            realm: "Filebrary".into(),
        }
    }
}

/// Free and used space, for the RFC 4331 quota properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quota {
    /// Bytes used.
    pub used: u64,
    /// Bytes still available, or `None` if unlimited.
    pub available: Option<u64>,
}

/// A change made through WebDAV, reported to [`DavAuth::on_change`] for the
/// activity log and notifications. Paths are real paths inside the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A file was written (`PUT`, or `LOCK` on an unmapped URL).
    Written {
        /// The source.
        source: String,
        /// The file.
        path: VPath,
        /// True if the file did not exist before.
        created: bool,
    },
    /// A collection was created (`MKCOL`).
    CreatedCollection {
        /// The source.
        source: String,
        /// The new directory.
        path: VPath,
    },
    /// A file or collection was deleted.
    Deleted {
        /// The source.
        source: String,
        /// What was deleted.
        path: VPath,
    },
    /// A file or collection was moved or renamed.
    Moved {
        /// The source it came from.
        source: String,
        /// The old path.
        from: VPath,
        /// The source it went to.
        to_source: String,
        /// The new path.
        to: VPath,
    },
    /// A file or collection was copied.
    Copied {
        /// The source it came from.
        source: String,
        /// The original.
        from: VPath,
        /// The source of the copy.
        to_source: String,
        /// The copy.
        to: VPath,
    },
    /// Dead properties were changed (`PROPPATCH`).
    Properties {
        /// The source.
        source: String,
        /// The resource.
        path: VPath,
    },
}

/// A WebDAV mount. Cheap to clone.
#[derive(Clone)]
pub struct DavServer {
    inner: Arc<server::Inner>,
}

impl DavServer {
    /// A server using `auth` for users and storage, with dead properties kept
    /// in memory.
    pub fn new(auth: Arc<dyn DavAuth>, config: DavConfig) -> Self {
        Self::with_props(auth, config, Arc::new(MemoryPropStore::default()))
    }

    /// A server with a custom dead-property store.
    pub fn with_props(
        auth: Arc<dyn DavAuth>,
        mut config: DavConfig,
        props: Arc<dyn PropStore>,
    ) -> Self {
        let trimmed = config.prefix.trim_end_matches('/');
        config.prefix = if trimmed.starts_with('/') {
            trimmed.to_owned()
        } else {
            format!("/{trimmed}")
        };
        Self {
            inner: Arc::new(server::Inner {
                auth,
                props,
                locks: lock::LockManager::new(),
                config,
            }),
        }
    }

    /// The mount prefix, normalised (leading slash, no trailing slash).
    pub fn prefix(&self) -> &str {
        &self.inner.config.prefix
    }

    /// Handles one request. The request path must start with the prefix.
    pub async fn handle(&self, req: Request<Body>) -> Response {
        self.inner.handle(req).await
    }

    /// A router that serves the mount prefix and everything below it, for
    /// every HTTP method (including the WebDAV extension methods).
    pub fn router<S: Clone + Send + Sync + 'static>(&self) -> Router<S> {
        let me = self.clone();
        let svc = tower::service_fn(move |req: Request<Body>| {
            let me = me.clone();
            async move { Ok::<_, Infallible>(me.handle(req).await) }
        });
        let prefix = self.prefix().to_owned();
        let mut r = Router::new()
            .route_service(&format!("{prefix}/"), svc.clone())
            .route_service(&format!("{prefix}/{{*rest}}"), svc.clone());
        if !prefix.is_empty() {
            r = r.route_service(&prefix, svc);
        }
        r
    }
}
