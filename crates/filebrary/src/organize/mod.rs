//! Organizing files: the trash bin, tags, stars and the maintenance task.
//!
//! * [`trash`]: delete to a per-source trash folder, restore, purge.
//! * [`meta`]: keeps tags and stars attached to files as they move. **The
//!   files, WebDAV and archive work streams must call its hooks after every
//!   rename, copy and permanent delete.**
//! * [`api`]: the REST endpoints (`/api/trash`, `/api/tags`, `/api/stars`,
//!   `/api/annotations`).
//!
//! The activity log is a sibling module, [`crate::activity`].

pub mod api;
pub mod meta;
pub mod trash;

use crate::{
    activity::{Activity, action},
    api::files,
    auth::{Perms, User},
    error::{AppError, AppResult},
    state::AppState,
    storage::VPath,
};

/// Checks `perm` on the user-visible `path` and returns its real path in the
/// source with every symlink in its *parent* folders resolved, guaranteed to
/// lie inside the user's scope. The final component is not followed, so a
/// symlink itself is moved or deleted, never its target.
///
/// Use this before operating on the unconfined source backend (the trash
/// lives at the source root, outside any user's scope): resolving through
/// [`files::scoped`] refuses a link that leads out of the user's folder.
///
/// # Errors
///
/// `NotFound` for invisible sources and the reserved trash folder,
/// `Forbidden` without `perm` or when a symlink leads outside the scope.
pub async fn confine(
    state: &AppState,
    user: &User,
    source: &str,
    perm: Perms,
    path: &VPath,
) -> AppResult<VPath> {
    let backend = files::scoped(state, user, source, perm, path).await?;
    let scope = user
        .grant(source)
        .map(|g| g.scope.clone())
        .ok_or(AppError::NotFound)?;
    let real = match (path.parent(), path.name()) {
        (Some(parent), Some(name)) => scope
            .join_path(&backend.resolve(&parent).await?)
            .join(name)?,
        _ => scope,
    };
    if trash::is_reserved(&real) {
        return Err(AppError::NotFound);
    }
    Ok(real)
}

/// Runs one maintenance pass (called hourly by [`crate::spawn_maintenance`]): purges expired trash items (recording a
/// `trash.purge` event for each) and old activity events.
pub async fn maintain(state: &AppState) {
    match trash::purge_expired(state).await {
        Ok(items) => {
            for item in &items {
                if let Ok(p) = item.original() {
                    Activity::new(action::PURGE)
                        .at(&item.source, &p)
                        .details(serde_json::json!({ "trash_id": item.id, "auto": true }))
                        .record(state)
                        .await;
                }
            }
            if !items.is_empty() {
                tracing::info!(count = items.len(), "purged expired trash items");
            }
        }
        Err(e) => tracing::warn!(error = %e, "trash purge failed"),
    }
    match crate::activity::purge_old(state).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(count = n, "deleted old activity events"),
        Err(e) => tracing::warn!(error = %e, "activity purge failed"),
    }
}
