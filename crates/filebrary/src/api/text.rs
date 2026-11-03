//! Text editing: load a text file for the web editor and save it back safely.
//!
//! # Routes (under `/api`)
//!
//! * `GET /sources/{id}/text?path=/notes/todo.md` returns
//!   `{path, content, etag, size, modified}` and an `ETag` header. Needs the
//!   `download` permission. Files larger than [`MAX_TEXT_BYTES`], files that
//!   are not valid UTF-8 and files containing NUL bytes are refused with
//!   `415 not_text` / `413 payload_too_large` so binaries never reach the editor.
//! * `PUT /sources/{id}/text?path=...` with the new content as the raw body.
//!   Exactly one precondition header is required:
//!   * `If-Match: "<etag>"` overwrites an existing file only if it still has
//!     that ETag (needs `edit`);
//!   * `If-None-Match: *` creates a file only if nothing exists yet (needs
//!     `upload`).
//!
//!   Without either header the request fails with `428 precondition_required`,
//!   so a client can never overwrite a file blindly. When the precondition
//!   fails, the response is `412` with
//!   `{"error": "precondition_failed", "message": ..., "etag": <current or null>}`
//!   so the editor can offer "reload" or "overwrite anyway" (the latter is a
//!   new PUT with the returned ETag). On success it returns
//!   `{path, etag, size, modified}`.
//!
//! # ETags and conflict detection
//!
//! The ETag is a strong validator derived from the file's SHA-256, not from
//! its modification time: mtimes have coarse resolution on some file systems
//! (FAT, SMB, many FUSE pools) and two saves within one tick would otherwise
//! look identical. Hashing is cheap at the editor's size limit.
//!
//! Saves to the same file through this endpoint are serialised by an
//! in-process lock, so the check-then-write sequence cannot interleave between
//! two editors. A writer outside Filebrary (a shell, WebDAV client, another
//! process) can still change the file between the check and the atomic rename;
//! that window is the duration of one hash and one write.

use std::{
    hash::{Hash, Hasher},
    io::Cursor,
    sync::{Arc, OnceLock},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use tokio::sync::Mutex;

use crate::{
    api::files::scoped,
    auth::{AuthUser, Perms},
    crypto,
    error::{AppError, AppResult},
    state::AppState,
    storage::{Entry, Storage, StorageError, VPath},
};

/// Largest file the editor will open or save, in bytes (5 MiB).
///
/// Bigger text files (logs, dumps) are better downloaded; a browser editor
/// becomes sluggish well before this point anyway.
pub const MAX_TEXT_BYTES: u64 = 5 * 1024 * 1024;

/// Number of lock stripes guarding saves (see the module docs).
const LOCK_STRIPES: usize = 64;

/// Routes for this module, merged under `/api`.
pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/sources/{id}/text",
        get(load)
            .put(save)
            // Allow a little headroom over the limit so an over-sized body is
            // reported by our own check with a clear message.
            .layer(DefaultBodyLimit::max(
                usize::try_from(MAX_TEXT_BYTES).unwrap_or(usize::MAX) + 1,
            )),
    )
}

/// The `path` query parameter shared by both routes.
#[derive(Debug, Deserialize)]
pub struct PathQuery {
    /// User-visible path of the file inside the source.
    pub path: String,
}

/// Body of a successful `GET`.
#[derive(Debug, Serialize, Deserialize)]
pub struct TextFile {
    /// The user-visible path, normalised.
    pub path: String,
    /// The whole file as UTF-8.
    pub content: String,
    /// Strong ETag (quoted), to send back in `If-Match`.
    pub etag: String,
    /// Size in bytes.
    pub size: u64,
    /// Last modification time, if known.
    #[serde(with = "time::serde::rfc3339::option")]
    pub modified: Option<OffsetDateTime>,
}

/// Body of a successful `PUT`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Saved {
    /// The user-visible path, normalised.
    pub path: String,
    /// ETag of the content now on disk.
    pub etag: String,
    /// Size in bytes.
    pub size: u64,
    /// Modification time after the save, if known.
    #[serde(with = "time::serde::rfc3339::option")]
    pub modified: Option<OffsetDateTime>,
}

/// Computes the strong ETag for some content: a quoted, truncated SHA-256.
pub fn etag_for(content: &[u8]) -> String {
    let digest = crypto::sha256(content);
    format!("\"{}\"", hex::encode(&digest[..16]))
}

/// Checks that `bytes` is something the editor can show: UTF-8 without NULs.
fn as_text(bytes: Vec<u8>) -> AppResult<String> {
    let text = String::from_utf8(bytes).map_err(|_| not_text())?;
    if text.contains('\0') {
        return Err(not_text());
    }
    Ok(text)
}

/// The error for content the editor cannot handle.
fn not_text() -> AppError {
    AppError::bad_request("this file is not a UTF-8 text file")
}

/// Reads a whole file into memory, refusing anything over [`MAX_TEXT_BYTES`].
///
/// Returns `None` if nothing exists at `path`.
async fn read_current(storage: &dyn Storage, path: &VPath) -> AppResult<Option<(Entry, Vec<u8>)>> {
    let entry = match storage.stat(path).await {
        Ok(e) => e,
        Err(StorageError::NotFound) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if entry.is_dir() {
        return Err(AppError::bad_request("is a directory"));
    }
    if entry.size > MAX_TEXT_BYTES {
        return Err(AppError::PayloadTooLarge);
    }
    let mut stream = storage.read(path, None).await?;
    let mut buf = Vec::with_capacity(usize::try_from(entry.size).unwrap_or(0));
    while let Some(chunk) = stream.try_next().await? {
        buf.extend_from_slice(&chunk);
        // The file may have grown since `stat`.
        if buf.len() as u64 > MAX_TEXT_BYTES {
            return Err(AppError::PayloadTooLarge);
        }
    }
    Ok(Some((entry, buf)))
}

/// `GET /api/sources/{id}/text?path=...`: opens a text file for editing.
pub async fn load(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PathQuery>,
    auth: AuthUser,
) -> AppResult<Response> {
    let vp = VPath::parse(&q.path)?;
    let storage = scoped(&state, &auth.user, &id, Perms::DOWNLOAD, &vp).await?;
    let (entry, bytes) = read_current(storage.as_ref(), &vp)
        .await?
        .ok_or(AppError::NotFound)?;
    let etag = etag_for(&bytes);
    let Ok(content) = as_text(bytes) else {
        return Ok(error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "not_text",
            "this file is not a UTF-8 text file",
            None,
        ));
    };
    let body = TextFile {
        path: vp.to_string(),
        content,
        etag: etag.clone(),
        size: entry.size,
        modified: entry.modified,
    };
    Ok(with_etag(Json(body).into_response(), &etag))
}

/// What the client asked us to check before writing.
#[derive(Debug, PartialEq, Eq)]
enum Precondition {
    /// Overwrite only if the current ETag is one of these.
    Matches(Vec<String>),
    /// Create only if nothing exists.
    Absent,
}

/// A rejected precondition: status, error code and message.
type PreError = (StatusCode, &'static str, &'static str);

fn precondition(headers: &HeaderMap) -> Result<Precondition, PreError> {
    let if_match = headers.get(header::IF_MATCH);
    let if_none = headers.get(header::IF_NONE_MATCH);
    match (if_match, if_none) {
        (Some(v), None) => {
            let tags: Vec<String> = v
                .to_str()
                .unwrap_or_default()
                .split(',')
                .map(|t| t.trim().to_owned())
                .filter(|t| !t.is_empty())
                .collect();
            if tags.is_empty() || tags.iter().any(|t| t.starts_with("W/") || t == "*") {
                Err((
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    "If-Match must carry the strong ETag returned when the file was opened",
                ))
            } else {
                Ok(Precondition::Matches(tags))
            }
        }
        (None, Some(v)) if v.as_bytes().trim_ascii() == b"*" => Ok(Precondition::Absent),
        (None, Some(_)) => Err((
            StatusCode::BAD_REQUEST,
            "bad_request",
            "only `If-None-Match: *` is supported on save",
        )),
        (Some(_), Some(_)) => Err((
            StatusCode::BAD_REQUEST,
            "bad_request",
            "send either If-Match or If-None-Match, not both",
        )),
        (None, None) => Err((
            StatusCode::PRECONDITION_REQUIRED,
            "precondition_required",
            "send If-Match with the file's ETag (or If-None-Match: * to create it)",
        )),
    }
}

/// `PUT /api/sources/{id}/text?path=...`: saves a text file with conflict detection.
pub async fn save(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PathQuery>,
    auth: AuthUser,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Response> {
    let vp = VPath::parse(&q.path)?;
    if vp.is_root() {
        return Err(AppError::bad_request("a file path is required"));
    }
    let pre = match precondition(&headers) {
        Ok(p) => p,
        Err((status, code, msg)) => return Ok(error_response(status, code, msg, None)),
    };
    let perm = match pre {
        Precondition::Matches(_) => Perms::EDIT,
        Precondition::Absent => Perms::UPLOAD,
    };
    let storage: Arc<dyn Storage> = scoped(&state, &auth.user, &id, perm, &vp).await?;
    // Lock on the real location so two users with different scopes that
    // reach the same file still take the same lock.
    let real = auth
        .user
        .grant(&id)
        .map(|g| g.scope.join_path(&vp))
        .unwrap_or_else(|| vp.clone());
    if body.len() as u64 > MAX_TEXT_BYTES {
        return Err(AppError::PayloadTooLarge);
    }
    if std::str::from_utf8(&body).is_err() || body.contains(&0) {
        return Err(AppError::bad_request("the content must be UTF-8 text"));
    }
    let new_tag = etag_for(&body);

    let _guard = lock_for(&id, &real).lock().await;
    let current = match read_current(storage.as_ref(), &vp).await {
        Ok(c) => c,
        // An existing file we would refuse to open counts as "changed".
        Err(AppError::PayloadTooLarge) => return Ok(conflict(Some(String::new()))),
        Err(e) => return Err(e),
    };
    let current_tag = current.as_ref().map(|(_, b)| etag_for(b));
    match (&pre, &current_tag) {
        (Precondition::Matches(tags), Some(cur)) if tags.contains(cur) => {}
        (Precondition::Absent, None) => {}
        _ => return Ok(conflict(current_tag)),
    }
    let overwrite = matches!(pre, Precondition::Matches(_));
    let size = body.len() as u64;
    match storage
        .write(&vp, Box::pin(Cursor::new(body)), overwrite)
        .await
    {
        Ok(_) => {}
        // Created by someone else between our check and the write.
        Err(StorageError::AlreadyExists) => return Ok(conflict(None)),
        Err(e) => return Err(e.into()),
    }
    let modified = storage.stat(&vp).await.ok().and_then(|e| e.modified);
    let status = if overwrite {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    let body = Saved {
        path: vp.to_string(),
        etag: new_tag.clone(),
        size,
        modified,
    };
    Ok(with_etag((status, Json(body)).into_response(), &new_tag))
}

fn conflict(current: Option<String>) -> Response {
    let current = current.filter(|t| !t.is_empty());
    error_response(
        StatusCode::PRECONDITION_FAILED,
        "precondition_failed",
        if current.is_some() {
            "the file was changed since you opened it"
        } else {
            "the file was created or deleted since you opened it"
        },
        Some(current),
    )
}

fn error_response(
    status: StatusCode,
    code: &str,
    message: &str,
    etag: Option<Option<String>>,
) -> Response {
    let mut body = json!({ "error": code, "message": message });
    if let Some(tag) = etag {
        body["etag"] = json!(tag);
    }
    (status, Json(body)).into_response()
}

fn with_etag(mut resp: Response, etag: &str) -> Response {
    if let Ok(v) = HeaderValue::from_str(etag) {
        resp.headers_mut().insert(header::ETAG, v);
    }
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// The lock stripe for one file. Different files may share a stripe, which
/// only costs a little concurrency; the same file always maps to the same one.
fn lock_for(source: &str, path: &VPath) -> &'static Mutex<()> {
    static LOCKS: OnceLock<Vec<Mutex<()>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| (0..LOCK_STRIPES).map(|_| Mutex::new(())).collect());
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (source, path).hash(&mut h);
    &locks[(h.finish() as usize) % LOCK_STRIPES]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hm(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(k.clone(), HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn etag_is_stable_and_quoted() {
        let a = etag_for(b"hello");
        assert_eq!(a, etag_for(b"hello"));
        assert_ne!(a, etag_for(b"hello!"));
        assert!(a.starts_with('"') && a.ends_with('"'));
        assert_eq!(a.len(), 34);
    }

    #[test]
    fn text_detection() {
        assert_eq!(as_text(b"caf\xc3\xa9\r\n".to_vec()).unwrap(), "café\r\n");
        assert!(as_text(vec![0xff, 0xfe]).is_err());
        assert!(as_text(b"a\0b".to_vec()).is_err());
    }

    #[test]
    fn preconditions() {
        assert_eq!(
            precondition(&hm(&[(header::IF_MATCH, "\"a\", \"b\"")])).unwrap(),
            Precondition::Matches(vec!["\"a\"".into(), "\"b\"".into()])
        );
        assert_eq!(
            precondition(&hm(&[(header::IF_NONE_MATCH, "*")])).unwrap(),
            Precondition::Absent
        );
        let status = |h: HeaderMap| precondition(&h).unwrap_err().0;
        assert_eq!(status(HeaderMap::new()), StatusCode::PRECONDITION_REQUIRED);
        assert_eq!(
            status(hm(&[(header::IF_MATCH, "W/\"a\"")])),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status(hm(&[(header::IF_MATCH, "*")])),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status(hm(&[(header::IF_NONE_MATCH, "\"a\"")])),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn same_file_same_lock() {
        let p = VPath::parse("a/b.txt").unwrap();
        assert!(std::ptr::eq(lock_for("s", &p), lock_for("s", &p)));
    }
}
