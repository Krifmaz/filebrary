//! The request handler: authentication, dispatch and every WebDAV method.

// Handlers return `Result<Resp, Resp>` so `?` can short-circuit with a ready
// response. A response is built once per request, so its size does not matter.
#![allow(clippy::result_large_err)]

use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    ops::Range,
    sync::Arc,
    time::{Duration, SystemTime},
};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, header},
};
use filebrary_storage::{Entry, Storage, StorageError, VPath};
use futures::{StreamExt, TryStreamExt};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    Change, DavConfig,
    auth::{AuthError, DavAuth, DavPerm, DavUser, Denied},
    ifheader::{IfHeader, list_matches},
    lock::{Lock, LockError, LockManager, Scope, parse_timeout},
    props::{PropError, PropStore},
    target::{Target, TargetError, href, mount_href, parse_target, split_uri},
    xml::{self, DAV, PatchOp, PropName, PropfindRequest, XmlWriter, error_body, escape_text},
};

/// Methods this server implements, for `Allow` and `OPTIONS`.
pub const ALLOW: &str =
    "OPTIONS, GET, HEAD, PUT, DELETE, PROPFIND, PROPPATCH, MKCOL, COPY, MOVE, LOCK, UNLOCK";

/// Live properties returned for `allprop`, in output order.
const ALLPROP: [&str; 8] = [
    "resourcetype",
    "displayname",
    "getcontentlength",
    "getcontenttype",
    "getetag",
    "getlastmodified",
    "supportedlock",
    "lockdiscovery",
];

/// Live properties a client may not change with PROPPATCH.
const PROTECTED: [&str; 11] = [
    "resourcetype",
    "displayname",
    "getcontentlength",
    "getcontenttype",
    "getetag",
    "getlastmodified",
    "creationdate",
    "supportedlock",
    "lockdiscovery",
    "quota-used-bytes",
    "quota-available-bytes",
];

type Resp = Response<Body>;

/// Shared state of one WebDAV mount.
pub(crate) struct Inner {
    pub(crate) auth: Arc<dyn DavAuth>,
    pub(crate) props: Arc<dyn PropStore>,
    pub(crate) locks: LockManager,
    pub(crate) config: DavConfig,
}

// ---------------------------------------------------------------- responses

fn status(code: StatusCode) -> Resp {
    let mut r = Response::new(Body::empty());
    *r.status_mut() = code;
    r
}

fn xml_response(code: StatusCode, body: String) -> Resp {
    let mut r = Response::new(Body::from(body));
    *r.status_mut() = code;
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    r
}

fn precondition(code: StatusCode, condition: &str, hrefs: &[String]) -> Resp {
    xml_response(code, error_body(condition, hrefs))
}

fn storage_status(e: &StorageError) -> StatusCode {
    match e {
        StorageError::NotFound => StatusCode::NOT_FOUND,
        StorageError::AlreadyExists => StatusCode::PRECONDITION_FAILED,
        StorageError::InvalidPath(_) => StatusCode::BAD_REQUEST,
        StorageError::OutsideRoot => StatusCode::FORBIDDEN,
        StorageError::NotADirectory => StatusCode::CONFLICT,
        StorageError::IsADirectory => StatusCode::METHOD_NOT_ALLOWED,
        StorageError::InvalidRange { .. } => StatusCode::RANGE_NOT_SATISFIABLE,
        StorageError::Unsupported(_) => StatusCode::FORBIDDEN,
        StorageError::Io(io) => match io.kind() {
            io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
                StatusCode::FORBIDDEN
            }
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => {
                StatusCode::INSUFFICIENT_STORAGE
            }
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        },
    }
}

fn storage_error(e: StorageError) -> Resp {
    let code = storage_status(&e);
    if code == StatusCode::INTERNAL_SERVER_ERROR {
        tracing::error!(error = %e, "webdav storage error");
    } else {
        tracing::debug!(error = %e, "webdav storage error");
    }
    status(code)
}

fn denied(d: Denied) -> Resp {
    match d {
        Denied::NotFound => status(StatusCode::NOT_FOUND),
        Denied::Forbidden => status(StatusCode::FORBIDDEN),
    }
}

fn http_date(t: SystemTime) -> String {
    httpdate::fmt_http_date(t)
}

fn rfc3339(t: SystemTime) -> String {
    OffsetDateTime::from(t)
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

fn system_time(e: &Entry) -> Option<SystemTime> {
    e.modified.map(SystemTime::from)
}

/// The entity tag of an entry: derived from size and modification time, the
/// same way the local backend reports changes.
pub(crate) fn etag(e: &Entry) -> String {
    let nanos = e
        .modified
        .map(|m| m.unix_timestamp_nanos())
        .unwrap_or(0)
        .max(0);
    if e.is_dir() {
        format!("\"d{nanos:x}\"")
    } else {
        format!("\"{:x}-{nanos:x}\"", e.size)
    }
}

fn content_type(e: &Entry) -> String {
    e.mime
        .clone()
        .unwrap_or_else(|| filebrary_storage::guess_mime(&e.name))
}

// ------------------------------------------------------------ request state

/// Everything a method needs about the current request.
struct Ctx<'a> {
    srv: &'a Inner,
    user: Arc<dyn DavUser>,
    headers: HeaderMap,
    if_header: Option<IfHeader>,
    /// Lock tokens submitted in `If` (and `Lock-Token` for UNLOCK).
    tokens: Vec<String>,
    /// Confined backends already opened for this request, by source.
    backends: std::sync::Mutex<HashMap<String, Arc<dyn Storage>>>,
}

/// A target inside a source, resolved for one permission.
struct Resolved {
    source: String,
    /// The user-visible path, for storage calls on the confined backend.
    path: VPath,
    /// The real path in the source, for locks and dead properties.
    key: VPath,
    storage: Arc<dyn Storage>,
}

impl Ctx<'_> {
    fn header(&self, name: impl header::AsHeaderName) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    fn principal(&self) -> String {
        self.user.id()
    }

    /// Checks `perm` on a user-visible path and returns the confined backend
    /// with both forms of the path. Backends are cached per request, so the
    /// scope is resolved once.
    async fn resolve(
        &self,
        source: &str,
        visible: &VPath,
        perm: DavPerm,
    ) -> Result<Resolved, Resp> {
        self.user.check(source, perm, visible).map_err(denied)?;
        let scope = self
            .user
            .scope(source)
            .ok_or_else(|| status(StatusCode::NOT_FOUND))?;
        let cached = self
            .backends
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(source)
            .cloned();
        let storage = match cached {
            Some(s) => s,
            None => {
                let s = self
                    .srv
                    .auth
                    .storage(self.user.as_ref(), source)
                    .await
                    .ok_or_else(|| status(StatusCode::NOT_FOUND))?;
                self.backends
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(source.to_owned(), s.clone());
                s
            }
        };
        Ok(Resolved {
            source: source.to_owned(),
            path: visible.clone(),
            key: scope.join_path(visible),
            storage,
        })
    }

    fn href(&self, source: &str, visible: &VPath, is_dir: bool) -> String {
        href(&self.srv.config.prefix, source, visible, is_dir)
    }

    /// The href a user sees for a lock root, if it is inside their scope.
    fn lock_href(&self, lock: &Lock, is_dir: bool) -> Option<String> {
        self.user
            .visible(&lock.source, &lock.root)
            .map(|v| self.href(&lock.source, &v, is_dir))
    }

    /// Checks locks for a change and turns a conflict into a 423 response.
    fn check_lock(&self, source: &str, real: &VPath, scope: Scope) -> Result<(), Resp> {
        match self
            .srv
            .locks
            .check(source, real, scope, &self.tokens, &self.principal())
        {
            Ok(()) => Ok(()),
            Err(LockError::Conflict(roots)) => {
                let hrefs: Vec<String> = roots
                    .iter()
                    .filter_map(|r| self.user.visible(source, r))
                    .map(|v| self.href(source, &v, false))
                    .collect();
                Err(precondition(
                    StatusCode::LOCKED,
                    "lock-token-submitted",
                    &hrefs,
                ))
            }
            Err(_) => Err(status(StatusCode::LOCKED)),
        }
    }

    /// Lock checks for adding or removing `real` from its parent collection.
    fn check_membership(&self, source: &str, real: &VPath) -> Result<(), Resp> {
        match real.parent() {
            Some(parent) => self.check_lock(source, &parent, Scope::Resource),
            None => Ok(()),
        }
    }

    fn notify(&self, change: Change) {
        self.srv.auth.on_change(self.user.as_ref(), &change);
    }
}

async fn stat_opt(r: &Resolved) -> Result<Option<Entry>, Resp> {
    match r.storage.stat(&r.path).await {
        Ok(e) => Ok(Some(e)),
        Err(StorageError::NotFound) => Ok(None),
        Err(e) => Err(storage_error(e)),
    }
}

/// Checks that the parent of `r` exists and is a collection (409 otherwise).
async fn require_parent(r: &Resolved) -> Result<(), Resp> {
    let Some(parent) = r.path.parent() else {
        return Err(status(StatusCode::CONFLICT));
    };
    match r.storage.stat(&parent).await {
        Ok(e) if e.is_dir() => Ok(()),
        Ok(_) | Err(StorageError::NotFound | StorageError::NotADirectory) => {
            Err(status(StatusCode::CONFLICT))
        }
        Err(e) => Err(storage_error(e)),
    }
}

async fn read_xml_body(body: Body) -> Result<Vec<u8>, Resp> {
    let mut out = Vec::new();
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| status(StatusCode::BAD_REQUEST))?;
        if out.len() + chunk.len() > xml::MAX_BODY {
            return Err(status(StatusCode::PAYLOAD_TOO_LARGE));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

fn body_reader(body: Body) -> filebrary_storage::ByteReader {
    let stream = body.into_data_stream().map_err(io::Error::other);
    Box::pin(tokio_util::io::StreamReader::new(stream))
}

enum Depth {
    Zero,
    One,
    Infinity,
}

fn depth(headers: &HeaderMap) -> Result<Option<Depth>, Resp> {
    match headers.get("depth").map(|v| v.to_str().unwrap_or("")) {
        None => Ok(None),
        Some("0") => Ok(Some(Depth::Zero)),
        Some("1") => Ok(Some(Depth::One)),
        Some(v) if v.eq_ignore_ascii_case("infinity") => Ok(Some(Depth::Infinity)),
        Some(_) => Err(status(StatusCode::BAD_REQUEST)),
    }
}

// ------------------------------------------------------------------ dispatch

impl Inner {
    pub(crate) async fn handle(&self, req: Request<Body>) -> Resp {
        let method = req.method().clone();
        let mut resp = self.handle_inner(req).await;
        let not_allowed = resp.status() == StatusCode::METHOD_NOT_ALLOWED;
        let h = resp.headers_mut();
        h.insert("dav", HeaderValue::from_static("1, 2"));
        h.insert("ms-author-via", HeaderValue::from_static("DAV"));
        if not_allowed {
            h.insert(header::ALLOW, HeaderValue::from_static(ALLOW));
        }
        tracing::debug!(%method, status = resp.status().as_u16(), "webdav");
        resp
    }

    async fn handle_inner(&self, req: Request<Body>) -> Resp {
        let (parts, body) = req.into_parts();

        // OPTIONS reveals nothing, and Windows sends it before authenticating.
        if parts.method == Method::OPTIONS {
            let mut r = status(StatusCode::OK);
            r.headers_mut()
                .insert(header::ALLOW, HeaderValue::from_static(ALLOW));
            r.headers_mut()
                .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            return r;
        }

        let client = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0);
        let user = match self.auth.authenticate(&parts.headers, client).await {
            Ok(u) => u,
            Err(AuthError::Unauthorized) => {
                let mut r = status(StatusCode::UNAUTHORIZED);
                let challenge = format!(
                    "Basic realm=\"{}\", charset=\"UTF-8\"",
                    self.config.realm.replace(['"', '\\'], "")
                );
                if let Ok(v) = HeaderValue::from_str(&challenge) {
                    r.headers_mut().insert(header::WWW_AUTHENTICATE, v);
                }
                return r;
            }
            Err(AuthError::RateLimited { retry_after }) => {
                let mut r = status(StatusCode::TOO_MANY_REQUESTS);
                r.headers_mut()
                    .insert(header::RETRY_AFTER, HeaderValue::from(retry_after));
                return r;
            }
            Err(AuthError::Forbidden) => return status(StatusCode::FORBIDDEN),
            Err(AuthError::Internal) => return status(StatusCode::INTERNAL_SERVER_ERROR),
        };

        let target = match parse_target(&self.config.prefix, parts.uri.path()) {
            Ok(t) => t,
            Err(TargetError::OutsideMount) => return status(StatusCode::NOT_FOUND),
            Err(TargetError::Forbidden) => return status(StatusCode::FORBIDDEN),
            Err(TargetError::BadEncoding) => return status(StatusCode::BAD_REQUEST),
        };

        let if_header = match parts.headers.get("if") {
            None => None,
            Some(v) => match v.to_str().ok().map(IfHeader::parse) {
                Some(Ok(h)) => Some(h),
                _ => return status(StatusCode::BAD_REQUEST),
            },
        };
        let tokens = if_header.as_ref().map(IfHeader::tokens).unwrap_or_default();
        let ctx = Ctx {
            srv: self,
            user,
            headers: parts.headers,
            if_header,
            tokens,
            backends: Default::default(),
        };

        if let Err(r) = self.evaluate_if(&ctx, &target).await {
            return r;
        }

        let m = parts.method.as_str();
        let result = match (&target, m) {
            (Target::Mount, "PROPFIND") => self.propfind_mount(&ctx, body).await,
            (Target::Mount, "GET" | "HEAD") => Ok(self.get_mount(&ctx, m == "HEAD")),
            (Target::Mount, _) => Err(status(StatusCode::METHOD_NOT_ALLOWED)),
            (Target::Source { source, path }, _) => match m {
                "PROPFIND" => self.propfind(&ctx, source, path, body).await,
                "PROPPATCH" => self.proppatch(&ctx, source, path, body).await,
                "GET" => self.get(&ctx, source, path, false).await,
                "HEAD" => self.get(&ctx, source, path, true).await,
                "PUT" => self.put(&ctx, source, path, body).await,
                "DELETE" => self.delete(&ctx, source, path).await,
                "MKCOL" => self.mkcol(&ctx, source, path, body).await,
                "COPY" => self.copy_move(&ctx, source, path, false).await,
                "MOVE" => self.copy_move(&ctx, source, path, true).await,
                "LOCK" => self.lock(&ctx, source, path, body).await,
                "UNLOCK" => self.unlock(&ctx, source, path).await,
                _ => Err(status(StatusCode::METHOD_NOT_ALLOWED)),
            },
        };
        result.unwrap_or_else(|r| r)
    }

    /// Evaluates the `If` header (RFC 4918 §10.4). For every resource it
    /// names, at least one of its lists must hold, or the request fails with
    /// 412 Precondition Failed.
    async fn evaluate_if(&self, ctx: &Ctx<'_>, target: &Target) -> Result<(), Resp> {
        let Some(h) = &ctx.if_header else {
            return Ok(());
        };
        for resource in h.resources() {
            let t = match &resource {
                None => Some(target.clone()),
                Some(uri) => {
                    split_uri(uri).and_then(|(_, p)| parse_target(&self.config.prefix, p).ok())
                }
            };
            // Find the resource's lock state and entity tag.
            let (key, etag) = match &t {
                Some(Target::Source { source, path }) => {
                    match ctx.resolve(source, path, DavPerm::View).await {
                        Ok(r) => {
                            let etag = r.storage.stat(&r.path).await.ok().map(|e| etag(&e));
                            (Some((r.source, r.key)), etag)
                        }
                        Err(_) => (None, None),
                    }
                }
                _ => (None, None),
            };
            let token_matches = |tok: &str| match (&key, self.locks.get(tok)) {
                (Some((s, p)), Some(l)) => l.covers(s, p),
                _ => false,
            };
            let ok = h
                .lists
                .iter()
                .filter(|l| l.resource == resource)
                .any(|l| list_matches(l, token_matches, etag.as_deref()));
            if !ok {
                return Err(status(StatusCode::PRECONDITION_FAILED));
            }
        }
        Ok(())
    }

    // --------------------------------------------------------------- PROPFIND

    async fn propfind_mount(&self, ctx: &Ctx<'_>, body: Body) -> Result<Resp, Resp> {
        let req = parse_propfind_body(body).await?;
        let d = depth(&ctx.headers)?;
        if matches!(d, None | Some(Depth::Infinity)) {
            return Err(precondition(
                StatusCode::FORBIDDEN,
                "propfind-finite-depth",
                &[],
            ));
        }
        let mut w = XmlWriter::new("multistatus");
        let root = Entry {
            name: String::new(),
            path: VPath::root(),
            kind: filebrary_storage::EntryKind::Dir,
            size: 0,
            modified: None,
            mime: None,
        };
        self.write_response(
            ctx,
            &mut w,
            &mount_href(&self.config.prefix),
            &root,
            None,
            &req,
        )
        .await;
        if matches!(d, Some(Depth::One)) {
            for name in ctx.user.sources() {
                let Ok(r) = ctx.resolve(&name, &VPath::root(), DavPerm::View).await else {
                    continue;
                };
                let Ok(mut e) = r.storage.stat(&r.path).await else {
                    continue;
                };
                e.name.clone_from(&name);
                let h = ctx.href(&name, &VPath::root(), true);
                self.write_response(ctx, &mut w, &h, &e, Some((&r.source, &r.key)), &req)
                    .await;
            }
        }
        Ok(xml_response(
            StatusCode::MULTI_STATUS,
            w.finish("multistatus"),
        ))
    }

    async fn propfind(
        &self,
        ctx: &Ctx<'_>,
        source: &str,
        path: &VPath,
        body: Body,
    ) -> Result<Resp, Resp> {
        let r = ctx.resolve(source, path, DavPerm::View).await?;
        let req = parse_propfind_body(body).await?;
        let d = depth(&ctx.headers)?;
        let entry = r.storage.stat(&r.path).await.map_err(storage_error)?;
        let mut w = XmlWriter::new("multistatus");
        let mut entry0 = entry.clone();
        if path.is_root() {
            entry0.name = source.to_owned();
        }
        let h = ctx.href(source, path, entry.is_dir());
        self.write_response(ctx, &mut w, &h, &entry0, Some((source, &r.key)), &req)
            .await;
        if entry.is_dir() {
            match d {
                Some(Depth::Zero) => {}
                Some(Depth::One) => {
                    let mut children = r.storage.list(&r.path).await.map_err(storage_error)?;
                    children.sort_by(|a, b| a.name.cmp(&b.name));
                    for c in children {
                        let (Ok(vis), Ok(real)) = (path.join(&c.name), r.key.join(&c.name)) else {
                            continue;
                        };
                        let h = ctx.href(source, &vis, c.is_dir());
                        self.write_response(ctx, &mut w, &h, &c, Some((source, &real)), &req)
                            .await;
                    }
                }
                Some(Depth::Infinity) | None => {
                    return Err(precondition(
                        StatusCode::FORBIDDEN,
                        "propfind-finite-depth",
                        &[],
                    ));
                }
            }
        }
        Ok(xml_response(
            StatusCode::MULTI_STATUS,
            w.finish("multistatus"),
        ))
    }

    /// Writes one `DAV:response`. `key` is the (source, real path) for dead
    /// properties and locks; `None` for the virtual mount root.
    async fn write_response(
        &self,
        ctx: &Ctx<'_>,
        w: &mut XmlWriter,
        href: &str,
        entry: &Entry,
        key: Option<(&str, &VPath)>,
        req: &PropfindRequest,
    ) {
        let dead = match key {
            Some((s, p)) => self.props.get(s, p).await,
            None => Vec::new(),
        };
        let locks = match key {
            Some((s, p)) => self.locks.discover(s, p),
            None => Vec::new(),
        };
        let quota = key.and_then(|(s, _)| ctx.user.quota(s));

        w.open("response");
        w.text("href", href);
        match req {
            PropfindRequest::PropName => {
                w.open("propstat").open("prop");
                for name in ALLPROP {
                    if entry.is_dir() && matches!(name, "getcontentlength" | "getcontenttype") {
                        continue;
                    }
                    w.empty(name);
                }
                for (n, _) in &dead {
                    w.empty_prop(n);
                }
                w.close("prop");
                w.text("status", "HTTP/1.1 200 OK");
                w.close("propstat");
            }
            PropfindRequest::AllProp(include) => {
                w.open("propstat").open("prop");
                for name in ALLPROP {
                    live_prop(ctx, w, entry, &locks, quota, name);
                }
                for (_, v) in &dead {
                    w.raw(v);
                }
                for p in include {
                    if p.ns == DAV && !ALLPROP.contains(&p.name.as_str()) {
                        live_prop(ctx, w, entry, &locks, quota, &p.name);
                    }
                }
                w.close("prop");
                w.text("status", "HTTP/1.1 200 OK");
                w.close("propstat");
            }
            PropfindRequest::Prop(names) => {
                let dead: HashMap<&PropName, &String> = dead.iter().map(|(k, v)| (k, v)).collect();
                let mut missing = Vec::new();
                let mut found = String::new();
                for p in names {
                    let mut tmp = XmlWriter::default();
                    let ok = if p.ns == DAV {
                        live_prop(ctx, &mut tmp, entry, &locks, quota, &p.name)
                    } else {
                        false
                    };
                    if ok {
                        found.push_str(&tmp.finish_fragment());
                    } else if let Some(v) = dead.get(p) {
                        found.push_str(v);
                    } else {
                        missing.push(p);
                    }
                }
                if !found.is_empty() || missing.is_empty() {
                    w.open("propstat").open("prop").raw(&found).close("prop");
                    w.text("status", "HTTP/1.1 200 OK");
                    w.close("propstat");
                }
                if !missing.is_empty() {
                    w.open("propstat").open("prop");
                    for p in missing {
                        w.empty_prop(p);
                    }
                    w.close("prop");
                    w.text("status", "HTTP/1.1 404 Not Found");
                    w.close("propstat");
                }
            }
        }
        w.close("response");
    }

    // -------------------------------------------------------------- PROPPATCH

    async fn proppatch(
        &self,
        ctx: &Ctx<'_>,
        source: &str,
        path: &VPath,
        body: Body,
    ) -> Result<Resp, Resp> {
        let r = ctx.resolve(source, path, DavPerm::Edit).await?;
        let entry = r.storage.stat(&r.path).await.map_err(storage_error)?;
        ctx.check_lock(source, &r.key, Scope::Resource)?;
        let body = read_xml_body(body).await?;
        let ops = xml::parse_proppatch(&body).map_err(|_| status(StatusCode::BAD_REQUEST))?;
        let names: Vec<&PropName> = ops
            .iter()
            .map(|o| match o {
                PatchOp::Set(n, _) | PatchOp::Remove(n) => n,
            })
            .collect();
        let protected = |n: &PropName| n.ns == DAV && PROTECTED.contains(&n.name.as_str());
        let any_protected = names.iter().any(|n| protected(n));
        // Atomic: either every change is applied or none (RFC 4918 §9.2).
        let failure = if any_protected {
            None
        } else {
            match self.props.patch(source, &r.key, &ops).await {
                Ok(()) => None,
                Err(PropError::TooLarge) => Some("HTTP/1.1 507 Insufficient Storage"),
            }
        };
        let mut w = XmlWriter::new("multistatus");
        w.open("response");
        w.text("href", &ctx.href(source, path, entry.is_dir()));
        let mut seen = std::collections::HashSet::new();
        for n in names {
            if !seen.insert(n) {
                continue;
            }
            let st = if any_protected {
                if protected(n) {
                    "HTTP/1.1 403 Forbidden"
                } else {
                    "HTTP/1.1 424 Failed Dependency"
                }
            } else {
                failure.unwrap_or("HTTP/1.1 200 OK")
            };
            w.open("propstat").open("prop");
            w.empty_prop(n);
            w.close("prop");
            w.text("status", st);
            if any_protected && protected(n) {
                w.open("error")
                    .empty("cannot-modify-protected-property")
                    .close("error");
            }
            w.close("propstat");
        }
        w.close("response");
        if failure.is_none() && !any_protected {
            ctx.notify(Change::Properties {
                source: source.to_owned(),
                path: r.key.clone(),
            });
        }
        Ok(xml_response(
            StatusCode::MULTI_STATUS,
            w.finish("multistatus"),
        ))
    }

    // --------------------------------------------------------------- GET/HEAD

    fn get_mount(&self, ctx: &Ctx<'_>, head: bool) -> Resp {
        let items: Vec<(String, String)> = ctx
            .user
            .sources()
            .into_iter()
            .map(|s| (ctx.href(&s, &VPath::root(), true), format!("{s}/")))
            .collect();
        html_listing(&mount_href(&self.config.prefix), &items, head)
    }

    async fn get(
        &self,
        ctx: &Ctx<'_>,
        source: &str,
        path: &VPath,
        head: bool,
    ) -> Result<Resp, Resp> {
        let view = ctx.resolve(source, path, DavPerm::View).await?;
        let entry = view.storage.stat(&view.path).await.map_err(storage_error)?;
        if entry.is_dir() {
            let mut children = view.storage.list(&view.path).await.map_err(storage_error)?;
            children.sort_by(|a, b| (!a.is_dir(), &a.name).cmp(&(!b.is_dir(), &b.name)));
            let mut items = Vec::new();
            if let Some(parent) = path.parent() {
                items.push((ctx.href(source, &parent, true), "../".to_owned()));
            } else {
                items.push((mount_href(&self.config.prefix), "../".to_owned()));
            }
            for c in children {
                if let Ok(v) = path.join(&c.name) {
                    let label = if c.is_dir() {
                        format!("{}/", c.name)
                    } else {
                        c.name.clone()
                    };
                    items.push((ctx.href(source, &v, c.is_dir()), label));
                }
            }
            return Ok(html_listing(&ctx.href(source, path, true), &items, head));
        }
        let r = ctx.resolve(source, path, DavPerm::Download).await?;
        let tag = etag(&entry);
        let modified = system_time(&entry);

        // Conditional GET.
        if let Some(inm) = ctx.header(header::IF_NONE_MATCH) {
            if etag_list_matches(inm, &tag) {
                return Ok(not_modified(&tag, modified));
            }
        } else if let (Some(ims), Some(m)) = (
            ctx.header(header::IF_MODIFIED_SINCE)
                .and_then(|v| httpdate::parse_http_date(v).ok()),
            modified,
        ) && m
            .duration_since(ims)
            .map_or(true, |d| d < Duration::from_secs(1))
        {
            return Ok(not_modified(&tag, modified));
        }
        if let Some(im) = ctx.header(header::IF_MATCH)
            && !etag_list_matches(im, &tag)
        {
            return Err(status(StatusCode::PRECONDITION_FAILED));
        }

        let size = entry.size;
        let range = match ctx.header(header::RANGE) {
            Some(v) if if_range_ok(ctx, &tag, modified) => match parse_range(v, size) {
                RangeSpec::None => None,
                RangeSpec::Unsatisfiable => {
                    let mut resp = status(StatusCode::RANGE_NOT_SATISFIABLE);
                    if let Ok(v) = HeaderValue::from_str(&format!("bytes */{size}")) {
                        resp.headers_mut().insert(header::CONTENT_RANGE, v);
                    }
                    return Err(resp);
                }
                RangeSpec::One(r) => Some(r),
            },
            _ => None,
        };
        let len = range.as_ref().map_or(size, |r| r.end - r.start);
        let body = if head {
            Body::empty()
        } else {
            let stream = r
                .storage
                .read(&r.path, range.clone())
                .await
                .map_err(storage_error)?;
            Body::from_stream(stream)
        };
        let mut resp = Response::new(body);
        if let Some(rg) = &range {
            *resp.status_mut() = StatusCode::PARTIAL_CONTENT;
            let h = resp.headers_mut();
            if let Ok(v) =
                HeaderValue::from_str(&format!("bytes {}-{}/{size}", rg.start, rg.end - 1))
            {
                h.insert(header::CONTENT_RANGE, v);
            }
        }
        let h = resp.headers_mut();
        h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
        h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        if let Ok(v) = HeaderValue::from_str(&content_type(&entry)) {
            h.insert(header::CONTENT_TYPE, v);
        }
        if let Ok(v) = HeaderValue::from_str(&tag) {
            h.insert(header::ETAG, v);
        }
        if let Some(m) = modified
            && let Ok(v) = HeaderValue::from_str(&http_date(m))
        {
            h.insert(header::LAST_MODIFIED, v);
        }
        // Never let a browser render user content as a page on this origin.
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; sandbox"),
        );
        h.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        Ok(resp)
    }

    // -------------------------------------------------------------------- PUT

    async fn put(
        &self,
        ctx: &Ctx<'_>,
        source: &str,
        path: &VPath,
        body: Body,
    ) -> Result<Resp, Resp> {
        let view = ctx.resolve(source, path, DavPerm::View).await?;
        if path.is_root() {
            return Err(status(StatusCode::METHOD_NOT_ALLOWED));
        }
        // A partial PUT would replace the file with the fragment (RFC 9110 §9.3.4).
        if ctx.headers.contains_key(header::CONTENT_RANGE) {
            return Err(status(StatusCode::BAD_REQUEST));
        }
        let existing = stat_opt(&view).await?;
        if existing.as_ref().is_some_and(Entry::is_dir) {
            return Err(status(StatusCode::METHOD_NOT_ALLOWED));
        }
        let perm = if existing.is_some() {
            DavPerm::Edit
        } else {
            DavPerm::Upload
        };
        let r = ctx.resolve(source, path, perm).await?;
        if let Some(im) = ctx.header(header::IF_MATCH)
            && !existing
                .as_ref()
                .is_some_and(|e| etag_list_matches(im, &etag(e)))
        {
            return Err(status(StatusCode::PRECONDITION_FAILED));
        }
        if let Some(inm) = ctx.header(header::IF_NONE_MATCH)
            && existing
                .as_ref()
                .is_some_and(|e| etag_list_matches(inm, &etag(e)))
        {
            return Err(status(StatusCode::PRECONDITION_FAILED));
        }
        if existing.is_none() {
            require_parent(&r).await?;
            ctx.check_membership(source, &r.key)?;
        }
        ctx.check_lock(source, &r.key, Scope::Resource)?;
        if let (Some(q), Some(len)) = (ctx.user.quota(source), content_length(ctx)) {
            let old = existing.as_ref().map_or(0, |e| e.size);
            if q.available.is_some_and(|a| len.saturating_sub(old) > a) {
                return Err(status(StatusCode::INSUFFICIENT_STORAGE));
            }
        }
        r.storage
            .write(&r.path, body_reader(body), true)
            .await
            .map_err(storage_error)?;
        let mut resp = status(if existing.is_some() {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::CREATED
        });
        if let Ok(e) = r.storage.stat(&r.path).await
            && let Ok(v) = HeaderValue::from_str(&etag(&e))
        {
            resp.headers_mut().insert(header::ETAG, v);
        }
        ctx.notify(Change::Written {
            source: source.to_owned(),
            path: r.key,
            created: existing.is_none(),
        });
        Ok(resp)
    }

    // ----------------------------------------------------------------- DELETE

    async fn delete(&self, ctx: &Ctx<'_>, source: &str, path: &VPath) -> Result<Resp, Resp> {
        let r = ctx.resolve(source, path, DavPerm::Delete).await?;
        if r.key.is_root() || path.is_root() {
            return Err(status(StatusCode::FORBIDDEN));
        }
        let entry = r.storage.stat(&r.path).await.map_err(storage_error)?;
        if entry.is_dir() && !matches!(depth(&ctx.headers)?, None | Some(Depth::Infinity)) {
            return Err(status(StatusCode::BAD_REQUEST));
        }
        ctx.check_lock(source, &r.key, Scope::Tree)?;
        ctx.check_membership(source, &r.key)?;
        r.storage.delete(&r.path).await.map_err(storage_error)?;
        self.props.delete_tree(source, &r.key).await;
        self.locks.remove_tree(source, &r.key);
        ctx.notify(Change::Deleted {
            source: source.to_owned(),
            path: r.key,
        });
        Ok(status(StatusCode::NO_CONTENT))
    }

    // ------------------------------------------------------------------ MKCOL

    async fn mkcol(
        &self,
        ctx: &Ctx<'_>,
        source: &str,
        path: &VPath,
        body: Body,
    ) -> Result<Resp, Resp> {
        let r = ctx.resolve(source, path, DavPerm::Upload).await?;
        if path.is_root() {
            return Err(status(StatusCode::METHOD_NOT_ALLOWED));
        }
        // MKCOL with a body asks for something we do not support (RFC 4918 §9.3).
        let body = read_xml_body(body).await?;
        if !body.is_empty() {
            return Err(status(StatusCode::UNSUPPORTED_MEDIA_TYPE));
        }
        if stat_opt(&r).await?.is_some() {
            return Err(status(StatusCode::METHOD_NOT_ALLOWED));
        }
        require_parent(&r).await?;
        ctx.check_membership(source, &r.key)?;
        ctx.check_lock(source, &r.key, Scope::Resource)?;
        r.storage.mkdir(&r.path).await.map_err(storage_error)?;
        ctx.notify(Change::CreatedCollection {
            source: source.to_owned(),
            path: r.key,
        });
        Ok(status(StatusCode::CREATED))
    }

    // ------------------------------------------------------------- COPY, MOVE

    async fn copy_move(
        &self,
        ctx: &Ctx<'_>,
        source: &str,
        path: &VPath,
        is_move: bool,
    ) -> Result<Resp, Resp> {
        // Where to.
        let dest = ctx
            .header("destination")
            .ok_or_else(|| status(StatusCode::BAD_REQUEST))?;
        let (_, dest_path) = split_uri(dest).ok_or_else(|| status(StatusCode::BAD_REQUEST))?;
        let (dsource, dpath) = match parse_target(&self.config.prefix, dest_path) {
            Ok(Target::Source { source, path }) => (source, path),
            Ok(Target::Mount) => return Err(status(StatusCode::FORBIDDEN)),
            // Outside this mount: another server as far as we are concerned.
            Err(TargetError::OutsideMount) => return Err(status(StatusCode::BAD_GATEWAY)),
            Err(TargetError::Forbidden) => return Err(status(StatusCode::FORBIDDEN)),
            Err(TargetError::BadEncoding) => return Err(status(StatusCode::BAD_REQUEST)),
        };
        let overwrite = match ctx.header("overwrite") {
            None => true,
            Some(v) if v.eq_ignore_ascii_case("t") => true,
            Some(v) if v.eq_ignore_ascii_case("f") => false,
            Some(_) => return Err(status(StatusCode::BAD_REQUEST)),
        };
        let d = depth(&ctx.headers)?;
        let shallow = match (is_move, d) {
            (_, None | Some(Depth::Infinity)) => false,
            (false, Some(Depth::Zero)) => true,
            _ => return Err(status(StatusCode::BAD_REQUEST)),
        };

        // From where.
        let src = ctx
            .resolve(
                source,
                path,
                if is_move {
                    DavPerm::Edit
                } else {
                    DavPerm::Download
                },
            )
            .await?;
        if path.is_root() && is_move {
            return Err(status(StatusCode::FORBIDDEN));
        }
        let src_entry = src.storage.stat(&src.path).await.map_err(storage_error)?;
        if is_move && src.key.is_root() {
            return Err(status(StatusCode::FORBIDDEN));
        }

        let dview = ctx.resolve(&dsource, &dpath, DavPerm::View).await?;
        if dview.key.is_root() {
            return Err(status(StatusCode::FORBIDDEN));
        }
        let same_source = dsource == source;
        if same_source && dview.key == src.key {
            return Err(status(StatusCode::FORBIDDEN));
        }
        if same_source
            && src_entry.is_dir()
            && (dview.key.starts_with(&src.key) || (is_move && src.key.starts_with(&dview.key)))
        {
            return Err(status(StatusCode::CONFLICT));
        }
        if is_move && same_source && src.key.starts_with(&dview.key) {
            // Overwriting an ancestor would delete the source with it.
            return Err(status(StatusCode::CONFLICT));
        }
        let existing = stat_opt(&dview).await?;
        if existing.is_some() && !overwrite {
            return Err(status(StatusCode::PRECONDITION_FAILED));
        }
        let dst = ctx
            .resolve(
                &dsource,
                &dpath,
                if existing.is_some() {
                    DavPerm::Edit
                } else {
                    DavPerm::Upload
                },
            )
            .await?;
        if existing.is_some() {
            // Replacing the destination deletes it first.
            ctx.resolve(&dsource, &dpath, DavPerm::Delete).await?;
        }
        require_parent(&dst).await?;

        // Locks.
        if is_move {
            ctx.check_lock(source, &src.key, Scope::Tree)?;
            ctx.check_membership(source, &src.key)?;
        }
        ctx.check_lock(&dsource, &dst.key, Scope::Tree)?;
        ctx.check_membership(&dsource, &dst.key)?;

        // Do it.
        if existing.is_some() {
            dst.storage.delete(&dst.path).await.map_err(storage_error)?;
            self.props.delete_tree(&dsource, &dst.key).await;
            self.locks.remove_tree(&dsource, &dst.key);
        }
        if same_source {
            let res = if is_move {
                src.storage.rename(&src.path, &dst.path, false).await
            } else if shallow && src_entry.is_dir() {
                src.storage.mkdir(&dst.path).await
            } else {
                src.storage.copy(&src.path, &dst.path, false).await
            };
            res.map_err(storage_error)?;
        } else {
            copy_across(&src, &dst, src_entry.is_dir(), shallow)
                .await
                .map_err(storage_error)?;
            if is_move {
                src.storage.delete(&src.path).await.map_err(storage_error)?;
            }
        }
        if is_move {
            self.props
                .move_tree(source, &src.key, &dsource, &dst.key)
                .await;
            self.locks.remove_tree(source, &src.key);
            ctx.notify(Change::Moved {
                source: source.to_owned(),
                from: src.key.clone(),
                to_source: dsource.clone(),
                to: dst.key.clone(),
            });
        } else {
            if shallow {
                let props: Vec<PatchOp> = self
                    .props
                    .get(source, &src.key)
                    .await
                    .into_iter()
                    .map(|(n, v)| PatchOp::Set(n, v))
                    .collect();
                if !props.is_empty() {
                    let _ = self.props.patch(&dsource, &dst.key, &props).await;
                }
            } else {
                self.props
                    .copy_tree(source, &src.key, &dsource, &dst.key)
                    .await;
            }
            ctx.notify(Change::Copied {
                source: source.to_owned(),
                from: src.key.clone(),
                to_source: dsource.clone(),
                to: dst.key.clone(),
            });
        }
        Ok(status(if existing.is_some() {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::CREATED
        }))
    }

    // ------------------------------------------------------------ LOCK/UNLOCK

    async fn lock(
        &self,
        ctx: &Ctx<'_>,
        source: &str,
        path: &VPath,
        body: Body,
    ) -> Result<Resp, Resp> {
        let view = ctx.resolve(source, path, DavPerm::View).await?;
        let existing = stat_opt(&view).await?;
        let timeout = parse_timeout(ctx.header("timeout"));
        let body = read_xml_body(body).await?;

        // An empty body refreshes the locks named in the If header.
        if body.iter().all(u8::is_ascii_whitespace) {
            let Some(entry) = existing else {
                return Err(status(StatusCode::PRECONDITION_FAILED));
            };
            let mut refreshed = None;
            for t in &ctx.tokens {
                match self
                    .locks
                    .refresh(t, &ctx.principal(), source, &view.key, timeout)
                {
                    Ok(l) => {
                        refreshed = Some(l);
                    }
                    Err(LockError::NotOwner) => return Err(status(StatusCode::FORBIDDEN)),
                    Err(_) => {}
                }
            }
            let Some(lock) = refreshed else {
                return Err(precondition(
                    StatusCode::PRECONDITION_FAILED,
                    "lock-token-matches-request-uri",
                    &[],
                ));
            };
            let mut w = XmlWriter::new("prop");
            w.open("lockdiscovery");
            write_activelock(ctx, &mut w, &lock, entry.is_dir());
            w.close("lockdiscovery");
            return Ok(xml_response(StatusCode::OK, w.finish("prop")));
        }

        let info = xml::parse_lockinfo(&body).map_err(|_| status(StatusCode::BAD_REQUEST))?;
        let infinite = match depth(&ctx.headers)? {
            None | Some(Depth::Infinity) => true,
            Some(Depth::Zero) => false,
            Some(Depth::One) => return Err(status(StatusCode::BAD_REQUEST)),
        };
        let r = ctx
            .resolve(
                source,
                path,
                if existing.is_some() {
                    DavPerm::Edit
                } else {
                    DavPerm::Upload
                },
            )
            .await?;
        if existing.is_none() {
            if path.is_root() {
                return Err(status(StatusCode::CONFLICT));
            }
            require_parent(&r).await?;
            ctx.check_membership(source, &r.key)?;
        }
        let is_dir = existing.as_ref().is_some_and(Entry::is_dir);
        let lock = match self.locks.lock(
            source,
            &r.key,
            infinite && is_dir,
            info.shared,
            info.owner,
            &ctx.principal(),
            timeout,
        ) {
            Ok(l) => l,
            Err(LockError::Conflict(roots)) => {
                let hrefs: Vec<String> = roots
                    .iter()
                    .filter_map(|p| ctx.user.visible(source, p))
                    .map(|v| ctx.href(source, &v, false))
                    .collect();
                return Err(precondition(
                    StatusCode::LOCKED,
                    "no-conflicting-lock",
                    &hrefs,
                ));
            }
            Err(LockError::TooMany) => return Err(status(StatusCode::SERVICE_UNAVAILABLE)),
            Err(_) => return Err(status(StatusCode::LOCKED)),
        };

        // Locking an unmapped URL creates an empty file (RFC 4918 §7.3).
        let created = existing.is_none();
        if created {
            let empty: filebrary_storage::ByteReader = Box::pin(tokio::io::empty());
            if let Err(e) = r.storage.write(&r.path, empty, false).await {
                let _ = self
                    .locks
                    .unlock(&lock.token, &ctx.principal(), source, &r.key);
                return Err(storage_error(e));
            }
            ctx.notify(Change::Written {
                source: source.to_owned(),
                path: r.key.clone(),
                created: true,
            });
        }

        let mut w = XmlWriter::new("prop");
        w.open("lockdiscovery");
        write_activelock(ctx, &mut w, &lock, is_dir);
        w.close("lockdiscovery");
        let mut resp = xml_response(
            if created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            },
            w.finish("prop"),
        );
        if let Ok(v) = HeaderValue::from_str(&format!("<{}>", lock.token)) {
            resp.headers_mut().insert("lock-token", v);
        }
        Ok(resp)
    }

    async fn unlock(&self, ctx: &Ctx<'_>, source: &str, path: &VPath) -> Result<Resp, Resp> {
        let r = ctx.resolve(source, path, DavPerm::View).await?;
        let token = ctx
            .header("lock-token")
            .map(str::trim)
            .and_then(|t| t.strip_prefix('<'))
            .and_then(|t| t.strip_suffix('>'))
            .ok_or_else(|| status(StatusCode::BAD_REQUEST))?;
        match self.locks.unlock(token, &ctx.principal(), source, &r.key) {
            Ok(()) => Ok(status(StatusCode::NO_CONTENT)),
            Err(LockError::NotOwner) => Err(status(StatusCode::FORBIDDEN)),
            Err(_) => Err(precondition(
                StatusCode::CONFLICT,
                "lock-token-matches-request-uri",
                &[],
            )),
        }
    }
}

// --------------------------------------------------------------- properties

/// Writes one live `DAV:` property. Returns false if the resource does not
/// have it.
fn live_prop(
    ctx: &Ctx<'_>,
    w: &mut XmlWriter,
    e: &Entry,
    locks: &[Lock],
    quota: Option<crate::Quota>,
    name: &str,
) -> bool {
    match name {
        "resourcetype" => {
            if e.is_dir() {
                w.open("resourcetype")
                    .empty("collection")
                    .close("resourcetype");
            } else {
                w.empty("resourcetype");
            }
        }
        "displayname" => {
            w.text("displayname", &e.name);
        }
        "getcontentlength" if !e.is_dir() => {
            w.text("getcontentlength", &e.size.to_string());
        }
        "getcontenttype" if !e.is_dir() => {
            w.text("getcontenttype", &content_type(e));
        }
        "getetag" => {
            w.text("getetag", &etag(e));
        }
        "getlastmodified" => match system_time(e) {
            Some(t) => {
                w.text("getlastmodified", &http_date(t));
            }
            None => return false,
        },
        "creationdate" => match system_time(e) {
            // Backends do not report a birth time everywhere; the last change
            // is the closest truthful value.
            Some(t) => {
                w.text("creationdate", &rfc3339(t));
            }
            None => return false,
        },
        "supportedlock" => {
            w.open("supportedlock");
            for scope in ["exclusive", "shared"] {
                w.open("lockentry")
                    .open("lockscope")
                    .empty(scope)
                    .close("lockscope")
                    .open("locktype")
                    .empty("write")
                    .close("locktype")
                    .close("lockentry");
            }
            w.close("supportedlock");
        }
        "lockdiscovery" => {
            if locks.is_empty() {
                w.empty("lockdiscovery");
            } else {
                w.open("lockdiscovery");
                for l in locks {
                    write_activelock(ctx, w, l, e.is_dir() || l.infinite);
                }
                w.close("lockdiscovery");
            }
        }
        "quota-used-bytes" => match quota {
            Some(q) => {
                w.text("quota-used-bytes", &q.used.to_string());
            }
            None => return false,
        },
        "quota-available-bytes" => match quota.and_then(|q| q.available) {
            Some(a) => {
                w.text("quota-available-bytes", &a.to_string());
            }
            None => return false,
        },
        _ => return false,
    }
    true
}

fn write_activelock(ctx: &Ctx<'_>, w: &mut XmlWriter, l: &Lock, root_is_dir: bool) {
    w.open("activelock");
    w.open("locktype").empty("write").close("locktype");
    w.open("lockscope")
        .empty(if l.shared { "shared" } else { "exclusive" })
        .close("lockscope");
    w.text("depth", if l.infinite { "infinity" } else { "0" });
    if let Some(o) = &l.owner {
        w.raw(o);
    }
    w.text(
        "timeout",
        &format!("Second-{}", l.remaining().as_secs().max(1)),
    );
    w.open("locktoken")
        .text("href", &l.token)
        .close("locktoken");
    if let Some(h) = ctx.lock_href(l, root_is_dir && l.infinite) {
        w.open("lockroot").text("href", &h).close("lockroot");
    }
    w.close("activelock");
}

async fn parse_propfind_body(body: Body) -> Result<PropfindRequest, Resp> {
    let body = read_xml_body(body).await?;
    xml::parse_propfind(&body).map_err(|_| status(StatusCode::BAD_REQUEST))
}

// --------------------------------------------------------------- GET helpers

fn content_length(ctx: &Ctx<'_>) -> Option<u64> {
    ctx.header(header::CONTENT_LENGTH)
        .or_else(|| ctx.header("x-expected-entity-length"))
        .and_then(|v| v.parse().ok())
}

fn etag_list_matches(list: &str, tag: &str) -> bool {
    list.split(',').map(str::trim).any(|t| t == "*" || t == tag)
}

fn not_modified(tag: &str, modified: Option<SystemTime>) -> Resp {
    let mut r = status(StatusCode::NOT_MODIFIED);
    if let Ok(v) = HeaderValue::from_str(tag) {
        r.headers_mut().insert(header::ETAG, v);
    }
    if let Some(m) = modified
        && let Ok(v) = HeaderValue::from_str(&http_date(m))
    {
        r.headers_mut().insert(header::LAST_MODIFIED, v);
    }
    r
}

fn if_range_ok(ctx: &Ctx<'_>, tag: &str, modified: Option<SystemTime>) -> bool {
    match ctx.header(header::IF_RANGE) {
        None => true,
        Some(v) if v.starts_with('"') => v == tag,
        Some(v) => match (httpdate::parse_http_date(v), modified) {
            (Ok(d), Some(m)) => m
                .duration_since(d)
                .map_or(true, |x| x < Duration::from_secs(1)),
            _ => false,
        },
    }
}

/// The result of parsing a `Range` header.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RangeSpec {
    /// No usable range: send the whole file (also for multiple ranges).
    None,
    /// Syntactically valid but outside the file (416).
    Unsatisfiable,
    /// One half-open byte range.
    One(Range<u64>),
}

/// Parses a single `bytes=` range against a file of `size` bytes.
pub(crate) fn parse_range(v: &str, size: u64) -> RangeSpec {
    let Some(spec) = v.trim().strip_prefix("bytes=") else {
        return RangeSpec::None;
    };
    if spec.contains(',') {
        return RangeSpec::None;
    }
    let Some((a, b)) = spec.trim().split_once('-') else {
        return RangeSpec::None;
    };
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() {
        // Suffix: the last N bytes.
        let Ok(n) = b.parse::<u64>() else {
            return RangeSpec::None;
        };
        if n == 0 || size == 0 {
            return RangeSpec::Unsatisfiable;
        }
        return RangeSpec::One(size.saturating_sub(n)..size);
    }
    let Ok(start) = a.parse::<u64>() else {
        return RangeSpec::None;
    };
    let end = if b.is_empty() {
        size
    } else {
        match b.parse::<u64>() {
            Ok(e) if e >= start => e.saturating_add(1).min(size),
            _ => return RangeSpec::None,
        }
    };
    if start >= size {
        return RangeSpec::Unsatisfiable;
    }
    RangeSpec::One(start..end)
}

fn html_listing(title: &str, items: &[(String, String)], head: bool) -> Resp {
    let mut s = String::from("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>");
    s.push_str(&escape_text(title));
    s.push_str("</title></head><body><h1>");
    s.push_str(&escape_text(title));
    s.push_str("</h1><ul>");
    for (href, label) in items {
        s.push_str("<li><a href=\"");
        s.push_str(&xml::escape_attr(href));
        s.push_str("\">");
        s.push_str(&escape_text(label));
        s.push_str("</a></li>");
    }
    s.push_str("</ul></body></html>");
    let len = s.len();
    let mut r = Response::new(if head { Body::empty() } else { Body::from(s) });
    let h = r.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'"),
    );
    r
}

// --------------------------------------------------------- cross-source copy

/// Copies between two different sources by streaming through the server.
async fn copy_across(
    src: &Resolved,
    dst: &Resolved,
    is_dir: bool,
    shallow: bool,
) -> Result<(), StorageError> {
    if !is_dir {
        let stream = src.storage.read(&src.path, None).await?;
        let reader: filebrary_storage::ByteReader =
            Box::pin(tokio_util::io::StreamReader::new(stream));
        dst.storage.write(&dst.path, reader, false).await?;
        return Ok(());
    }
    // Iterative depth-first walk so deep trees cannot overflow the stack.
    let mut stack = vec![(src.key.clone(), dst.key.clone())];
    while let Some((from, to)) = stack.pop() {
        dst.storage.mkdir(&to).await?;
        if shallow {
            break;
        }
        for c in src.storage.list(&from).await? {
            let (f, t) = (from.join(&c.name)?, to.join(&c.name)?);
            if c.is_dir() {
                stack.push((f, t));
            } else {
                let stream = src.storage.read(&f, None).await?;
                let reader: filebrary_storage::ByteReader =
                    Box::pin(tokio_util::io::StreamReader::new(stream));
                dst.storage.write(&t, reader, false).await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-9", 100), RangeSpec::One(0..10));
        assert_eq!(parse_range("bytes=90-", 100), RangeSpec::One(90..100));
        assert_eq!(parse_range("bytes=-10", 100), RangeSpec::One(90..100));
        assert_eq!(parse_range("bytes=-500", 100), RangeSpec::One(0..100));
        assert_eq!(parse_range("bytes=50-500", 100), RangeSpec::One(50..100));
        assert_eq!(parse_range("bytes=100-", 100), RangeSpec::Unsatisfiable);
        assert_eq!(parse_range("bytes=-0", 100), RangeSpec::Unsatisfiable);
        assert_eq!(parse_range("bytes=5-1", 100), RangeSpec::None);
        assert_eq!(parse_range("bytes=0-1,5-6", 100), RangeSpec::None);
        assert_eq!(parse_range("items=0-1", 100), RangeSpec::None);
        assert_eq!(parse_range("bytes=x-1", 100), RangeSpec::None);
    }

    #[test]
    fn etag_lists() {
        assert!(etag_list_matches("*", "\"a\""));
        assert!(etag_list_matches("\"b\", \"a\"", "\"a\""));
        assert!(!etag_list_matches("\"b\"", "\"a\""));
    }
}
