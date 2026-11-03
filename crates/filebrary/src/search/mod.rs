//! Indexed search across every source.
//!
//! # How it works
//!
//! * A SQLite table, `file_index` (migration `0030_search`), holds one row per
//!   file and folder: name, extension, coarse type, size and modification
//!   time. An FTS5 table with the trigram tokenizer mirrors the names so that
//!   substring queries stay fast on millions of rows.
//! * At start-up every indexed source is scanned ([`scan`]). Rows are written
//!   in batches stamped with a *generation*; when a scan finishes, rows of
//!   older generations under the scanned folder are swept away. Memory use is
//!   bounded by the batch size and a short channel, not by the size of the tree.
//! * Local sources are then kept fresh by a file-system watcher ([`watch`]:
//!   inotify on Linux, FSEvents on macOS, ReadDirectoryChangesW on Windows,
//!   all through the `notify` crate). Every source also gets a periodic
//!   rescan: a slow safety net for watched sources and the only mechanism for
//!   sources that cannot be watched (network backends, or when the watcher
//!   cannot start, for example because `fs.inotify.max_user_watches` is too low).
//! * Changes are published as [`IndexEvent`]s on a broadcast channel
//!   ([`Search::subscribe`]) for other indexes (the full-text content index)
//!   to follow.
//! * [`query`] turns the filters of `GET /api/search` into one SQL query that
//!   also restricts results to the sources and folders the caller may view.
//!
//! The top-level trash folder of each source ([`TRASH_DIR`]) is never indexed.

pub mod api;
pub mod query;
pub mod scan;
pub mod store;
pub mod watch;

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, broadcast};

use crate::{config::SourceKind, state::AppState, storage::VPath};

pub use store::{IndexRow, IndexStore};

/// Name of the per-source trash folder (at the source root), which is never indexed.
pub const TRASH_DIR: &str = ".filebrary-trash";

/// Returns `true` for paths the indexer must skip: the trash folder and its
/// contents, and temporary files of in-flight atomic writes.
pub fn is_ignored(path: &VPath) -> bool {
    if path.components().first().is_some_and(|c| c == TRASH_DIR) {
        return true;
    }
    path.name()
        .is_some_and(|n| n.starts_with('.') && n.ends_with(".fbtmp"))
}

/// The `[search]` section of the configuration.
///
/// ```toml
/// [search]
/// enabled = true
/// watch = true
/// rescan_minutes = 60
/// watched_rescan_hours = 24
/// exclude_sources = ["scratch"]
/// ```
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct SearchSettings {
    /// Build and maintain the index at all. When off, `GET /api/search`
    /// answers with an error and no background work runs.
    pub enabled: bool,
    /// Watch local sources for changes (inotify and friends).
    pub watch: bool,
    /// Minutes between full rescans of sources that are not watched.
    pub rescan_minutes: u64,
    /// Hours between full rescans of watched sources (a safety net for missed
    /// events). `0` disables them.
    pub watched_rescan_hours: u64,
    /// Sources that are never indexed.
    pub exclude_sources: Vec<String>,
    /// Rows written per database transaction while scanning. Larger batches
    /// scan faster and use a little more memory.
    pub batch_size: usize,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            watch: true,
            rescan_minutes: 60,
            watched_rescan_hours: 24,
            exclude_sources: Vec::new(),
            batch_size: 500,
        }
    }
}

impl SearchSettings {
    /// Checks the values for mistakes.
    ///
    /// # Errors
    ///
    /// Fails on out-of-range numbers.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.rescan_minutes == 0 {
            anyhow::bail!("search.rescan_minutes must be at least 1");
        }
        if !(1..=10_000).contains(&self.batch_size) {
            anyhow::bail!("search.batch_size must be between 1 and 10000");
        }
        Ok(())
    }
}

/// A change to the index, published on [`Search::subscribe`].
///
/// Paths are real paths inside the source in display form (`/a/b.txt`).
/// Receivers that fall behind lose events (`RecvError::Lagged`) and should
/// treat the next [`IndexEvent::ScanDone`] as the point to resynchronise.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum IndexEvent {
    /// A file or folder was added or its size or modification time changed.
    Upsert {
        /// Source name.
        source: String,
        /// Path inside the source.
        path: String,
        /// Size in bytes (0 for folders).
        size: u64,
        /// Modification time as Unix seconds, if known.
        mtime: Option<i64>,
        /// `true` for folders.
        is_dir: bool,
    },
    /// A file or folder was removed. For a folder, everything under it went too.
    Remove {
        /// Source name.
        source: String,
        /// Path inside the source.
        path: String,
    },
    /// A file or folder (with everything under it) moved within the source.
    Rename {
        /// Source name.
        source: String,
        /// Old path.
        from: String,
        /// New path.
        to: String,
    },
    /// A full or partial scan of the source finished.
    ScanDone {
        /// Source name.
        source: String,
    },
}

/// Indexing status of one source, as shown by `GET /api/search/status`.
#[derive(Clone, Debug, Default, Serialize)]
pub struct SourceStatus {
    /// Source name.
    pub source: String,
    /// Number of indexed files and folders (refreshed after each scan).
    pub entries: i64,
    /// A full scan is running.
    pub scanning: bool,
    /// A file-system watcher keeps this source fresh.
    pub watching: bool,
    /// When the last full scan finished (Unix seconds).
    pub last_scan: Option<i64>,
    /// How long the last full scan took, in milliseconds.
    pub last_scan_ms: Option<u64>,
    /// The last scan or watcher error, if any.
    pub last_error: Option<String>,
}

/// Per-source control block shared by the scanner, the watcher and the API.
#[derive(Default)]
pub(crate) struct SourceCtl {
    /// Current scan generation; writes are stamped with it.
    pub generation: AtomicI64,
    /// Wakes the source's loop for an immediate full rescan.
    pub rescan: Notify,
    /// Serialises full scans of this source.
    pub scan_lock: tokio::sync::Mutex<()>,
    /// Status shown to users.
    pub status: Mutex<SourceStatus>,
}

/// Handle to the search subsystem, stored in [`AppState`]. Cheap to clone.
#[derive(Clone)]
pub struct Search(Arc<Inner>);

struct Inner {
    store: IndexStore,
    settings: SearchSettings,
    events: broadcast::Sender<IndexEvent>,
    sources: BTreeMap<String, Arc<SourceCtl>>,
}

impl Search {
    /// Creates the handle. No background work starts until [`Search::start`].
    ///
    /// `sources` lists the configured sources; excluded ones are dropped here.
    pub fn new(db: sqlx::SqlitePool, settings: SearchSettings, sources: &[String]) -> Self {
        let (events, _) = broadcast::channel(1024);
        let sources = sources
            .iter()
            .filter(|s| !settings.exclude_sources.contains(s))
            .map(|s| {
                let ctl = SourceCtl::default();
                lock(&ctl.status).source.clone_from(s);
                (s.clone(), Arc::new(ctl))
            })
            .collect();
        Self(Arc::new(Inner {
            store: IndexStore::new(db),
            settings,
            events,
            sources,
        }))
    }

    /// The settings in effect.
    pub fn settings(&self) -> &SearchSettings {
        &self.0.settings
    }

    /// The index tables.
    pub fn store(&self) -> &IndexStore {
        &self.0.store
    }

    /// Returns `true` if `source` is indexed.
    pub fn indexes(&self, source: &str) -> bool {
        self.0.settings.enabled && self.0.sources.contains_key(source)
    }

    /// Subscribes to index changes (see [`IndexEvent`]).
    pub fn subscribe(&self) -> broadcast::Receiver<IndexEvent> {
        self.0.events.subscribe()
    }

    /// Publishes an event; having no subscribers is not an error.
    pub(crate) fn publish(&self, ev: IndexEvent) {
        let _ = self.0.events.send(ev);
    }

    pub(crate) fn ctl(&self, source: &str) -> Option<&Arc<SourceCtl>> {
        self.0.sources.get(source)
    }

    /// The current generation of `source`, for writes outside a full scan.
    pub(crate) fn generation(&self, source: &str) -> i64 {
        self.ctl(source)
            .map_or(0, |c| c.generation.load(Ordering::SeqCst))
    }

    /// Status of every indexed source.
    pub fn status(&self) -> Vec<SourceStatus> {
        self.0
            .sources
            .values()
            .map(|c| lock(&c.status).clone())
            .collect()
    }

    /// Asks for an immediate full rescan of `source`. Returns `false` if the
    /// source is not indexed.
    pub fn request_rescan(&self, source: &str) -> bool {
        match self.ctl(source) {
            Some(c) if self.0.settings.enabled => {
                c.rescan.notify_one();
                true
            }
            _ => false,
        }
    }

    /// Starts background indexing: one task per source that scans at once,
    /// then watches (local sources) and rescans periodically.
    ///
    /// Must be called from within a Tokio runtime. Does nothing when search
    /// is disabled.
    pub fn start(&self, state: &AppState) {
        if !self.0.settings.enabled {
            tracing::info!("search indexing is disabled");
            return;
        }
        let keep: Vec<String> = self.0.sources.keys().cloned().collect();
        let me = self.clone();
        let state = state.clone();
        tokio::spawn(async move {
            let keep_refs: Vec<&str> = keep.iter().map(String::as_str).collect();
            match me.store().retain_sources(&keep_refs).await {
                Ok(n) if n > 0 => tracing::info!(rows = n, "dropped index rows of removed sources"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "could not prune the search index"),
            }
            for name in keep {
                let local_root = local_root(&state, &name);
                tokio::spawn(run_source(me.clone(), state.clone(), name, local_root));
            }
        });
    }
}

/// The canonical root of a local source, `None` for other backends.
fn local_root(state: &AppState, name: &str) -> Option<PathBuf> {
    state
        .config
        .sources
        .iter()
        .find(|s| s.name == name)
        .filter(|s| s.kind == SourceKind::Local)
        .and_then(|s| std::fs::canonicalize(&s.path).ok())
}

impl Search {
    /// Tells the index that `path` (a real path inside `source`) was created
    /// or changed through Filebrary: an upload, a new folder, the target of a
    /// copy or move. Folders are walked. Runs in the background and never
    /// fails the caller; errors are logged.
    ///
    /// Watched sources would notice anyway; calling this makes the change
    /// searchable at once and is what keeps unwatched sources fresh between
    /// rescans.
    pub fn changed(&self, state: &AppState, source: &str, path: &VPath) {
        let Some(job) = self.hook(state, source, path) else {
            return;
        };
        tokio::spawn(async move {
            if let Err(e) = job.refresh().await {
                tracing::warn!(source = %job.source.name, path = %job.path, error = %e, "updating the search index");
            }
        });
    }

    /// Tells the index that `path` (and everything under it) was deleted or
    /// moved to the trash. Runs in the background.
    pub fn removed(&self, state: &AppState, source: &str, path: &VPath) {
        let Some(job) = self.hook(state, source, path) else {
            return;
        };
        tokio::spawn(async move {
            match job.search.store().remove(&job.source.name, &job.path).await {
                Ok(n) if n > 0 => job.search.publish(IndexEvent::Remove {
                    source: job.source.name.clone(),
                    path: job.path.to_string(),
                }),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "updating the search index"),
            }
        });
    }

    /// Tells the index that `from` was renamed or moved to `to` within
    /// `source` (real paths). The indexed subtree moves in place; if `from`
    /// was not indexed, `to` is indexed from scratch. Runs in the background.
    pub fn renamed(&self, state: &AppState, source: &str, from: &VPath, to: &VPath) {
        let Some(job) = self.hook(state, source, to) else {
            return;
        };
        let from = from.clone();
        tokio::spawn(async move {
            let moved = match job
                .search
                .store()
                .rename(&job.source.name, &from, &job.path)
                .await
            {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(error = %e, "updating the search index");
                    false
                }
            };
            if moved {
                job.search.publish(IndexEvent::Rename {
                    source: job.source.name.clone(),
                    from: from.to_string(),
                    to: job.path.to_string(),
                });
            }
            if let Err(e) = job.refresh_shallow(!moved).await {
                tracing::warn!(error = %e, "updating the search index");
            }
        });
    }

    fn hook(&self, state: &AppState, source: &str, path: &VPath) -> Option<HookJob> {
        if !self.indexes(source) || path.is_root() || is_ignored(path) {
            return None;
        }
        Some(HookJob {
            search: self.clone(),
            source: state.source(source)?.clone(),
            local_root: local_root(state, source),
            path: path.clone(),
        })
    }
}

/// One background index update requested by a file operation.
struct HookJob {
    search: Search,
    source: crate::storage::Source,
    local_root: Option<PathBuf>,
    path: VPath,
}

impl HookJob {
    /// Re-reads the path; walks it if it is a folder.
    async fn refresh(&self) -> anyhow::Result<()> {
        self.refresh_shallow(true).await
    }

    /// Re-reads the path; walks it if it is a folder and `walk` is set.
    async fn refresh_shallow(&self, walk: bool) -> anyhow::Result<()> {
        let name = &self.source.name;
        let row = match &self.local_root {
            Some(root) => {
                let (root, path) = (root.clone(), self.path.clone());
                tokio::task::spawn_blocking(move || {
                    scan::local_row(&root, &scan::local_path(&root, &path), &path)
                })
                .await?
            }
            None => match self.source.storage.stat(&self.path).await {
                Ok(e) => Some((IndexRow::from_entry(&e), e.is_dir())),
                Err(crate::storage::StorageError::NotFound) => None,
                Err(e) => return Err(e.into()),
            },
        };
        let Some((row, descend)) = row else {
            if self.search.store().remove(name, &self.path).await? > 0 {
                self.search.publish(IndexEvent::Remove {
                    source: name.clone(),
                    path: self.path.to_string(),
                });
            }
            return Ok(());
        };
        let generation = self.search.generation(name);
        let out = self
            .search
            .store()
            .upsert_batch(name, std::slice::from_ref(&row), generation)
            .await?;
        for r in out.changed {
            self.search.publish(IndexEvent::Upsert {
                source: name.clone(),
                path: r.path,
                size: r.size,
                mtime: r.mtime,
                is_dir: r.is_dir,
            });
        }
        if walk && descend {
            scan::scan(
                &self.search,
                &self.source,
                self.local_root.as_deref(),
                &self.path,
            )
            .await?;
        }
        Ok(())
    }
}

/// The life of one source: initial scan, watcher, periodic rescans.
async fn run_source(search: Search, state: AppState, name: String, local_root: Option<PathBuf>) {
    let Some(ctl) = search.ctl(&name).cloned() else {
        return;
    };
    let Some(source) = state.source(&name).cloned() else {
        return;
    };
    match search.store().max_generation(&name).await {
        Ok(g) => ctl.generation.store(g, Ordering::SeqCst),
        Err(e) => tracing::warn!(source = %name, error = %e, "reading index generation"),
    }

    // Start watching before the first scan so that nothing changed during the
    // scan is missed (the watcher's writes carry the newest generation).
    let _watcher = match (&local_root, search.settings().watch) {
        (Some(root), true) => match watch::spawn(search.clone(), source.clone(), root.clone()) {
            Ok(w) => {
                lock(&ctl.status).watching = true;
                Some(w)
            }
            Err(e) => {
                tracing::warn!(
                    source = %name, error = %e,
                    "cannot watch this source; relying on periodic rescans \
                     (on Linux, raise fs.inotify.max_user_watches)"
                );
                lock(&ctl.status).last_error = Some(format!("watcher: {e}"));
                None
            }
        },
        _ => None,
    };
    let interval = if _watcher.is_some() {
        (search.settings().watched_rescan_hours > 0)
            .then(|| Duration::from_secs(search.settings().watched_rescan_hours * 3600))
    } else {
        Some(Duration::from_secs(search.settings().rescan_minutes * 60))
    };

    loop {
        if let Err(e) = scan::full_scan(&search, &source, local_root.as_deref()).await {
            tracing::warn!(source = %name, error = %e, "index scan failed");
        }
        match interval {
            Some(d) => {
                tokio::select! {
                    () = tokio::time::sleep(d) => {}
                    () = ctl.rescan.notified() => {}
                }
            }
            None => ctl.rescan.notified().await,
        }
    }
}

/// Locks a std mutex, recovering from poisoning (status data stays usable).
pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_trash_and_temp_files() {
        assert!(is_ignored(&VPath::parse(".filebrary-trash").unwrap()));
        assert!(is_ignored(
            &VPath::parse(".filebrary-trash/1/a.txt").unwrap()
        ));
        assert!(is_ignored(&VPath::parse("a/.x.txt.0123.fbtmp").unwrap()));
        assert!(!is_ignored(&VPath::parse("a/.filebrary-trash").unwrap()));
        assert!(!is_ignored(&VPath::parse("a/b.txt").unwrap()));
    }

    #[test]
    fn settings_validate() {
        SearchSettings::default().validate().unwrap();
        let bad = SearchSettings {
            batch_size: 0,
            ..SearchSettings::default()
        };
        assert!(bad.validate().is_err());
    }
}
