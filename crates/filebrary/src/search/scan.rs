//! Walking a source and writing what is found into the index.
//!
//! Two walkers produce batches of [`IndexRow`]s into a short channel, and one
//! writer drains it into SQLite, so memory stays bounded by
//! `batch_size × CHANNEL_DEPTH` rows plus the stack of folders still to visit:
//!
//! * [`walk_local`] reads a local directory with `std::fs` on a blocking
//!   thread. It never descends into symlinked folders (which could form
//!   cycles; their targets inside the source are indexed under their real
//!   paths anyway) and skips symlinks that lead outside the source root,
//!   mirroring what the local backend shows in listings.
//! * [`walk_storage`] uses only the [`Storage`](crate::storage::Storage) trait,
//!   for backends without a local path (S3, SFTP, SMB).

use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::Instant,
};

use tokio::sync::mpsc;

use super::{IndexEvent, IndexRow, Search, is_ignored, lock};
use crate::storage::{Source, VPath};

/// Batches in flight between a walker and the writer.
const CHANNEL_DEPTH: usize = 4;

/// What a walker sends to the writer.
#[derive(Debug)]
pub enum Batch {
    /// Entries found.
    Rows(Vec<IndexRow>),
    /// A folder that could not be listed this time: keep what is indexed under
    /// it rather than sweep it away because of a transient error.
    Keep(VPath),
}

/// Totals for one scan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanStats {
    /// Entries seen.
    pub seen: u64,
    /// Entries added or changed.
    pub changed: u64,
    /// Entries removed because they no longer exist.
    pub removed: u64,
}

/// Rescans a whole source, updating its status. Waits if a full scan of the
/// same source is already running.
///
/// # Errors
///
/// Fails on database errors or if the source root cannot be listed.
pub async fn full_scan(
    search: &Search,
    source: &Source,
    local_root: Option<&Path>,
) -> anyhow::Result<ScanStats> {
    let Some(ctl) = search.ctl(&source.name).cloned() else {
        anyhow::bail!("source {} is not indexed", source.name);
    };
    let _guard = ctl.scan_lock.lock().await;
    lock(&ctl.status).scanning = true;
    let started = Instant::now();
    let result = scan(search, source, local_root, &VPath::root()).await;
    let entries = search.store().count(&source.name).await.unwrap_or(0);
    {
        let mut st = lock(&ctl.status);
        st.scanning = false;
        st.entries = entries;
        match &result {
            Ok(stats) => {
                st.last_scan = Some(crate::db::now());
                st.last_scan_ms =
                    Some(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
                st.last_error = None;
                tracing::info!(
                    source = %source.name, seen = stats.seen, changed = stats.changed,
                    removed = stats.removed, ms = st.last_scan_ms, "index scan finished"
                );
            }
            Err(e) => st.last_error = Some(e.to_string()),
        }
    }
    result
}

/// Scans `under` (a folder, or the root) and everything below it.
///
/// Rows are stamped with a fresh generation; when the walk completes, rows
/// under `under` with an older generation are removed. `under` itself is not
/// written (the caller indexes it if it is not the root).
///
/// # Errors
///
/// Fails on database errors or if `under` cannot be listed.
pub async fn scan(
    search: &Search,
    source: &Source,
    local_root: Option<&Path>,
    under: &VPath,
) -> anyhow::Result<ScanStats> {
    let ctl = search
        .ctl(&source.name)
        .ok_or_else(|| anyhow::anyhow!("source {} is not indexed", source.name))?;
    let generation = ctl.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let batch = search.settings().batch_size;
    let (tx, mut rx) = mpsc::channel::<Batch>(CHANNEL_DEPTH);

    let walker = match local_root {
        Some(root) => {
            let (root, under) = (root.to_path_buf(), under.clone());
            tokio::task::spawn_blocking(move || walk_local(&root, &under, batch, &tx))
        }
        None => {
            let (source, under) = (source.clone(), under.clone());
            tokio::spawn(async move { walk_storage(&source, &under, batch, &tx).await })
        }
    };

    let mut stats = ScanStats::default();
    let mut write_err = None;
    while let Some(msg) = rx.recv().await {
        let rows = match msg {
            Batch::Rows(rows) => rows,
            Batch::Keep(dir) => {
                if let Err(e) = search.store().touch(&source.name, &dir, generation).await {
                    write_err = Some(e);
                    break;
                }
                continue;
            }
        };
        stats.seen += rows.len() as u64;
        match search
            .store()
            .upsert_batch(&source.name, &rows, generation)
            .await
        {
            Ok(out) => {
                stats.changed += out.changed.len() as u64;
                for r in out.changed {
                    search.publish(IndexEvent::Upsert {
                        source: source.name.clone(),
                        path: r.path,
                        size: r.size,
                        mtime: r.mtime,
                        is_dir: r.is_dir,
                    });
                }
            }
            Err(e) => {
                write_err = Some(e);
                break; // dropping rx stops the walker
            }
        }
    }
    drop(rx);
    let walked = walker.await;
    if let Some(e) = write_err {
        return Err(e.into());
    }
    match walked {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e),
        Err(e) => anyhow::bail!("index walker panicked: {e}"),
    }

    // Only a complete walk may sweep, or a failed listing would wipe rows.
    let gone = search
        .store()
        .sweep(&source.name, under, generation)
        .await?;
    stats.removed = gone.len() as u64;
    for path in gone {
        search.publish(IndexEvent::Remove {
            source: source.name.clone(),
            path,
        });
    }
    search.publish(IndexEvent::ScanDone {
        source: source.name.clone(),
    });
    Ok(stats)
}

/// Builds the row for a local path, or `None` if it must not be indexed
/// (vanished, non-UTF-8, symlink outside the root). A symlink inside the root
/// is indexed with its target's metadata and the target's real path, which
/// search uses to hide the link from users whose scope does not contain it. The flag says whether the
/// walk may descend into it (a real folder, not a symlinked one).
pub(crate) fn local_row(root: &Path, abs: &Path, vp: &VPath) -> Option<(IndexRow, bool)> {
    let lmeta = std::fs::symlink_metadata(abs).ok()?;
    let (meta, descend, target) = if lmeta.file_type().is_symlink() {
        let real = std::fs::canonicalize(abs).ok()?;
        // Outside the root (or not UTF-8): hidden, like in listings.
        let rel = real.strip_prefix(root).ok()?;
        let mut t = String::new();
        for c in rel.components() {
            t.push('/');
            t.push_str(c.as_os_str().to_str()?);
        }
        let target = VPath::parse(&t).ok()?.to_string();
        (std::fs::metadata(&real).ok()?, false, Some(target))
    } else {
        let d = lmeta.is_dir();
        (lmeta, d, None)
    };
    let is_dir = meta.is_dir();
    Some((
        IndexRow {
            path: vp.to_string(),
            name: vp.name()?.to_owned(),
            is_dir,
            size: if is_dir { 0 } else { meta.len() },
            mtime: meta
                .modified()
                .ok()
                .map(|t| time::OffsetDateTime::from(t).unix_timestamp()),
            target,
        },
        descend && is_dir,
    ))
}

/// The absolute path of `vp` under `root`.
pub(crate) fn local_path(root: &Path, vp: &VPath) -> PathBuf {
    let mut p = root.to_path_buf();
    for c in vp.components() {
        p.push(c);
    }
    p
}

/// Walks a local folder tree (blocking). See the module docs.
///
/// # Errors
///
/// Fails only if `under` itself cannot be read; unreadable subfolders are
/// logged and their existing rows kept ([`Batch::Keep`]).
pub fn walk_local(
    root: &Path,
    under: &VPath,
    batch: usize,
    tx: &mpsc::Sender<Batch>,
) -> anyhow::Result<()> {
    let mut buf = Vec::with_capacity(batch);
    let mut stack = vec![under.clone()];
    let mut first = true;
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(local_path(root, &dir)) {
            Ok(rd) => rd,
            Err(e) if first => anyhow::bail!("listing {dir}: {e}"),
            Err(e) => {
                tracing::debug!(%dir, error = %e, "skipping unreadable folder");
                if tx.blocking_send(Batch::Keep(dir)).is_err() {
                    return Ok(());
                }
                continue;
            }
        };
        first = false;
        for ent in rd.flatten() {
            let Some(name) = ent.file_name().to_str().map(str::to_owned) else {
                continue; // non-UTF-8 names cannot be addressed
            };
            let Ok(child) = dir.join(&name) else {
                continue;
            };
            if is_ignored(&child) {
                continue;
            }
            let Some((row, descend)) = local_row(root, &ent.path(), &child) else {
                continue;
            };
            buf.push(row);
            if descend {
                stack.push(child);
            }
            if buf.len() >= batch
                && tx
                    .blocking_send(Batch::Rows(std::mem::take(&mut buf)))
                    .is_err()
            {
                return Ok(()); // the writer stopped; it reports its own error
            }
        }
    }
    if !buf.is_empty() {
        let _ = tx.blocking_send(Batch::Rows(buf));
    }
    Ok(())
}

/// Walks a source through the storage trait (for non-local backends).
///
/// # Errors
///
/// Fails only if `under` itself cannot be listed.
pub async fn walk_storage(
    source: &Source,
    under: &VPath,
    batch: usize,
    tx: &mpsc::Sender<Batch>,
) -> anyhow::Result<()> {
    let mut buf = Vec::with_capacity(batch);
    let mut stack = vec![under.clone()];
    let mut first = true;
    while let Some(dir) = stack.pop() {
        let entries = match source.storage.list(&dir).await {
            Ok(e) => e,
            Err(e) if first => anyhow::bail!("listing {dir}: {e}"),
            Err(e) => {
                tracing::debug!(%dir, error = %e, "skipping unreadable folder");
                if tx.send(Batch::Keep(dir)).await.is_err() {
                    return Ok(());
                }
                continue;
            }
        };
        first = false;
        for e in entries {
            if is_ignored(&e.path) {
                continue;
            }
            if e.is_dir() {
                stack.push(e.path.clone());
            }
            buf.push(IndexRow::from_entry(&e));
            if buf.len() >= batch
                && tx
                    .send(Batch::Rows(std::mem::take(&mut buf)))
                    .await
                    .is_err()
            {
                return Ok(());
            }
        }
    }
    if !buf.is_empty() {
        let _ = tx.send(Batch::Rows(buf)).await;
    }
    Ok(())
}
