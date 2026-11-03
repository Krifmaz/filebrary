//! Keeping local sources fresh with file-system notifications.
//!
//! The `notify` crate delivers events on its own thread. They are pushed into
//! a bounded channel (so an event storm cannot grow memory without limit) and
//! handled by one task per source, which waits [`DEBOUNCE`] after the first
//! event to coalesce bursts, then for every touched path:
//!
//! * gone: removes it and everything under it from the index;
//! * a folder that was created or moved in: indexes it and walks it;
//! * anything else: re-reads its metadata and updates the row.
//!
//! Renames reported with both ends (`RenameMode::Both`, as inotify does) move
//! the indexed subtree in place instead of re-walking it.
//!
//! If the channel overflows or the OS reports that events were lost
//! (`Flag::Rescan`, e.g. an inotify queue overflow), a full rescan is requested.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use notify::{
    EventKind, RecommendedWatcher, RecursiveMode, Watcher,
    event::{CreateKind, Flag, ModifyKind, RenameMode},
};
use tokio::sync::mpsc;

use super::{IndexEvent, Search, is_ignored, lock, scan};
use crate::storage::{Source, VPath};

/// How long to wait after an event for more to arrive before acting.
pub const DEBOUNCE: Duration = Duration::from_millis(300);

/// Events buffered between the OS watcher thread and the handler task.
const QUEUE: usize = 8192;

/// Most distinct paths handled per debounce window; beyond this a full rescan
/// is cheaper than path-by-path updates.
const MAX_DIRTY: usize = 20_000;

/// A running watcher. Dropping it stops the OS watch and the handler task.
pub struct WatchHandle {
    _watcher: RecommendedWatcher,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Starts watching the local `source` rooted at `root` (canonical).
///
/// # Errors
///
/// Fails if the OS watch cannot be set up (for example when the inotify watch
/// limit is reached).
pub fn spawn(search: Search, source: Source, root: PathBuf) -> notify::Result<WatchHandle> {
    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>(QUEUE);
    let overflow = Arc::new(AtomicBool::new(false));
    let of = overflow.clone();
    let mut watcher = notify::recommended_watcher(move |ev| {
        if tx.try_send(ev).is_err() {
            of.store(true, Ordering::SeqCst);
        }
    })?;
    watcher.watch(&root, RecursiveMode::Recursive)?;
    let task = tokio::spawn(handle_events(search, source, root, rx, overflow));
    Ok(WatchHandle {
        _watcher: watcher,
        task,
    })
}

/// What to do with one path after a debounce window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Dirty {
    /// Walk it if it is a folder (it is new to this place).
    walk: bool,
}

/// Paths and renames collected in one debounce window.
#[derive(Debug, Default)]
struct Window {
    dirty: BTreeMap<VPath, Dirty>,
    renames: Vec<(VPath, VPath)>,
    rescan: bool,
}

impl Window {
    fn add(&mut self, root: &Path, ev: notify::Event) {
        if ev.need_rescan() || ev.attrs.flag() == Some(Flag::Rescan) {
            self.rescan = true;
            return;
        }
        let walk = matches!(
            ev.kind,
            EventKind::Create(CreateKind::Folder | CreateKind::Any | CreateKind::Other)
                | EventKind::Modify(ModifyKind::Name(RenameMode::To | RenameMode::Any))
        );
        if let EventKind::Modify(ModifyKind::Name(RenameMode::Both)) = ev.kind
            && let [from, to] = ev.paths.as_slice()
            && let (Some(f), Some(t)) = (to_vpath(root, from), to_vpath(root, to))
        {
            self.renames.push((f, t));
            return;
        }
        if matches!(ev.kind, EventKind::Access(_)) {
            return;
        }
        for p in &ev.paths {
            if let Some(vp) = to_vpath(root, p) {
                let d = self.dirty.entry(vp).or_default();
                d.walk |= walk;
            }
        }
        if self.dirty.len() > MAX_DIRTY {
            self.rescan = true;
        }
    }
}

/// Maps an absolute path under `root` to a source path. Returns `None` for
/// the root itself, paths outside it, non-UTF-8 names and ignored paths.
fn to_vpath(root: &Path, abs: &Path) -> Option<VPath> {
    let rel = abs.strip_prefix(root).ok()?;
    let mut s = String::new();
    for c in rel.components() {
        let std::path::Component::Normal(c) = c else {
            return None;
        };
        s.push('/');
        s.push_str(c.to_str()?);
    }
    let vp = VPath::parse(&s).ok()?;
    (!vp.is_root() && !is_ignored(&vp)).then_some(vp)
}

async fn handle_events(
    search: Search,
    source: Source,
    root: PathBuf,
    mut rx: mpsc::Receiver<notify::Result<notify::Event>>,
    overflow: Arc<AtomicBool>,
) {
    let name = source.name.clone();
    while let Some(first) = rx.recv().await {
        let mut win = Window::default();
        let push = |r: notify::Result<notify::Event>, win: &mut Window| match r {
            Ok(ev) => win.add(&root, ev),
            Err(e) => {
                tracing::warn!(source = %name, error = %e, "watcher error; rescanning");
                win.rescan = true;
            }
        };
        push(first, &mut win);
        let deadline = tokio::time::Instant::now() + DEBOUNCE;
        while let Ok(Some(r)) = tokio::time::timeout_at(deadline, rx.recv()).await {
            push(r, &mut win);
        }
        if overflow.swap(false, Ordering::SeqCst) {
            win.rescan = true;
        }
        if win.rescan {
            search.request_rescan(&name);
            continue;
        }
        if let Err(e) = apply(&search, &source, &root, win).await {
            tracing::warn!(source = %name, error = %e, "applying file changes to the index");
            if let Some(ctl) = search.ctl(&name) {
                lock(&ctl.status).last_error = Some(e.to_string());
            }
        }
    }
}

async fn apply(
    search: &Search,
    source: &Source,
    root: &Path,
    mut win: Window,
) -> anyhow::Result<()> {
    let name = &source.name;
    for (from, to) in &win.renames {
        if search.store().rename(name, from, to).await? {
            search.publish(IndexEvent::Rename {
                source: name.clone(),
                from: from.to_string(),
                to: to.to_string(),
            });
            // Already moved in place: just refresh the top entry.
            win.dirty.entry(to.clone()).or_default().walk = false;
        } else {
            // Not indexed under the old name (e.g. moved in from an ignored
            // place): index the new location from scratch.
            win.dirty.entry(to.clone()).or_default().walk = true;
        }
        win.dirty.entry(from.clone()).or_default();
    }

    let generation = search.generation(name);
    let mut walks = Vec::new();
    for (vp, d) in win.dirty {
        let abs = scan::local_path(root, &vp);
        let row = {
            let (root, abs, vp) = (root.to_path_buf(), abs.clone(), vp.clone());
            tokio::task::spawn_blocking(move || scan::local_row(&root, &abs, &vp)).await?
        };
        match row {
            None => {
                if search.store().remove(name, &vp).await? > 0 {
                    search.publish(IndexEvent::Remove {
                        source: name.clone(),
                        path: vp.to_string(),
                    });
                }
            }
            Some((row, descend)) => {
                let out = search
                    .store()
                    .upsert_batch(name, std::slice::from_ref(&row), generation)
                    .await?;
                for r in out.changed {
                    search.publish(IndexEvent::Upsert {
                        source: name.clone(),
                        path: r.path,
                        size: r.size,
                        mtime: r.mtime,
                        is_dir: r.is_dir,
                    });
                }
                if d.walk && descend {
                    walks.push(vp);
                }
            }
        }
    }
    // Walk new folders, skipping any nested inside another one being walked.
    walks.sort();
    let mut last: Option<VPath> = None;
    for vp in walks {
        if last.as_ref().is_some_and(|l| vp.starts_with(l)) {
            continue;
        }
        scan::scan(search, source, Some(root), &vp).await?;
        last = Some(vp);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_paths_under_the_root() {
        let root = Path::new("/srv/files");
        assert_eq!(
            to_vpath(root, Path::new("/srv/files/a/b.txt"))
                .unwrap()
                .to_string(),
            "/a/b.txt"
        );
        assert!(to_vpath(root, Path::new("/srv/files")).is_none());
        assert!(to_vpath(root, Path::new("/srv/other/x")).is_none());
        assert!(to_vpath(root, Path::new("/srv/files/.filebrary-trash/1")).is_none());
    }
}
