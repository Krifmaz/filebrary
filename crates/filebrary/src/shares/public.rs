//! Anonymous access to shares, nested at `/api/public/shares`.
//!
//! | Method | Path                 | Purpose                                      |
//! |--------|----------------------|----------------------------------------------|
//! | GET    | `/{token}`           | describe the share or list a folder (`?path=`) |
//! | POST   | `/{token}/unlock`    | exchange the password for an access grant    |
//! | GET    | `/{token}/download`  | download a file, or a folder as zip (`?path=`) |
//! | POST   | `/{token}/uploads`   | drop links: upload one file (`?path=name`)   |
//!
//! Paths are *share-relative*. For a share of one folder they are relative to
//! that folder; for a share of several items the first segment is the item's
//! name; a share of one file only knows the empty path.
//!
//! Password-protected shares accept an access grant from the `fb_share_{token}`
//! cookie, the `X-Share-Access` header or the `access` query parameter.
//!
//! Download limits: when a share has `max_downloads`, every download request
//! counts, including `Range` requests (otherwise a client could fetch an
//! entire file in ranges for free). Shares without a limit only count
//! requests that start at byte 0, so the counter stays meaningful.

use std::sync::Arc;

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use axum_extra::extract::{
    CookieJar,
    cookie::{Cookie, SameSite},
};
use futures::TryStreamExt;
use serde::Deserialize;
use tokio_util::io::StreamReader;

use super::{
    error::{ShareError, ShareResult},
    model::{
        MAX_PASSWORD_LEN, PublicEntry, PublicShareResponse, UnlockRequest, UnlockResponse,
        format_ts,
    },
    ratelimit,
    store::{ACCESS_TTL_SECS, Share, ShareStore, now},
};
use crate::{
    api::{download, zip},
    auth::{self, Perms},
    error::AppError,
    organize::trash,
    state::AppState,
    storage::{Entry, Storage, StorageError, VPath},
};

/// Header carrying an access grant for clients that do not keep cookies.
pub const ACCESS_HEADER: &str = "x-share-access";

/// Routes for `/api/public/shares`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{token}", get(info))
        .route("/{token}/unlock", post(unlock))
        .route("/{token}/download", get(download))
        .route("/{token}/uploads", post(upload))
}

/// Name of the cookie holding the access grant for one share.
pub fn cookie_name(token: &str) -> String {
    format!("fb_share_{token}")
}

/// One shared item, resolved to the storage that holds it.
///
/// Storage is always *confined*: `scoped` is the source as the share's owner
/// sees it (rooted at their scope), and `storage` is, for a shared folder, a
/// backend rooted at that folder. The backend checks symlinks against its own
/// root, so a link inside a shared folder cannot reach anything outside it.
#[derive(Clone)]
pub struct ShareItem {
    /// Source name.
    pub source: String,
    /// The item's name, used as its top-level name in multi-item shares.
    pub name: String,
    /// Backend confined to the item (folders) or to the owner's scope (files).
    pub storage: Arc<dyn Storage>,
    /// Path of the item inside `storage` (the root for folders).
    pub path: VPath,
    /// Backend confined to the owner's scope.
    pub scoped: Arc<dyn Storage>,
    /// Path of the item inside `scoped`.
    pub visible: VPath,
    /// Path of the item inside the source itself.
    pub real: VPath,
}

/// A share that is alive and usable right now.
pub struct LiveShare {
    /// The database row.
    pub share: Share,
    /// What it exposes.
    pub items: Vec<ShareItem>,
}

/// Where a share-relative path points.
pub enum Target {
    /// The virtual folder listing the items of a multi-item share.
    Root,
    /// A real path inside one item.
    Item {
        /// The item.
        item: ShareItem,
        /// Path inside the item's (confined) storage.
        path: VPath,
        /// The same location as a path inside the source itself.
        real: VPath,
    },
}

impl LiveShare {
    /// Maps a share-relative path to a target. Unknown names, and anything
    /// inside the source's trash folder, are `NotFound`.
    pub fn locate(&self, rel: &VPath) -> ShareResult<Target> {
        let (item, inner) = if let [only] = self.items.as_slice() {
            (only, rel.clone())
        } else {
            let Some((first, rest)) = rel.components().split_first() else {
                return Ok(Target::Root);
            };
            let item = self
                .items
                .iter()
                .find(|i| &i.name == first)
                .ok_or_else(ShareError::not_found)?;
            (item, VPath::parse(&rest.join("/"))?)
        };
        let real = item.real.join_path(&inner);
        if trash::is_reserved(&real) {
            return Err(ShareError::not_found());
        }
        Ok(Target::Item {
            item: item.clone(),
            path: item.path.join_path(&inner),
            real,
        })
    }

    /// Display name of the share as a whole.
    pub fn name(&self) -> String {
        match self.items.as_slice() {
            [only] => only.name.clone(),
            _ => "Shared files".to_owned(),
        }
    }
}

/// Loads a share by token and checks it is still alive: not expired, owner
/// still enabled and still holding the rights the share hands out. Builds the
/// confined storage for each item. (Collections hook in here.)
pub async fn resolve(state: &AppState, token: &str) -> ShareResult<LiveShare> {
    let share = ShareStore::new(state.db.clone())
        .get_by_token(token)
        .await?
        .ok_or_else(ShareError::not_found)?;
    if share.is_expired(now()) {
        return Err(ShareError::not_found());
    }
    let owner = auth::load_user(state, share.owner_id)
        .await?
        .ok_or_else(ShareError::not_found)?;
    let needed = Perms::SHARE
        | if share.upload_only {
            Perms::UPLOAD
        } else {
            Perms::DOWNLOAD
        };
    let source = state
        .source(&share.source)
        .ok_or_else(ShareError::not_found)?;
    let grant = owner
        .grant(&share.source)
        .ok_or_else(ShareError::not_found)?;
    let scoped: Arc<dyn Storage> = if grant.scope.is_root() {
        source.storage.clone()
    } else {
        match source.storage.sub(&grant.scope).await {
            Ok(s) => s,
            Err(StorageError::NotFound | StorageError::NotADirectory) => {
                return Err(ShareError::not_found());
            }
            Err(e) => return Err(e.into()),
        }
    };
    let mut items = Vec::with_capacity(share.paths.len());
    for real in &share.paths {
        let visible = owner
            .visible_path(&share.source, real)
            .ok_or_else(ShareError::not_found)?;
        if owner.require(&share.source, needed, &visible).is_err() {
            return Err(ShareError::not_found());
        }
        // Folders get their own root; files (and vanished items, which will
        // answer 404 when used) are reached through the owner's scope.
        let (storage, path) = match scoped.sub(&visible).await {
            Ok(sub) => (sub, VPath::root()),
            Err(StorageError::NotADirectory | StorageError::NotFound) => {
                (scoped.clone(), visible.clone())
            }
            Err(e) => return Err(e.into()),
        };
        items.push(ShareItem {
            source: share.source.clone(),
            name: real.name().unwrap_or(&source.name).to_owned(),
            storage,
            path,
            scoped: scoped.clone(),
            visible,
            real: real.clone(),
        });
    }
    Ok(LiveShare { share, items })
}

/// Query parameters shared by the public routes.
#[derive(Debug, Default, Deserialize)]
pub struct PublicQuery {
    /// Share-relative path.
    #[serde(default)]
    pub path: String,
    /// Access grant, for clients without cookies.
    #[serde(default)]
    pub access: Option<String>,
    /// Ask to display a file instead of downloading it.
    #[serde(default)]
    pub inline: bool,
}

/// Checks the password gate. Shares without a password are always open.
async fn is_unlocked(
    state: &AppState,
    live: &LiveShare,
    jar: &CookieJar,
    headers: &HeaderMap,
    q: &PublicQuery,
) -> ShareResult<bool> {
    if live.share.password_hash.is_none() {
        return Ok(true);
    }
    let presented = jar
        .get(&cookie_name(&live.share.token))
        .map(|c| c.value().to_owned())
        .or_else(|| {
            headers
                .get(ACCESS_HEADER)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        })
        .or_else(|| q.access.clone());
    match presented {
        Some(a) => Ok(ShareStore::new(state.db.clone())
            .check_access(live.share.id, &a)
            .await?),
        None => Ok(false),
    }
}

async fn require_unlocked(
    state: &AppState,
    live: &LiveShare,
    jar: &CookieJar,
    headers: &HeaderMap,
    q: &PublicQuery,
) -> ShareResult<()> {
    if is_unlocked(state, live, jar, headers, q).await? {
        Ok(())
    } else {
        Err(ShareError::PasswordRequired)
    }
}

fn public_entry(e: &Entry, rel: &VPath) -> PublicEntry {
    let path = rel.join(&e.name).map(|p| p.to_string()).unwrap_or_default();
    PublicEntry {
        name: e.name.clone(),
        path: path.trim_start_matches('/').to_owned(),
        is_dir: e.is_dir(),
        size: if e.is_dir() { 0 } else { e.size },
        modified: e.modified.map(|m| format_ts(m.unix_timestamp())),
    }
}

async fn info(
    State(st): State<AppState>,
    Path(token): Path<String>,
    Query(q): Query<PublicQuery>,
    jar: CookieJar,
    headers: HeaderMap,
) -> ShareResult<Json<PublicShareResponse>> {
    let live = resolve(&st, &token).await?;
    let expires_at = live.share.expires_at.map(format_ts);
    if !is_unlocked(&st, &live, &jar, &headers, &q).await? {
        // Reveal nothing about the content until the password is given.
        return Ok(Json(PublicShareResponse {
            name: String::new(),
            is_dir: false,
            size: None,
            upload_only: live.share.upload_only,
            needs_password: true,
            expires_at,
            entries: None,
        }));
    }
    if live.share.upload_only {
        return Ok(Json(PublicShareResponse {
            name: live.name(),
            is_dir: true,
            size: None,
            upload_only: true,
            needs_password: false,
            expires_at,
            entries: None,
        }));
    }
    let rel = VPath::parse(&q.path)?;
    let (name, is_dir, size, entries) = match live.locate(&rel)? {
        Target::Root => {
            let mut entries = Vec::with_capacity(live.items.len());
            for item in &live.items {
                match item.storage.stat(&item.path).await {
                    Ok(mut e) => {
                        e.name = item.name.clone();
                        entries.push(public_entry(&e, &rel));
                    }
                    Err(StorageError::NotFound) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            (live.name(), true, None, Some(entries))
        }
        Target::Item { item, path, real } => {
            let entry = item.storage.stat(&path).await?;
            let name = if rel.is_root() {
                live.name()
            } else {
                entry.name.clone()
            };
            if entry.is_dir() {
                let mut list = item.storage.list(&path).await?;
                // Never show the source's trash folder.
                if real.is_root() {
                    list.retain(|e| e.name != trash::TRASH_DIR);
                }
                list.sort_by(|a, b| {
                    b.is_dir()
                        .cmp(&a.is_dir())
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
                let entries = list.iter().map(|e| public_entry(e, &rel)).collect();
                (name, true, None, Some(entries))
            } else {
                (name, false, Some(entry.size), None)
            }
        }
    };
    Ok(Json(PublicShareResponse {
        name,
        is_dir,
        size,
        upload_only: false,
        needs_password: false,
        expires_at,
        entries,
    }))
}

async fn unlock(
    State(st): State<AppState>,
    Path(token): Path<String>,
    jar: CookieJar,
    Json(req): Json<UnlockRequest>,
) -> ShareResult<impl IntoResponse> {
    let live = resolve(&st, &token).await?;
    let Some(hash) = live.share.password_hash.clone() else {
        return Err(ShareError::bad_request("this share has no password"));
    };
    let now = now();
    let window = i64::try_from(st.config.login_window_secs).unwrap_or(i64::MAX);
    if let Some(retry_after) =
        ratelimit::check(&live.share.token, st.config.login_max_attempts, window, now)
    {
        return Err(AppError::TooManyRequests { retry_after }.into());
    }
    if req.password.len() > MAX_PASSWORD_LEN || !auth::password::verify(req.password, hash).await {
        ratelimit::fail(&live.share.token, window, now);
        return Err(ShareError::WrongPassword);
    }
    ratelimit::clear(&live.share.token);
    let access = ShareStore::new(st.db.clone())
        .grant_access(live.share.id)
        .await?;
    let cookie = Cookie::build((cookie_name(&live.share.token), access.clone()))
        .path("/")
        .http_only(true)
        .secure(st.config.cookies_secure())
        .same_site(SameSite::Lax)
        .max_age(time::Duration::seconds(ACCESS_TTL_SECS))
        .build();
    Ok((jar.add(cookie), Json(UnlockResponse { access })))
}

async fn download(
    State(st): State<AppState>,
    Path(token): Path<String>,
    Query(q): Query<PublicQuery>,
    jar: CookieJar,
    headers: HeaderMap,
) -> ShareResult<Response> {
    let live = resolve(&st, &token).await?;
    if live.share.upload_only {
        return Err(AppError::forbidden("this is an upload-only link").into());
    }
    require_unlocked(&st, &live, &jar, &headers, &q).await?;
    let rel = VPath::parse(&q.path)?;
    let target = live.locate(&rel)?;

    // Resolve what will be sent before counting, so a bad path costs nothing.
    let file = match &target {
        Target::Item { item, path, .. } => {
            let e = item.storage.stat(path).await?;
            (!e.is_dir()).then_some(e)
        }
        Target::Root => None,
    };
    let counts = live.share.max_downloads.is_some() || !is_continuation(&headers);
    if counts
        && !ShareStore::new(st.db.clone())
            .record_download(live.share.id)
            .await?
    {
        return Err(ShareError::Exhausted);
    }

    match (target, file) {
        (Target::Item { item, path, .. }, Some(_)) => {
            Ok(download::file_response(item.storage.as_ref(), &path, &headers, q.inline).await?)
        }
        (Target::Item { item, path, real }, None) => {
            let name = if rel.is_root() {
                live.name()
            } else {
                path.name().unwrap_or("download").to_owned()
            };
            if path.is_root() {
                // The whole shared folder: archive its children under its name.
                let children = item.storage.list(&path).await?;
                let mut paths: Vec<VPath> = children
                    .into_iter()
                    .filter(|c| !(real.is_root() && c.name == trash::TRASH_DIR))
                    .map(|c| c.path)
                    .collect();
                paths.sort();
                if paths.is_empty() {
                    // An empty folder: archive it through the owner's scope.
                    return Ok(zip::zip_response(item.scoped, vec![item.visible], &name).await?);
                }
                Ok(zip::zip_response(item.storage, paths, &name).await?)
            } else {
                Ok(zip::zip_response(item.storage, vec![path], &name).await?)
            }
        }
        (Target::Root, _) => {
            let Some(first) = live.items.first() else {
                return Err(ShareError::not_found());
            };
            let paths = live.items.iter().map(|i| i.visible.clone()).collect();
            Ok(zip::zip_response(first.scoped.clone(), paths, "shared-files").await?)
        }
    }
}

/// Returns whether a request asks for a range that does not start at byte 0.
fn is_continuation(headers: &HeaderMap) -> bool {
    headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .is_some_and(|s| !s.trim_start().starts_with("0-"))
}

/// Splits `name.ext` into (`name`, `.ext`) for numbering duplicates.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

/// Picks a free name in `dir`: `name`, then `name (1)`, `name (2)`...
async fn free_name(storage: &dyn Storage, dir: &VPath, name: &str) -> ShareResult<VPath> {
    let (stem, ext) = split_ext(name);
    for n in 0..1000u32 {
        let candidate = if n == 0 {
            name.to_owned()
        } else {
            format!("{stem} ({n}){ext}")
        };
        let path = dir.join(&candidate)?;
        match storage.stat(&path).await {
            Err(StorageError::NotFound) => return Ok(path),
            Ok(_) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(AppError::conflict("too many files with this name").into())
}

async fn upload(
    State(st): State<AppState>,
    Path(token): Path<String>,
    Query(q): Query<PublicQuery>,
    jar: CookieJar,
    headers: HeaderMap,
    body: Body,
) -> ShareResult<Response> {
    let live = resolve(&st, &token).await?;
    if !live.share.upload_only {
        return Err(AppError::forbidden("this link does not accept uploads").into());
    }
    require_unlocked(&st, &live, &jar, &headers, &q).await?;

    // Visitors choose a file name, never a folder.
    let name = VPath::parse(&q.path)?;
    let [file_name] = name.components() else {
        return Err(ShareError::bad_request("path must be a single file name"));
    };
    let Target::Item {
        item, path: dir, ..
    } = live.locate(&VPath::root())?
    else {
        return Err(ShareError::not_found());
    };

    let limit = st.config.max_drop_upload_mib.saturating_mul(1024 * 1024);
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if limit > 0 && declared.is_some_and(|n| n > limit) {
        return Err(AppError::PayloadTooLarge.into());
    }

    let target = free_name(item.storage.as_ref(), &dir, file_name).await?;
    let mut seen: u64 = 0;
    let stream = body
        .into_data_stream()
        .map_err(std::io::Error::other)
        .and_then(move |chunk| {
            seen += chunk.len() as u64;
            let res = if limit > 0 && seen > limit {
                Err(std::io::Error::new(
                    std::io::ErrorKind::FileTooLarge,
                    "upload exceeds the size limit",
                ))
            } else {
                Ok(chunk)
            };
            std::future::ready(res)
        });
    let reader = Box::pin(StreamReader::new(stream));
    let written = match item.storage.write(&target, reader, false).await {
        Ok(n) => n,
        Err(StorageError::Io(e)) if e.kind() == std::io::ErrorKind::FileTooLarge => {
            return Err(AppError::PayloadTooLarge.into());
        }
        Err(StorageError::AlreadyExists) => {
            return Err(AppError::conflict("a file with this name was just uploaded").into());
        }
        Err(e) => return Err(e.into()),
    };
    tracing::info!(
        share = live.share.id,
        bytes = written,
        "file dropped into share"
    );
    let stored = target.name().unwrap_or_default().to_owned();
    let mut resp = (
        StatusCode::CREATED,
        Json(serde_json::json!({ "name": stored, "size": written })),
    )
        .into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::split_ext;

    #[test]
    fn splits_extensions() {
        assert_eq!(split_ext("a.txt"), ("a", ".txt"));
        assert_eq!(split_ext("a.tar.gz"), ("a.tar", ".gz"));
        assert_eq!(split_ext(".env"), (".env", ""));
        assert_eq!(split_ext("README"), ("README", ""));
    }
}
