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
//! | `POST /api/sources/{id}/delete {paths}` | delete |

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
    auth::{AuthUser, Perms, User},
    error::{AppError, AppResult},
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
    // A grant exists only when some rule gives the user something on the
    // source (possibly only in a subfolder); paths are checked per request.
    match user.grant(id) {
        Some(_) => Ok(src),
        None => Err(AppError::NotFound),
    }
}

/// Checks `perm` on `path` and returns the source's backend confined to the
/// user's scope. Paths given to the returned backend are user-visible paths.
///
/// Confining through [`crate::storage::Storage::sub`] (instead of prefixing the
/// scope) makes the backend check symlinks against the scope, so a link inside
/// a user's folder cannot reach elsewhere in the source. The permission check
/// itself is [`crate::auth::authorize`], which also checks the rules on the
/// symlink-resolved path.
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
    crate::auth::authorize(state, user, id, perm, path).await?;
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
    let mut entries: Vec<EntryDto> = st
        .list(&path)
        .await?
        .into_iter()
        .filter_map(|e| {
            let visible = path.join(&e.name).ok()?;
            // Hide entries a permission rule hides.
            let shown = if e.is_dir() {
                auth.user.can_traverse(&id, &visible)
            } else {
                auth.user.effective(&id, &visible).contains(Perms::VIEW)
            };
            shown.then(|| EntryDto::from_entry(e, visible))
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
    Path(id): Path<String>,
    Query(q): Query<PathQuery>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let path = q.vpath()?;
    let st = scoped(&state, &auth.user, &id, Perms::DOWNLOAD, &path).await?;
    let inline = q.inline.as_deref().is_some_and(|v| v == "1" || v == "true");
    download::file_response(st.as_ref(), &path, &headers, inline).await
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
    let default_name = match items.as_slice() {
        [one] => one.name().unwrap_or(&id).to_owned(),
        _ => "download".to_owned(),
    };
    let name = q
        .name
        .filter(|n| !n.is_empty() && n.len() < 200 && !n.contains(['/', '\\']))
        .unwrap_or(default_name);
    for item in &items {
        crate::auth::authorize(&state, &auth.user, &id, Perms::DOWNLOAD, item).await?;
    }
    zip::zip_response(st, items, &name, download_filter(&auth.user, &id)).await
}

/// Keeps what `user` may download (files) or look into (folders) in `source`;
/// paths are user-visible.
pub fn download_filter(user: &User, source: &str) -> zip::ZipFilter {
    let user = user.clone();
    let source = source.to_owned();
    Arc::new(move |p: &VPath, is_dir: bool| {
        if is_dir {
            user.can_traverse(&source, p)
        } else {
            user.effective(&source, p).contains(Perms::DOWNLOAD)
        }
    })
}

/// Body for `mkdir`.
#[derive(Debug, Deserialize)]
pub struct MkdirBody {
    path: VPath,
}

async fn mkdir(
    State(state): State<AppState>,
    auth: AuthUser,
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
    Path(id): Path<String>,
    Json(body): Json<TransferBody>,
) -> AppResult<Json<EntryDto>> {
    let st = scoped(&state, &auth.user, &id, Perms::EDIT, &body.from).await?;
    auth.user.require(&id, Perms::EDIT, &body.to)?;
    st.rename(&body.from, &body.to, body.overwrite).await?;
    let e = st.stat(&body.to).await?;
    Ok(Json(EntryDto::from_entry(e, body.to)))
}

async fn copy_entry(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<TransferBody>,
) -> AppResult<Json<EntryDto>> {
    let st = scoped(&state, &auth.user, &id, Perms::DOWNLOAD, &body.from).await?;
    let need = if body.overwrite {
        Perms::UPLOAD | Perms::EDIT
    } else {
        Perms::UPLOAD
    };
    auth.user.require(&id, need, &body.to)?;
    st.copy(&body.from, &body.to, body.overwrite).await?;
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
    Path(id): Path<String>,
    Json(body): Json<DeleteBody>,
) -> AppResult<Json<DeleteResult>> {
    // Check permissions first so a batch is not half-applied on a 403.
    if body.paths.iter().any(VPath::is_root) {
        return Err(AppError::bad_request("cannot delete the root"));
    }
    let st = scoped(&state, &auth.user, &id, Perms::DELETE, &VPath::root()).await?;
    let mut result = DeleteResult {
        deleted: Vec::new(),
        failed: Vec::new(),
    };
    for visible in body.paths {
        match st.delete(&visible).await {
            Ok(()) => result.deleted.push(visible),
            Err(e) => result.failed.push(DeleteFailure {
                path: visible,
                message: AppError::from(e).to_string(),
            }),
        }
    }
    Ok(Json(result))
}
