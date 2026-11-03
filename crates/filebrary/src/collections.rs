//! Collections: named lists of files and folders, possibly from several
//! sources, that a user gathers and can share with a single link.
//!
//! Nested at `/api/collections`:
//!
//! | Method   | Path                    | What                                         |
//! |----------|-------------------------|----------------------------------------------|
//! | `GET`    | ``                      | The caller's collections with item counts.   |
//! | `POST`   | `` (`{name, items?}`)   | Create one, optionally with items.           |
//! | `GET`    | `/{id}`                 | One collection with its items.               |
//! | `PATCH`  | `/{id}` (`{name}`)      | Rename.                                      |
//! | `DELETE` | `/{id}`                 | Delete (its share links go with it).         |
//! | `POST`   | `/{id}/items` (`{items}`)| Add items (`[{source, path}]`).             |
//! | `DELETE` | `/{id}/items/{item}`    | Remove one item.                             |
//!
//! Collections are private to their owner. Adding an item needs `view` on it.
//! Items are stored by real path; a response maps them back to what the owner
//! sees and flags items that have gone missing or out of reach.
//!
//! Sharing a collection goes through `/api/shares` with `collection_id`; see
//! [`share_items`], which the share code uses to resolve the items when the
//! link is opened (only items the owner may still share are included).

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};

use crate::api::files::scoped;
use crate::{
    auth::{AuthUser, Perms, User},
    db,
    error::{AppError, AppResult},
    state::AppState,
    storage::{EntryKind, VPath},
};

/// Most items one collection may hold.
pub const MAX_ITEMS: i64 = 1000;
/// Longest collection name, in characters.
pub const MAX_NAME_CHARS: usize = 200;

/// Routes nested at `/api/collections`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", get(show).patch(rename).delete(remove))
        .route("/{id}/items", post(add_items))
        .route("/{id}/items/{item}", delete(remove_item))
}

/// A collection in a listing.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct CollectionSummary {
    /// Database id.
    pub id: i64,
    /// Display name.
    pub name: String,
    /// Number of items.
    pub item_count: i64,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds.
    pub updated_at: i64,
}

/// One item as the owner sees it.
#[derive(Debug, Serialize)]
pub struct Item {
    /// Item id (for removal).
    pub id: i64,
    /// Source name.
    pub source: String,
    /// Path as the owner sees it; `None` if it is no longer in their scope.
    pub path: Option<VPath>,
    /// Final path component.
    pub name: String,
    /// `file` or `dir`; `None` if the item no longer exists or is out of reach.
    pub kind: Option<EntryKind>,
    /// Size in bytes (files only).
    pub size: Option<u64>,
    /// MIME type guess (files only).
    pub mime: Option<String>,
}

/// A collection with its items.
#[derive(Debug, Serialize)]
pub struct Collection {
    /// Database id.
    pub id: i64,
    /// Display name.
    pub name: String,
    /// Items, in the order they were added.
    pub items: Vec<Item>,
}

/// A file or folder to add, as the caller sees it.
#[derive(Debug, Deserialize)]
pub struct NewItem {
    /// Source name.
    pub source: String,
    /// Path as the caller sees it.
    pub path: String,
}

/// Body of `POST /api/collections`.
#[derive(Debug, Deserialize)]
pub struct CreateCollection {
    /// Display name.
    pub name: String,
    /// Optional first items.
    #[serde(default)]
    pub items: Vec<NewItem>,
}

/// Body of `PATCH /api/collections/{id}`.
#[derive(Debug, Deserialize)]
pub struct RenameCollection {
    /// New name.
    pub name: String,
}

/// Body of `POST /api/collections/{id}/items`.
#[derive(Debug, Deserialize)]
pub struct AddItems {
    /// Items to add; ones already present are ignored.
    pub items: Vec<NewItem>,
}

fn clean_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::bad_request("the name is empty"));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(AppError::bad_request(format!(
            "names are limited to {MAX_NAME_CHARS} characters"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(AppError::bad_request(
            "the name contains control characters",
        ));
    }
    Ok(name.to_owned())
}

/// Loads a collection's name, checking the caller owns it (`NotFound` otherwise).
async fn owned(state: &AppState, user: &User, id: i64) -> AppResult<String> {
    sqlx::query_scalar("SELECT name FROM collections WHERE id = ? AND owner_id = ?")
        .bind(id)
        .bind(user.id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound)
}

/// `GET /api/collections`.
async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
) -> AppResult<Json<Vec<CollectionSummary>>> {
    let rows = sqlx::query_as(
        "SELECT c.id, c.name, COUNT(i.id) AS item_count, c.created_at, c.updated_at
         FROM collections c LEFT JOIN collection_items i ON i.collection_id = c.id
         WHERE c.owner_id = ? GROUP BY c.id ORDER BY c.name COLLATE NOCASE, c.id",
    )
    .bind(auth.user.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

/// `POST /api/collections`.
async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreateCollection>,
) -> AppResult<(StatusCode, Json<Collection>)> {
    let name = clean_name(&req.name)?;
    let resolved = resolve_items(&state, &auth.user, &req.items).await?;
    let now = db::now();
    let mut tx = state.db.begin().await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO collections (owner_id, name, created_at, updated_at)
         VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(auth.user.id)
    .bind(&name)
    .bind(now)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    insert_items(&mut tx, id, &resolved, now).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(load(&state, &auth.user, id, name).await?),
    ))
}

/// `GET /api/collections/{id}`.
async fn show(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Collection>> {
    let name = owned(&state, &auth.user, id).await?;
    Ok(Json(load(&state, &auth.user, id, name).await?))
}

/// `PATCH /api/collections/{id}`.
async fn rename(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<RenameCollection>,
) -> AppResult<StatusCode> {
    let name = clean_name(&req.name)?;
    owned(&state, &auth.user, id).await?;
    sqlx::query("UPDATE collections SET name = ?, updated_at = ? WHERE id = ?")
        .bind(name)
        .bind(db::now())
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/collections/{id}`.
async fn remove(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let n = sqlx::query("DELETE FROM collections WHERE id = ? AND owner_id = ?")
        .bind(id)
        .bind(auth.user.id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/collections/{id}/items`.
async fn add_items(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<AddItems>,
) -> AppResult<Json<Collection>> {
    let name = owned(&state, &auth.user, id).await?;
    let resolved = resolve_items(&state, &auth.user, &req.items).await?;
    let now = db::now();
    let mut tx = state.db.begin().await?;
    insert_items(&mut tx, id, &resolved, now).await?;
    sqlx::query("UPDATE collections SET updated_at = ? WHERE id = ?")
        .bind(now)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(load(&state, &auth.user, id, name).await?))
}

/// `DELETE /api/collections/{id}/items/{item}`.
async fn remove_item(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((id, item)): Path<(i64, i64)>,
) -> AppResult<StatusCode> {
    owned(&state, &auth.user, id).await?;
    let n = sqlx::query("DELETE FROM collection_items WHERE id = ? AND collection_id = ?")
        .bind(item)
        .bind(id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound);
    }
    sqlx::query("UPDATE collections SET updated_at = ? WHERE id = ?")
        .bind(db::now())
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Checks each new item (view permission, existence) and maps it to its real path.
async fn resolve_items(
    state: &AppState,
    user: &User,
    items: &[NewItem],
) -> AppResult<Vec<(String, VPath)>> {
    if items.len() as i64 > MAX_ITEMS {
        return Err(AppError::bad_request(format!(
            "a collection holds at most {MAX_ITEMS} items"
        )));
    }
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        let visible = VPath::parse(&it.path)?;
        let real = user.require(&it.source, Perms::VIEW, &visible)?;
        if real.is_root() {
            return Err(AppError::bad_request(
                "add folders inside a source, not the source itself",
            ));
        }
        scoped(state, user, &it.source, Perms::VIEW, &visible)
            .await?
            .stat(&visible)
            .await?;
        out.push((it.source.clone(), real));
    }
    Ok(out)
}

async fn insert_items(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: i64,
    items: &[(String, VPath)],
    now: i64,
) -> AppResult<()> {
    for (source, real) in items {
        sqlx::query(
            "INSERT OR IGNORE INTO collection_items (collection_id, source, path, added_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(source)
        .bind(real.to_string())
        .bind(now)
        .execute(&mut **tx)
        .await?;
    }
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM collection_items WHERE collection_id = ?")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
    if count > MAX_ITEMS {
        return Err(AppError::bad_request(format!(
            "a collection holds at most {MAX_ITEMS} items"
        )));
    }
    Ok(())
}

/// Raw item rows of a collection: `(item id, source, real path)`, oldest first.
async fn item_rows(db: &sqlx::SqlitePool, id: i64) -> AppResult<Vec<(i64, String, String)>> {
    Ok(sqlx::query_as(
        "SELECT id, source, path FROM collection_items WHERE collection_id = ? ORDER BY id",
    )
    .bind(id)
    .fetch_all(db)
    .await?)
}

async fn load(state: &AppState, user: &User, id: i64, name: String) -> AppResult<Collection> {
    let mut items = Vec::new();
    for (item_id, source, path) in item_rows(&state.db, id).await? {
        let Ok(real) = VPath::parse(&path) else {
            continue;
        };
        let visible = user.visible_path(&source, &real);
        let entry = match &visible {
            Some(v) => match scoped(state, user, &source, Perms::VIEW, v).await {
                Ok(st) => st.stat(v).await.ok(),
                Err(_) => None,
            },
            None => None,
        };
        items.push(Item {
            id: item_id,
            name: real.name().unwrap_or_default().to_owned(),
            source,
            path: visible,
            kind: entry.as_ref().map(|e| e.kind),
            size: entry.as_ref().filter(|e| !e.is_dir()).map(|e| e.size),
            mime: entry.and_then(|e| e.mime),
        });
    }
    Ok(Collection { id, name, items })
}

/// A collection item that may be served through a share link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareableItem {
    /// Source name.
    pub source: String,
    /// Path as the *owner* sees it: serve it through
    /// `api::files::scoped(state, owner, source, perm, &path)`, never by
    /// prefixing a real path onto the unconfined source.
    pub path: VPath,
    /// Final path component, used as the item's name in the link.
    pub name: String,
}

/// Checks that `owner` owns collection `id` and may share every item in it
/// (`share` + `download`), for creating a link to it.
///
/// # Errors
///
/// `NotFound` if the collection is not the owner's, `BadRequest` if it is
/// empty, `Forbidden` if an item cannot be shared.
pub async fn check_shareable(state: &AppState, owner: &User, id: i64) -> AppResult<()> {
    owned(state, owner, id).await?;
    let rows = item_rows(&state.db, id).await?;
    if rows.is_empty() {
        return Err(AppError::bad_request("the collection is empty"));
    }
    for (_, source, path) in rows {
        let real = VPath::parse(&path).map_err(|_| AppError::NotFound)?;
        let visible = owner.visible_path(&source, &real).ok_or_else(|| {
            AppError::forbidden(format!("{source}:{path} is no longer in your reach"))
        })?;
        owner.require(&source, Perms::SHARE | Perms::DOWNLOAD, &visible)?;
    }
    Ok(())
}

/// The items a share link to collection `id` serves right now.
///
/// Called when a link is opened, with the share owner freshly loaded, so that
/// items the owner lost access to (or that left their scope) drop out of the
/// link instead of leaking. Duplicate names get ` (2)`, ` (3)`… suffixes so
/// every item has a unique top-level name in the link.
///
/// # Errors
///
/// Database failures.
pub async fn share_items(state: &AppState, owner: &User, id: i64) -> AppResult<Vec<ShareableItem>> {
    let mut out: Vec<ShareableItem> = Vec::new();
    for (_, source, path) in item_rows(&state.db, id).await? {
        let Ok(real) = VPath::parse(&path) else {
            continue;
        };
        let Some(visible) = owner.visible_path(&source, &real) else {
            continue;
        };
        if owner
            .require(&source, Perms::SHARE | Perms::DOWNLOAD, &visible)
            .is_err()
            || state.source(&source).is_none()
        {
            continue;
        }
        let base = real.name().unwrap_or("item").to_owned();
        let name = unique_name(&base, |n| out.iter().any(|i| i.name == n));
        out.push(ShareableItem {
            source,
            path: visible,
            name,
        });
    }
    Ok(out)
}

/// Returns `base`, or `stem (n).ext` for the first `n >= 2` not `taken`.
fn unique_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_owned();
    }
    let (stem, ext) = match base.rfind('.') {
        Some(i) if i > 0 => (&base[..i], &base[i..]),
        _ => (base, ""),
    };
    (2..)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|c| !taken(c))
        .unwrap_or_else(|| base.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(clean_name(" ").is_err());
        assert!(clean_name("a\u{7}b").is_err());
        assert_eq!(clean_name(" Trip ").unwrap(), "Trip");
    }

    #[test]
    fn unique_names() {
        let taken = ["a.txt", "a (2).txt", "dir"];
        let t = |n: &str| taken.contains(&n);
        assert_eq!(unique_name("b.txt", t), "b.txt");
        assert_eq!(unique_name("a.txt", t), "a (3).txt");
        assert_eq!(unique_name("dir", t), "dir (2)");
        assert_eq!(unique_name(".env", |n| n == ".env"), ".env (2)");
    }
}
