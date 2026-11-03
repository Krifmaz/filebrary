//! REST endpoints for the trash bin, tags, stars and listing annotations.
//!
//! All paths in requests and responses are as the caller sees them (relative
//! to their scope), like the rest of the API. Every endpoint needs a login;
//! unsafe methods need the CSRF header.
//!
//! | Method & path                          | Permission         | Purpose                              |
//! |----------------------------------------|--------------------|--------------------------------------|
//! | `GET /api/trash?source=`               | view               | Items in the trash                   |
//! | `POST /api/trash`                      | delete             | Move paths to the trash              |
//! | `POST /api/trash/{id}/restore`         | delete (+ upload at `to`) | Restore an item               |
//! | `DELETE /api/trash/{id}`               | delete             | Delete one item for good             |
//! | `DELETE /api/trash?source=`            | delete             | Empty the trash (what you can reach) |
//! | `GET /api/tags`                        | login              | All tags                             |
//! | `POST /api/tags`                       | edit anywhere      | Create a tag                         |
//! | `PATCH /api/tags/{id}`                 | admin              | Rename or recolour a tag             |
//! | `DELETE /api/tags/{id}`                | admin              | Delete a tag everywhere              |
//! | `GET /api/tags/{id}/files?source=`     | view               | Files with a tag                     |
//! | `POST /api/tags/{id}/files`            | edit               | Add a tag to paths                   |
//! | `DELETE /api/tags/{id}/files`          | edit               | Remove a tag from paths              |
//! | `GET /api/tags/file?source=&path=`     | view               | Tags of one path                     |
//! | `PUT /api/tags/file`                   | edit               | Replace the tags of one path         |
//! | `GET /api/stars`                       | login              | Your starred paths                   |
//! | `PUT /api/stars`                       | view               | Star a path                          |
//! | `DELETE /api/stars?source=&path=`      | login              | Unstar a path                        |
//! | `GET /api/annotations?source=&path=`   | view               | Stars and tags of a folder's entries |

use std::collections::BTreeMap;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    meta::{self, Annotation, TagRef},
    trash::{self, OnConflict, Removed, TrashView},
};
use crate::{
    activity::{Activity, ClientIp, action},
    api::files,
    auth::{AuthUser, Perms, User},
    db,
    error::{AppError, AppResult},
    state::AppState,
    storage::VPath,
};

/// Routes merged into `/api`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/trash",
            get(trash_list).post(trash_add).delete(trash_empty),
        )
        .route("/trash/{id}", axum::routing::delete(trash_purge))
        .route("/trash/{id}/restore", post(trash_restore))
        .route("/tags", get(tags_list).post(tag_create))
        .route("/tags/file", get(file_tags_get).put(file_tags_set))
        .route(
            "/tags/{id}",
            axum::routing::patch(tag_update).delete(tag_delete),
        )
        .route(
            "/tags/{id}/files",
            get(tag_files).post(tag_files_add).delete(tag_files_remove),
        )
        .route("/stars", get(stars_list).put(star_add).delete(star_remove))
        .route("/annotations", get(annotations))
}

/// Checks `perm` on the user-visible `path` and returns the real path,
/// refusing the reserved trash folder.
fn resolve(user: &User, source: &str, perm: Perms, path: &str) -> AppResult<VPath> {
    let real = user.require(source, perm, &VPath::parse(path)?)?;
    if trash::is_reserved(&real) {
        return Err(AppError::NotFound);
    }
    Ok(real)
}

/// Checks `perm` on a real path, returning what the user sees.
fn require_real(user: &User, source: &str, perm: Perms, real: &VPath) -> AppResult<VPath> {
    let vis = user.visible_path(source, real).ok_or(AppError::NotFound)?;
    user.require(source, perm, &vis)?;
    Ok(vis)
}

/// Fails with `NotFound` unless the real path exists *as seen through the
/// user's confined backend*, so a symlink in the user's folder that points
/// elsewhere in the source is refused rather than followed.
async fn must_exist(state: &AppState, user: &User, source: &str, real: &VPath) -> AppResult<()> {
    let vis = user.visible_path(source, real).ok_or(AppError::NotFound)?;
    let backend = files::scoped(state, user, source, Perms::VIEW, &vis).await?;
    backend.stat(&vis).await?;
    Ok(())
}

// ---------------------------------------------------------------- trash

/// `?source=` filter.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SourceFilter {
    source: Option<String>,
}

async fn trash_list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<SourceFilter>,
) -> AppResult<Json<Vec<TrashView>>> {
    let items = trash::list(&state, q.source.as_deref()).await?;
    Ok(Json(
        items
            .iter()
            .filter_map(|i| trash::view(&state, &auth.user, i))
            .collect(),
    ))
}

/// Body of `POST /api/trash`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrashRequest {
    source: String,
    paths: Vec<String>,
}

/// Result of `POST /api/trash`: what went to the trash, and what was deleted
/// outright because the trash is turned off.
#[derive(Debug, Serialize)]
struct TrashResponse {
    trashed: Vec<TrashView>,
    deleted: Vec<VPath>,
}

async fn trash_add(
    State(state): State<AppState>,
    auth: AuthUser,
    ip: ClientIp,
    Json(req): Json<TrashRequest>,
) -> AppResult<Json<TrashResponse>> {
    if req.paths.is_empty() || req.paths.len() > 1000 {
        return Err(AppError::bad_request("send between 1 and 1000 paths"));
    }
    // Check every path before touching any of them. `confine` resolves
    // symlinks inside the user's folder so the move cannot reach outside it.
    let mut reals = Vec::with_capacity(req.paths.len());
    for p in &req.paths {
        let vis = VPath::parse(p)?;
        reals.push(super::confine(&state, &auth.user, &req.source, Perms::DELETE, &vis).await?);
    }
    let mut out = TrashResponse {
        trashed: Vec::new(),
        deleted: Vec::new(),
    };
    for real in reals {
        match trash::remove(&state, Some(auth.user.id), &req.source, &real).await? {
            Removed::Trashed(item) => {
                Activity::new(action::TRASH)
                    .user(&auth.user)
                    .at(&req.source, &real)
                    .details(json!({ "trash_id": item.id }))
                    .ip(ip.0.as_deref())
                    .record(&state)
                    .await;
                out.trashed.extend(trash::view(&state, &auth.user, &item));
            }
            Removed::Deleted => {
                Activity::new(action::DELETE)
                    .user(&auth.user)
                    .at(&req.source, &real)
                    .ip(ip.0.as_deref())
                    .record(&state)
                    .await;
                out.deleted
                    .extend(auth.user.visible_path(&req.source, &real));
            }
        }
    }
    Ok(Json(out))
}

/// Body of `POST /api/trash/{id}/restore` (all optional).
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RestoreRequest {
    /// Restore here instead of the original location.
    to: Option<String>,
    /// `fail` (default), `rename` or `overwrite`.
    on_conflict: OnConflict,
}

/// Where an item was restored to.
#[derive(Debug, Serialize)]
struct Restored {
    source: String,
    path: VPath,
}

async fn trash_restore(
    State(state): State<AppState>,
    auth: AuthUser,
    ip: ClientIp,
    Path(id): Path<String>,
    body: Option<Json<RestoreRequest>>,
) -> AppResult<Json<Restored>> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let item = trash::get(&state, &id).await?;
    let original = item.original()?;
    let original_vis = require_real(&auth.user, &item.source, Perms::DELETE, &original)?;
    let (dest_vis, perm) = match &req.to {
        Some(p) => (VPath::parse(p)?, Perms::UPLOAD),
        None => (original_vis, Perms::DELETE),
    };
    if dest_vis.is_root() {
        return Err(AppError::bad_request("invalid restore target"));
    }
    if req.on_conflict == OnConflict::Overwrite {
        auth.user.require(&item.source, Perms::EDIT, &dest_vis)?;
    }
    // Create missing parents through the confined backend, then resolve the
    // target symlink-free: the folders may have changed since the delete.
    let backend = files::scoped(&state, &auth.user, &item.source, perm, &dest_vis).await?;
    backend.mkdir(&dest_vis.parent().expect("not root")).await?;
    let to = super::confine(&state, &auth.user, &item.source, perm, &dest_vis).await?;
    let dest = trash::restore(&state, &item, Some(to), req.on_conflict).await?;
    Activity::new(action::RESTORE)
        .user(&auth.user)
        .at(&item.source, &original)
        .target(&dest)
        .ip(ip.0.as_deref())
        .record(&state)
        .await;
    let path = auth
        .user
        .visible_path(&item.source, &dest)
        .ok_or(AppError::NotFound)?;
    Ok(Json(Restored {
        source: item.source,
        path,
    }))
}

async fn trash_purge(
    State(state): State<AppState>,
    auth: AuthUser,
    ip: ClientIp,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    let item = trash::get(&state, &id).await?;
    let original = item.original()?;
    require_real(&auth.user, &item.source, Perms::DELETE, &original)?;
    trash::purge(&state, &item).await?;
    Activity::new(action::DELETE)
        .user(&auth.user)
        .at(&item.source, &original)
        .details(json!({ "trash_id": item.id }))
        .ip(ip.0.as_deref())
        .record(&state)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn trash_empty(
    State(state): State<AppState>,
    auth: AuthUser,
    ip: ClientIp,
    Query(q): Query<SourceFilter>,
) -> AppResult<Json<serde_json::Value>> {
    let items = trash::list(&state, q.source.as_deref()).await?;
    let mut purged = 0u64;
    for item in items {
        let Ok(original) = item.original() else {
            continue;
        };
        if require_real(&auth.user, &item.source, Perms::DELETE, &original).is_err() {
            continue;
        }
        trash::purge(&state, &item).await?;
        purged += 1;
        Activity::new(action::DELETE)
            .user(&auth.user)
            .at(&item.source, &original)
            .details(json!({ "trash_id": item.id, "empty": true }))
            .ip(ip.0.as_deref())
            .record(&state)
            .await;
    }
    Ok(Json(json!({ "purged": purged })))
}

// ---------------------------------------------------------------- tags

/// Longest tag name, in characters.
pub const MAX_TAG_LEN: usize = 64;

/// Trims and validates a tag name.
fn clean_tag_name(name: &str) -> AppResult<String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > MAX_TAG_LEN || n.chars().any(char::is_control) {
        return Err(AppError::bad_request(format!(
            "tag names are 1-{MAX_TAG_LEN} characters without control characters"
        )));
    }
    Ok(n.to_owned())
}

/// Validates a `#rrggbb` colour; an empty string means "no colour".
fn clean_color(c: Option<&str>) -> AppResult<Option<String>> {
    match c.map(str::trim) {
        None | Some("") => Ok(None),
        Some(c)
            if c.len() == 7
                && c.starts_with('#')
                && c[1..].bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            Ok(Some(c.to_ascii_lowercase()))
        }
        Some(_) => Err(AppError::bad_request("colour must look like #1a2b3c")),
    }
}

/// Whether `user` may create new tag names: admins and anyone with the edit
/// permission somewhere.
fn may_create_tags(user: &User) -> bool {
    user.is_admin || user.grants.values().any(|g| g.perms.contains(Perms::EDIT))
}

async fn tags_list(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Vec<TagRef>>> {
    Ok(Json(
        sqlx::query_as("SELECT id, name, color FROM tags ORDER BY name COLLATE NOCASE")
            .fetch_all(&state.db)
            .await?,
    ))
}

/// Body of `POST /api/tags` and `PATCH /api/tags/{id}`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TagBody {
    name: Option<String>,
    /// `#rrggbb`; empty string clears it.
    color: Option<String>,
}

/// Maps a unique-constraint failure to `409`.
fn tag_conflict(e: sqlx::Error) -> AppError {
    match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            AppError::conflict("a tag with that name already exists")
        }
        _ => e.into(),
    }
}

async fn tag_create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<TagBody>,
) -> AppResult<(StatusCode, Json<TagRef>)> {
    if !may_create_tags(&auth.user) {
        return Err(AppError::forbidden("you cannot create tags"));
    }
    let name = clean_tag_name(body.name.as_deref().unwrap_or_default())?;
    let color = clean_color(body.color.as_deref())?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO tags (name, color, created_by, created_at) VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(&name)
    .bind(&color)
    .bind(auth.user.id)
    .bind(db::now())
    .fetch_one(&state.db)
    .await
    .map_err(tag_conflict)?;
    Ok((StatusCode::CREATED, Json(TagRef { id, name, color })))
}

async fn tag_update(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<TagBody>,
) -> AppResult<Json<TagRef>> {
    if !auth.user.is_admin {
        return Err(AppError::forbidden("administrators only"));
    }
    let mut tag: TagRef = sqlx::query_as("SELECT id, name, color FROM tags WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if let Some(n) = &body.name {
        tag.name = clean_tag_name(n)?;
    }
    if body.color.is_some() {
        tag.color = clean_color(body.color.as_deref())?;
    }
    sqlx::query("UPDATE tags SET name = ?, color = ? WHERE id = ?")
        .bind(&tag.name)
        .bind(&tag.color)
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(tag_conflict)?;
    Ok(Json(tag))
}

async fn tag_delete(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    if !auth.user.is_admin {
        return Err(AppError::forbidden("administrators only"));
    }
    let r = sqlx::query("DELETE FROM tags WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    if r.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// A file reference in responses.
#[derive(Debug, Serialize)]
struct FileRef {
    source: String,
    path: VPath,
}

async fn tag_files(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
    Query(q): Query<SourceFilter>,
) -> AppResult<Json<Vec<FileRef>>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT source, path FROM file_tags WHERE tag_id = ? AND (? IS NULL OR source = ?)
         ORDER BY source, path",
    )
    .bind(id)
    .bind(&q.source)
    .bind(&q.source)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(visible_refs(&auth.user, rows)))
}

/// Maps `(source, real path)` rows to what the user may see, dropping the
/// rest and anything in the trash.
fn visible_refs(user: &User, rows: Vec<(String, String)>) -> Vec<FileRef> {
    rows.into_iter()
        .filter_map(|(source, p)| {
            let real = VPath::parse(&p).ok()?;
            if trash::is_reserved(&real) {
                return None;
            }
            let vis = user.visible_path(&source, &real)?;
            user.require(&source, Perms::VIEW, &vis).ok()?;
            Some(FileRef { source, path: vis })
        })
        .collect()
}

/// Body of `POST`/`DELETE /api/tags/{id}/files`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PathsBody {
    source: String,
    paths: Vec<String>,
}

async fn tag_files_add(
    State(state): State<AppState>,
    auth: AuthUser,
    ip: ClientIp,
    Path(id): Path<i64>,
    Json(body): Json<PathsBody>,
) -> AppResult<StatusCode> {
    tag_files_change(&state, &auth.user, &ip, id, body, true).await
}

async fn tag_files_remove(
    State(state): State<AppState>,
    auth: AuthUser,
    ip: ClientIp,
    Path(id): Path<i64>,
    Json(body): Json<PathsBody>,
) -> AppResult<StatusCode> {
    tag_files_change(&state, &auth.user, &ip, id, body, false).await
}

async fn tag_files_change(
    state: &AppState,
    user: &User,
    ip: &ClientIp,
    id: i64,
    body: PathsBody,
    add: bool,
) -> AppResult<StatusCode> {
    if body.paths.is_empty() || body.paths.len() > 1000 {
        return Err(AppError::bad_request("send between 1 and 1000 paths"));
    }
    let tag: TagRef = sqlx::query_as("SELECT id, name, color FROM tags WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    let reals = body
        .paths
        .iter()
        .map(|p| resolve(user, &body.source, Perms::EDIT, p))
        .collect::<AppResult<Vec<_>>>()?;
    if add {
        for real in &reals {
            must_exist(state, user, &body.source, real).await?;
        }
    }
    let now = db::now();
    for real in &reals {
        let changed = if add {
            sqlx::query(
                "INSERT OR IGNORE INTO file_tags (tag_id, source, path, added_by, added_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(&body.source)
            .bind(real.to_string())
            .bind(user.id)
            .bind(now)
            .execute(&state.db)
            .await?
        } else {
            sqlx::query("DELETE FROM file_tags WHERE tag_id = ? AND source = ? AND path = ?")
                .bind(id)
                .bind(&body.source)
                .bind(real.to_string())
                .execute(&state.db)
                .await?
        };
        if changed.rows_affected() > 0 {
            let key = if add { "added" } else { "removed" };
            Activity::new(action::TAG)
                .user(user)
                .at(&body.source, real)
                .details(json!({ key: [tag.name] }))
                .ip(ip.0.as_deref())
                .record(state)
                .await;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `?source=&path=` for one path.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PathQuery {
    source: String,
    path: String,
}

async fn file_tags_get(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<PathQuery>,
) -> AppResult<Json<Vec<TagRef>>> {
    let real = resolve(&auth.user, &q.source, Perms::VIEW, &q.path)?;
    Ok(Json(meta::tags_of(&state.db, &q.source, &real).await?))
}

/// Body of `PUT /api/tags/file`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetTagsBody {
    source: String,
    path: String,
    /// Tag names; unknown names are created.
    tags: Vec<String>,
}

async fn file_tags_set(
    State(state): State<AppState>,
    auth: AuthUser,
    ip: ClientIp,
    Json(body): Json<SetTagsBody>,
) -> AppResult<Json<Vec<TagRef>>> {
    if body.tags.len() > 100 {
        return Err(AppError::bad_request("at most 100 tags per file"));
    }
    let real = resolve(&auth.user, &body.source, Perms::EDIT, &body.path)?;
    if real.is_root() {
        return Err(AppError::bad_request("the source root cannot be tagged"));
    }
    must_exist(&state, &auth.user, &body.source, &real).await?;
    let mut wanted = BTreeMap::new(); // lowercase name -> cleaned name
    for t in &body.tags {
        let n = clean_tag_name(t)?;
        wanted.entry(n.to_lowercase()).or_insert(n);
    }
    let before = meta::tags_of(&state.db, &body.source, &real).await?;
    let now = db::now();
    let path_s = real.to_string();
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM file_tags WHERE source = ? AND path = ?")
        .bind(&body.source)
        .bind(&path_s)
        .execute(&mut *tx)
        .await?;
    for name in wanted.values() {
        let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM tags WHERE name = ?")
            .bind(name)
            .fetch_optional(&mut *tx)
            .await?;
        let id = match existing {
            Some(id) => id,
            None => {
                // Editing a file you may edit is enough to coin a new name.
                sqlx::query_scalar(
                    "INSERT INTO tags (name, created_by, created_at) VALUES (?, ?, ?) RETURNING id",
                )
                .bind(name)
                .bind(auth.user.id)
                .bind(now)
                .fetch_one(&mut *tx)
                .await?
            }
        };
        sqlx::query(
            "INSERT OR IGNORE INTO file_tags (tag_id, source, path, added_by, added_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(&body.source)
        .bind(&path_s)
        .bind(auth.user.id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    let after = meta::tags_of(&state.db, &body.source, &real).await?;
    let added: Vec<&str> = after
        .iter()
        .filter(|t| !before.iter().any(|b| b.id == t.id))
        .map(|t| t.name.as_str())
        .collect();
    let removed: Vec<&str> = before
        .iter()
        .filter(|b| !after.iter().any(|t| t.id == b.id))
        .map(|t| t.name.as_str())
        .collect();
    if !added.is_empty() || !removed.is_empty() {
        Activity::new(action::TAG)
            .user(&auth.user)
            .at(&body.source, &real)
            .details(json!({ "added": added, "removed": removed }))
            .ip(ip.0.as_deref())
            .record(&state)
            .await;
    }
    Ok(Json(after))
}

// ---------------------------------------------------------------- stars

/// A starred path.
#[derive(Debug, Serialize)]
struct Star {
    source: String,
    path: VPath,
    name: String,
    starred_at: i64,
}

async fn stars_list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Vec<Star>>> {
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT source, path, created_at FROM stars WHERE user_id = ? ORDER BY created_at DESC",
    )
    .bind(auth.user.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .filter_map(|(source, p, starred_at)| {
                let real = VPath::parse(&p).ok()?;
                if trash::is_reserved(&real) {
                    return None;
                }
                let path = auth.user.visible_path(&source, &real)?;
                auth.user.require(&source, Perms::VIEW, &path).ok()?;
                Some(Star {
                    name: path.name().unwrap_or(&source).to_owned(),
                    source,
                    path,
                    starred_at,
                })
            })
            .collect(),
    ))
}

/// Body of `PUT /api/stars`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StarBody {
    source: String,
    path: String,
}

async fn star_add(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<StarBody>,
) -> AppResult<StatusCode> {
    let real = resolve(&auth.user, &body.source, Perms::VIEW, &body.path)?;
    must_exist(&state, &auth.user, &body.source, &real).await?;
    sqlx::query(
        "INSERT OR IGNORE INTO stars (user_id, source, path, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(auth.user.id)
    .bind(&body.source)
    .bind(real.to_string())
    .bind(db::now())
    .execute(&state.db)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn star_remove(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<PathQuery>,
) -> AppResult<StatusCode> {
    // Unstarring needs no permission beyond seeing the path: the row is yours.
    let vis = VPath::parse(&q.path)?;
    let scope = auth
        .user
        .grant(&q.source)
        .map(|g| g.scope.clone())
        .ok_or(AppError::NotFound)?;
    sqlx::query("DELETE FROM stars WHERE user_id = ? AND source = ? AND path = ?")
        .bind(auth.user.id)
        .bind(&q.source)
        .bind(scope.join_path(&vis).to_string())
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- annotations

async fn annotations(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<PathQuery>,
) -> AppResult<Json<BTreeMap<String, Annotation>>> {
    let real = resolve(&auth.user, &q.source, Perms::VIEW, &q.path)?;
    let mut map = meta::annotate_dir(&state.db, auth.user.id, &q.source, &real).await?;
    if real.is_root() {
        map.remove(trash::TRASH_DIR);
    }
    Ok(Json(map))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_names() {
        assert_eq!(clean_tag_name("  Holiday 2026 ").unwrap(), "Holiday 2026");
        assert!(clean_tag_name("").is_err());
        assert!(clean_tag_name("   ").is_err());
        assert!(clean_tag_name("a\nb").is_err());
        assert!(clean_tag_name(&"x".repeat(65)).is_err());
        assert!(clean_tag_name(&"é".repeat(64)).is_ok());
    }

    #[test]
    fn colors() {
        assert_eq!(clean_color(Some("#A1b2C3")).unwrap().unwrap(), "#a1b2c3");
        assert_eq!(clean_color(Some("")).unwrap(), None);
        assert_eq!(clean_color(None).unwrap(), None);
        for bad in ["red", "#12345", "#1234567", "#12345g", "123456"] {
            assert!(clean_color(Some(bad)).is_err(), "{bad}");
        }
    }
}
