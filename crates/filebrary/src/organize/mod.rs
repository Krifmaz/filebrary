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

use std::time::Duration;

use crate::{
    activity::{Activity, action},
    state::AppState,
};

/// How often the maintenance task runs.
pub const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(3600);

/// Runs one maintenance pass: purges expired trash items (recording a
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

/// Spawns the hourly maintenance task (first run shortly after start-up).
pub fn spawn_maintenance(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let mut tick = tokio::time::interval(MAINTENANCE_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            maintain(&state).await;
        }
    })
}
