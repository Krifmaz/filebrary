//! Browsing and file operations on a source.
//!
//! All routes take user-visible paths (relative to the user's scope in the
//! source) and check the needed permission before touching storage.
//!
//! | Route | Permission |
//! |---|---|
//! | `GET /api/sources` | — |
//! | `GET /api/sources/{id}/list?path=` | view |
//! | `GET /api/sources/{id}/stat?path=` | view |
//! | `GET /api/sources/{id}/download?path=[&inline=1]` | download |
//! | `GET /api/sources/{id}/zip?path=a&path=b[&name=]` | download |
//! | `POST /api/sources/{id}/mkdir {path}` | upload |
//! | `POST /api/sources/{id}/move {from,to,overwrite?}` | edit |
//! | `POST /api/sources/{id}/copy {from,to,overwrite?}` | upload (+edit to overwrite) |
//! | `POST /api/sources/{id}/delete {paths}` | delete (moves to the trash) |
//!
//! The source's trash folder ([`trash::TRASH_DIR`]) is hidden from listings
//! and refused as a path. Moves, copies and deletes keep tags and stars
//! attached ([`meta`]) and every change is recorded in the activity log.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    routing::{get, post},
};
use axum_extra::extract::Query;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::{download, zip};
use crate::{
    activity::{Activity, ClientIp, action},
    auth::{AuthUser, Perms, User},
    error::{AppError, AppResult},
    organize::{
        self, meta,
        trash::{self, Removed},
    },
    state::AppState,
    storage::{Entry, Source, Storage, VPath},
};

/// Routes for this module, merged under `/api`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sources", get(list_sources))
        .route("/sources/{id}/list", get(list))
        .route("/sources/{id}/stat", get(stat))
        .route("/sources/{id}/download", get(download_file))
        .route("/sources/{id}/zip", get(zip_selection))
        .route("/sources/{id}/mkdir", post(mkdir))
        .route("/sources/{id}/move", post(move_entry))
        .route("/sources/{id}/copy", post(copy_entry))
        .route("/sources/{id}/delete", post(delete))
}

/// Permission flags as booleans, for the UI.
#[derive(Debug, Serialize)]
pub struct PermissionFlags {
    view: bool,
    download: bool,
    upload: bool,
    edit: bool,
    delete: bool,
    share: bool,
}

impl From<Perms> for PermissionFlags {
    fn from(p: Perms) -> Self {
        Self {
            view: p.contains(Perms::VIEW),
            download: p.contains(Perms::DOWNLOAD),
            upload: p.contains(Perms::UPLOAD),
            edit: p.contains(Perms::EDIT),
            delete: p.contains(Perms::DELETE),
            share: p.contains(Perms::SHARE),
        }
    }
}

/// A source the user can see.
#[derive(Debug, Serialize)]
pub struct SourceInfo {
    id: String,
    name: String,
    permissions: PermissionFlags,
}

/// A file or folder as returned to clients.
#[derive(Debug, Serialize)]
pub struct EntryDto {
    /// Final path component.
    pub name: String,
    /// User-visible path.
    pub path: VPath,
    /// Folder or not.
    pub is_dir: bool,
    /// Size in bytes (0 for folders).
    pub size: u64,
    /// Last modification time (RFC 3339).
    #[serde(with = "time::serde::rfc3339::option")]
    pub modified: Option<OffsetDateTime>,
    /// Guessed MIME type for files.
    pub mime: Option<String>,
}

impl EntryDto {
    /// Converts a storage entry, rewriting its path to `visible`.
    pub fn from_entry(e: Entry, visible: VPath) -> Self {
        Self {
            is_dir: e.is_dir(),
            name: e.name,
            path: visible,
            size: e.size,
            modified: e.modified,
            mime: e.mime,
        }
    }
}

/// `path` query parameter.
#[derive(Debug, Deserialize)]
pub struct PathQuery {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    inline: Option<String>,
}

impl PathQuery {
    fn vpath(&self) -> AppResult<VPath> {
        Ok(VPath::parse(self.path.as_deref().unwrap_or("/"))?)
    }
}

/// Resolves a source the user can view.
pub fn source_for<'a>(state: &'a AppState, user: &User, id: &str) -> AppResult<&'a Source> {
    let src = state.source(id).ok_or(AppError::NotFound)?;
    match user.grant(id) {
        Some(g) if g.perms.contains(Perms::VIEW) => Ok(src),
        _ => Err(AppError::NotFound),
    }
}

/// Checks `perm` on `path` and returns the source's backend confined to the
/// user's scope. Paths given to the returned backend are user-visible paths.
///
/// Confining through [`crate::storage::Storage::sub`] (instead of prefixing the
/// scope) makes the backend check symlinks against the scope, so a link inside
/// a user's folder cannot reach elsewhere in the source.
///
/// # Errors
///
/// `NotFound` if the source is invisible to the user, `Forbidden` without `perm`.
pub async fn scoped(
    state: &AppState,
    user: &User,
    id: &str,
    perm: Perms,
    path: &VPath,
) -> AppResult<Arc<dyn Storage>> {
    let src = source_for(state, user, id)?;
    if trash::is_reserved(&user.require(id, perm, path)?) {
        return Err(AppError::NotFound);
    }
    let grant = user.grant(id).ok_or(AppError::NotFound)?;
    if grant.scope.is_root() {
        Ok(src.storage.clone())
    } else {
        Ok(src.storage.sub(&grant.scope).await?)
    }
}

async fn list_sources(State(state): State<AppState>, auth: AuthUser) -> Json<Vec<SourceInfo>> {
    let list = state
        .sources
        .keys()
        .filter_map(|name| {
            let g = auth.user.grant(name)?;
            g.perms.contains(Perms::VIEW).then(|| SourceInfo {
                id: name.clone(),
                name: name.clone(),
                permissions: g.perms.into(),
            })
        })
        .collect();
    Json(list)
}

/// Listing response.
#[derive(Debug, Serialize)]
pub struct Listing {
    path: VPath,
    entries: Vec<EntryDto>,
}

async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
    Query(q): Query<PathQuery>,
) -> AppResult<Json<Listing>> {
    let path = q.vpath()?;
    let st = scoped(&state, &auth.user, &id, Perms::VIEW, &path).await?;
    let real_root = auth.user.require(&id, Perms::VIEW, &path)?.is_root();
    let mut entries: Vec<EntryDto> = st
        .list(&path)
        .await?
        .into_iter()
        // The trash folder at the top of the source is never listed.
        .filter(|e| !(e.name == trash::TRASH_DIR && real_root))
        .map(|e| {
            let visible = path.join(&e.name).unwrap_or_else(|_| path.clone());
            EntryDto::from_entry(e, visible)
        })
        .collect();
    // Folders first, then case-insensitive by name.
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(Json(Listing { path, entries }))
}

async fn stat(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
    Query(q): Query<PathQuery>,
) -> AppResult<Json<EntryDto>> {
    let path = q.vpath()?;
    let st = scoped(&state, &auth.user, &id, Perms::VIEW, &path).await?;
    let e = st.stat(&path).await?;
    Ok(Json(EntryDto::from_entry(e, path)))
}

async fn download_file(
    State(state): State<AppState>,
    auth: AuthUser,
    ip: ClientIp,
    Path(id): Path<String>,
    Query(q): Query<PathQuery>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let path = q.vpath()?;
    let st = scoped(&state, &auth.user, &id, Perms::DOWNLOAD, &path).await?;
    let inline = q.inline.as_deref().is_some_and(|v| v == "1" || v == "true");
    let resp = download::file_response(st.as_ref(), &path, &headers, inline).await?;
    // Log a download once, not for every seek of a media player.
    let first_part = headers
        .get(axum::http::header::RANGE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|r| r.trim_start().starts_with("bytes=0-"));
    if first_part && resp.status().is_success() {
        Activity::new(action::DOWNLOAD)
            .user(&auth.user)
            .at(&id, &auth.user.require(&id, Perms::DOWNLOAD, &path)?)
            .details(serde_json::json!({ "inline": inline }))
            .ip(ip.0.as_deref())
            .record(&state)
            .await;
    }
    Ok(resp)
}

/// Query for `zip`: repeated `path` and an optional archive `name`.
#[derive(Debug, Deserialize)]
pub struct ZipQuery {
    #[serde(default)]
    path: Vec<String>,
    #[serde(default)]
    name: Option<String>,
}

async fn zip_selection(
    State(state): State<AppState>,
    auth: AuthUser,
    ip: ClientIp,
    Path(id): Path<String>,
    Query(q): Query<ZipQuery>,
) -> AppResult<Response> {
    if q.path.len() > 10_000 {
        return Err(AppError::bad_request("too many items"));
    }
    let st = scoped(&state, &auth.user, &id, Perms::DOWNLOAD, &VPath::root()).await?;
    let items = q
        .path
        .iter()
        .map(|p| VPath::parse(p))
        .collect::<Result<Vec<_>, _>>()?;
    let mut reals = Vec::with_capacity(items.len());
    for item in &items {
        let real = auth.user.require(&id, Perms::DOWNLOAD, item)?;
        if trash::is_reserved(&real) {
            return Err(AppError::NotFound);
        }
        reals.push(real.to_string());
    }
    let default_name = match items.as_slice() {
        [one] => one.name().unwrap_or(&id).to_owned(),
        _ => "download".to_owned(),
    };
    let name = q
        .name
        .filter(|n| !n.is_empty() && n.len() < 200 && !n.contains(['/', '\\']))
        .unwrap_or(default_name);
    let resp = zip::zip_response(st, items, &name).await?;
    Activity::new(action::DOWNLOAD)
        .user(&auth.user)
        .details(serde_json::json!({ "source": id, "zip": reals }))
        .ip(ip.0.as_deref())
        .record(&state)
        .await;
    Ok(resp)
}

/// Body for `mkdir`.
#[derive(Debug, Deserialize)]
pub struct MkdirBody {
    path: VPath,
}

async fn mkdir(
    State(state): State<AppState>,
    auth: AuthUser,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(body): Json<MkdirBody>,
) -> AppResult<Json<EntryDto>> {
    if body.path.is_root() {
        return Err(AppError::bad_request("folder name required"));
    }
    let st = scoped(&state, &auth.user, &id, Perms::UPLOAD, &body.path).await?;
    if st.stat(&body.path).await.is_ok() {
        return Err(AppError::conflict("the target already exists"));
    }
    st.mkdir(&body.path).await?;
    let e = st.stat(&body.path).await?;
    Activity::new(action::MKDIR)
        .user(&auth.user)
        .at(&id, &auth.user.require(&id, Perms::UPLOAD, &body.path)?)
        .ip(ip.0.as_deref())
        .record(&state)
        .await;
    Ok(Json(EntryDto::from_entry(e, body.path)))
}

/// Body for `move` and `copy`.
#[derive(Debug, Deserialize)]
pub struct TransferBody {
    from: VPath,
    to: VPath,
    #[serde(default)]
    overwrite: bool,
}

async fn move_entry(
    State(state): State<AppState>,
    auth: AuthUser,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(body): Json<TransferBody>,
) -> AppResult<Json<EntryDto>> {
    let st = scoped(&state, &auth.user, &id, Perms::EDIT, &body.from).await?;
    let from = auth.user.require(&id, Perms::EDIT, &body.from)?;
    let to = auth.user.require(&id, Perms::EDIT, &body.to)?;
    if trash::is_reserved(&to) {
        return Err(AppError::NotFound);
    }
    st.rename(&body.from, &body.to, body.overwrite).await?;
    meta::on_move(&state.db, &id, &from, &id, &to).await?;
    Activity::new(action::MOVE)
        .user(&auth.user)
        .at(&id, &from)
        .target(&to)
        .ip(ip.0.as_deref())
        .record(&state)
        .await;
    let e = st.stat(&body.to).await?;
    Ok(Json(EntryDto::from_entry(e, body.to)))
}

async fn copy_entry(
    State(state): State<AppState>,
    auth: AuthUser,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(body): Json<TransferBody>,
) -> AppResult<Json<EntryDto>> {
    let st = scoped(&state, &auth.user, &id, Perms::DOWNLOAD, &body.from).await?;
    let need = if body.overwrite {
        Perms::UPLOAD | Perms::EDIT
    } else {
        Perms::UPLOAD
    };
    let from = auth.user.require(&id, Perms::DOWNLOAD, &body.from)?;
    let to = auth.user.require(&id, need, &body.to)?;
    if trash::is_reserved(&to) {
        return Err(AppError::NotFound);
    }
    st.copy(&body.from, &body.to, body.overwrite).await?;
    if body.overwrite {
        // The replaced item's tags and stars go with it.
        meta::on_delete(&state.db, &id, &to).await?;
    }
    meta::on_copy(&state.db, &id, &from, &id, &to).await?;
    Activity::new(action::COPY)
        .user(&auth.user)
        .at(&id, &from)
        .target(&to)
        .ip(ip.0.as_deref())
        .record(&state)
        .await;
    let e = st.stat(&body.to).await?;
    Ok(Json(EntryDto::from_entry(e, body.to)))
}

/// Body for `delete`.
#[derive(Debug, Deserialize)]
pub struct DeleteBody {
    paths: Vec<VPath>,
}

/// One failed deletion.
#[derive(Debug, Serialize)]
pub struct DeleteFailure {
    path: VPath,
    message: String,
}

/// Result of a batch delete.
#[derive(Debug, Serialize)]
pub struct DeleteResult {
    deleted: Vec<VPath>,
    failed: Vec<DeleteFailure>,
}

async fn delete(
    State(state): State<AppState>,
    auth: AuthUser,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(body): Json<DeleteBody>,
) -> AppResult<Json<DeleteResult>> {
    // Check permissions first so a batch is not half-applied on a 403.
    if body.paths.iter().any(VPath::is_root) {
        return Err(AppError::bad_request("cannot delete the root"));
    }
    scoped(&state, &auth.user, &id, Perms::DELETE, &VPath::root()).await?;
    let mut result = DeleteResult {
        deleted: Vec::new(),
        failed: Vec::new(),
    };
    for visible in body.paths {
        // Deletes go to the trash (or are permanent when it is off). The
        // path is confined first: the trash lives outside the user's scope.
        let outcome = async {
            let real = organize::confine(&state, &auth.user, &id, Perms::DELETE, &visible).await?;
            let removed = trash::remove(&state, Some(auth.user.id), &id, &real).await?;
            AppResult::Ok((real, removed))
        }
        .await;
        match outcome {
            Ok((real, removed)) => {
                let ev = match &removed {
                    Removed::Trashed(item) => Activity::new(action::TRASH)
                        .details(serde_json::json!({ "trash_id": item.id })),
                    Removed::Deleted => Activity::new(action::DELETE),
                };
                ev.user(&auth.user)
                    .at(&id, &real)
                    .ip(ip.0.as_deref())
                    .record(&state)
                    .await;
                result.deleted.push(visible);
            }
            Err(e) => result.failed.push(DeleteFailure {
                path: visible,
                message: e.to_string(),
            }),
        }
    }
    Ok(Json(result))
}
