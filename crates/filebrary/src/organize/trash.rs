//! The trash bin: deleted files wait here until restored or purged.
//!
//! Each source keeps its own trash in a hidden top-level folder,
//! [`TRASH_DIR`], so moving an item there is a cheap rename on the same
//! storage (no copy between disks or buckets). An item deleted from
//! `/photos/a.jpg` becomes `/.filebrary-trash/<id>` and a row in the `trash`
//! table remembers where it came from. Its tags and stars travel with it and
//! come back on restore.
//!
//! The trash folder is reserved: listings, search, WebDAV and every path a
//! user sends must leave it alone. Check with [`is_reserved`].
//!
//! Items are purged automatically after `trash_retention_days` (default 30) by
//! the maintenance task. With `trash_retention_days = 0` the trash is off and
//! [`remove`] deletes permanently.

use serde::Serialize;

use crate::{
    auth::{Perms, User},
    db,
    error::{AppError, AppResult},
    organize::meta,
    state::AppState,
    storage::{EntryKind, StorageError, VPath},
};

/// Name of the hidden top-level folder holding a source's trash.
pub const TRASH_DIR: &str = ".filebrary-trash";

/// Returns `true` if `path` (a real path inside a source) is the trash folder
/// or inside it. Such paths must not be listed, indexed or accepted from users.
pub fn is_reserved(path: &VPath) -> bool {
    path.components().first().is_some_and(|c| c == TRASH_DIR)
}

/// Where the item with `id` is kept inside its source.
fn item_path(id: &str) -> VPath {
    VPath::root()
        .join(TRASH_DIR)
        .and_then(|d| d.join(id))
        .expect("trash ids are valid path components")
}

/// One item in the trash, as stored.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct TrashItem {
    /// Random id.
    pub id: String,
    /// Source name.
    pub source: String,
    /// Real path it was deleted from.
    pub original_path: String,
    /// `file` or `dir`.
    pub kind: String,
    /// Size in bytes (0 for folders).
    pub size: i64,
    /// Who deleted it.
    pub deleted_by: Option<i64>,
    /// Their name (joined from `users`).
    pub deleted_by_name: Option<String>,
    /// Unix time of deletion.
    pub deleted_at: i64,
}

impl TrashItem {
    /// The original path, parsed.
    ///
    /// # Errors
    ///
    /// Fails only if the database was edited by hand.
    pub fn original(&self) -> AppResult<VPath> {
        Ok(VPath::parse(&self.original_path)?)
    }
}

/// A trash item as a particular user sees it.
#[derive(Clone, Debug, Serialize)]
pub struct TrashView {
    /// Id, for restore and purge.
    pub id: String,
    /// Source name.
    pub source: String,
    /// Original path, relative to the user's scope.
    pub path: VPath,
    /// Original file name.
    pub name: String,
    /// `file` or `dir`.
    pub kind: String,
    /// Size in bytes (0 for folders).
    pub size: i64,
    /// Name of the user who deleted it.
    pub deleted_by: Option<String>,
    /// Unix time of deletion.
    pub deleted_at: i64,
    /// Unix time it will be purged automatically.
    pub purge_at: i64,
}

/// What [`remove`] did.
#[derive(Debug)]
pub enum Removed {
    /// Moved to the trash.
    Trashed(TrashItem),
    /// Deleted permanently (the trash is turned off).
    Deleted,
}

fn retention_secs(state: &AppState) -> i64 {
    i64::try_from(state.config.trash_retention_days.saturating_mul(86_400)).unwrap_or(i64::MAX / 2)
}

/// Maps an item to what `user` sees, or `None` if it is outside their scope.
pub fn view(state: &AppState, user: &User, item: &TrashItem) -> Option<TrashView> {
    let grant = user.grant(&item.source)?;
    if !grant.perms.contains(Perms::VIEW) {
        return None;
    }
    let path = user.visible_path(&item.source, &item.original().ok()?)?;
    Some(TrashView {
        id: item.id.clone(),
        source: item.source.clone(),
        name: path.name().unwrap_or_default().to_owned(),
        path,
        kind: item.kind.clone(),
        size: item.size,
        deleted_by: item.deleted_by_name.clone(),
        deleted_at: item.deleted_at,
        purge_at: item.deleted_at.saturating_add(retention_secs(state)),
    })
}

const SELECT: &str = "SELECT t.id, t.source, t.original_path, t.kind, t.size, t.deleted_by,
    u.username AS deleted_by_name, t.deleted_at
    FROM trash t LEFT JOIN users u ON u.id = t.deleted_by";

/// Moves `path` (a real path; permissions already checked) to the trash, or
/// deletes it if the trash is off. Tags and stars follow the item.
///
/// This is the function the files `DELETE` endpoint and WebDAV should call.
/// The caller records the activity event.
///
/// # Errors
///
/// `BadRequest` for the root or a path inside the trash, `NotFound` if
/// missing, storage and database failures.
pub async fn remove(
    state: &AppState,
    user_id: Option<i64>,
    source: &str,
    path: &VPath,
) -> AppResult<Removed> {
    if path.is_root() {
        return Err(AppError::bad_request("cannot delete the source root"));
    }
    if is_reserved(path) {
        return Err(AppError::NotFound);
    }
    let src = state.source(source).ok_or(AppError::NotFound)?;
    if state.config.trash_retention_days == 0 {
        src.storage.delete(path).await?;
        meta::on_delete(&state.db, source, path).await?;
        return Ok(Removed::Deleted);
    }
    let entry = src.storage.stat(path).await?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let dest = item_path(&id);
    src.storage
        .mkdir(&dest.parent().expect("item path has a parent"))
        .await?;
    src.storage.rename(path, &dest, false).await?;
    let (kind, size) = match entry.kind {
        EntryKind::Dir => ("dir", 0),
        EntryKind::File => ("file", i64::try_from(entry.size).unwrap_or(i64::MAX)),
    };
    let now = db::now();
    let inserted = sqlx::query(
        "INSERT INTO trash (id, source, original_path, kind, size, deleted_by, deleted_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(source)
    .bind(path.to_string())
    .bind(kind)
    .bind(size)
    .bind(user_id)
    .bind(now)
    .execute(&state.db)
    .await;
    if let Err(e) = inserted {
        // Put it back so nothing is lost without a record.
        if let Err(e2) = src.storage.rename(&dest, path, false).await {
            tracing::error!(error = %e2, %id, "could not undo a trash move; item left in trash folder");
        }
        return Err(e.into());
    }
    meta::on_move(&state.db, source, path, source, &dest).await?;
    get(state, &id).await.map(Removed::Trashed)
}

/// Loads one item.
///
/// # Errors
///
/// `NotFound`, database failures.
pub async fn get(state: &AppState, id: &str) -> AppResult<TrashItem> {
    Ok(sqlx::query_as(crate::sql_fmt!("{SELECT} WHERE t.id = ?"))
        .bind(id)
        .fetch_one(&state.db)
        .await?)
}

/// Every item in `source` (or all sources), newest first.
///
/// # Errors
///
/// Database failures.
pub async fn list(state: &AppState, source: Option<&str>) -> AppResult<Vec<TrashItem>> {
    Ok(match source {
        Some(s) => {
            sqlx::query_as(crate::sql_fmt!(
                "{SELECT} WHERE t.source = ? ORDER BY t.deleted_at DESC, t.id"
            ))
            .bind(s)
            .fetch_all(&state.db)
            .await?
        }
        None => {
            sqlx::query_as(crate::sql_fmt!("{SELECT} ORDER BY t.deleted_at DESC, t.id"))
                .fetch_all(&state.db)
                .await?
        }
    })
}

/// What to do when the restore target already exists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnConflict {
    /// Fail with `409 Conflict` (default).
    #[default]
    Fail,
    /// Pick a free name such as `report (restored).pdf`.
    Rename,
    /// Replace what is there.
    Overwrite,
}

/// `report.pdf` → `report (restored 2).pdf`; `notes` → `notes (restored)`.
fn restored_name(name: &str, n: u32) -> String {
    let suffix = if n == 1 {
        " (restored)".to_owned()
    } else {
        format!(" (restored {n})")
    };
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{}{suffix}{}", &name[..dot], &name[dot..]),
        _ => format!("{name}{suffix}"),
    }
}

/// Restores `item` to `to` (a real path; defaults to where it was), creating
/// missing parent folders. Returns the real path it was restored to.
///
/// # Errors
///
/// `Conflict` if the target exists and `on_conflict` is `Fail`; storage and
/// database failures.
pub async fn restore(
    state: &AppState,
    item: &TrashItem,
    to: Option<VPath>,
    on_conflict: OnConflict,
) -> AppResult<VPath> {
    let src = state.source(&item.source).ok_or(AppError::NotFound)?;
    let mut dest = match to {
        Some(p) => p,
        None => item.original()?,
    };
    if dest.is_root() || is_reserved(&dest) {
        return Err(AppError::bad_request("invalid restore target"));
    }
    let parent = dest.parent().expect("not root");
    src.storage.mkdir(&parent).await?;
    let exists = |p: VPath| async move {
        match src.storage.stat(&p).await {
            Ok(_) => Ok(true),
            Err(StorageError::NotFound) => Ok(false),
            Err(e) => Err(AppError::from(e)),
        }
    };
    if exists(dest.clone()).await? {
        match on_conflict {
            OnConflict::Fail => {
                return Err(AppError::conflict(
                    "something already exists at the original location",
                ));
            }
            OnConflict::Overwrite => {}
            OnConflict::Rename => {
                let name = dest.name().expect("not root").to_owned();
                let mut n = 1;
                loop {
                    let candidate = parent.join(&restored_name(&name, n))?;
                    if !exists(candidate.clone()).await? {
                        dest = candidate;
                        break;
                    }
                    n += 1;
                    if n > 1000 {
                        return Err(AppError::conflict("no free name to restore to"));
                    }
                }
            }
        }
    }
    let from = item_path(&item.id);
    src.storage
        .rename(&from, &dest, on_conflict == OnConflict::Overwrite)
        .await?;
    sqlx::query("DELETE FROM trash WHERE id = ?")
        .bind(&item.id)
        .execute(&state.db)
        .await?;
    meta::on_move(&state.db, &item.source, &from, &item.source, &dest).await?;
    state.search.changed(state, &item.source, &dest);
    Ok(dest)
}

/// Deletes `item` for good.
///
/// # Errors
///
/// Storage and database failures (an already missing file is fine).
pub async fn purge(state: &AppState, item: &TrashItem) -> AppResult<()> {
    let path = item_path(&item.id);
    if let Some(src) = state.source(&item.source) {
        match src.storage.delete(&path).await {
            Ok(()) | Err(StorageError::NotFound) => {}
            Err(e) => return Err(e.into()),
        }
    }
    meta::on_delete(&state.db, &item.source, &path).await?;
    sqlx::query("DELETE FROM trash WHERE id = ?")
        .bind(&item.id)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Purges every item older than the retention period. Returns the purged
/// items. Called hourly by the maintenance task.
///
/// # Errors
///
/// Database failures (a failing item is logged and skipped).
pub async fn purge_expired(state: &AppState) -> AppResult<Vec<TrashItem>> {
    if state.config.trash_retention_days == 0 {
        return Ok(Vec::new());
    }
    let cutoff = db::now().saturating_sub(retention_secs(state));
    let items: Vec<TrashItem> = sqlx::query_as(crate::sql_fmt!(
        "{SELECT} WHERE t.deleted_at <= ? ORDER BY t.deleted_at"
    ))
    .bind(cutoff)
    .fetch_all(&state.db)
    .await?;
    let mut done = Vec::with_capacity(items.len());
    for item in items {
        if state.source(&item.source).is_none() {
            continue; // source removed from the config: keep the record
        }
        match purge(state, &item).await {
            Ok(()) => done.push(item),
            Err(e) => tracing::warn!(error = %e, id = %item.id, "could not purge trash item"),
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_paths() {
        assert!(is_reserved(&VPath::parse(".filebrary-trash").unwrap()));
        assert!(is_reserved(
            &VPath::parse("/.filebrary-trash/abc/x").unwrap()
        ));
        assert!(!is_reserved(&VPath::parse("a/.filebrary-trash").unwrap()));
        assert!(!is_reserved(&VPath::root()));
    }

    #[test]
    fn restored_names() {
        assert_eq!(restored_name("report.pdf", 1), "report (restored).pdf");
        assert_eq!(restored_name("report.pdf", 3), "report (restored 3).pdf");
        assert_eq!(restored_name("notes", 1), "notes (restored)");
        assert_eq!(restored_name(".bashrc", 2), ".bashrc (restored 2)");
    }
}
