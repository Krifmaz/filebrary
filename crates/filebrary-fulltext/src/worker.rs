//! The background task that keeps the content index in step with storage.
//!
//! The file-name indexer (scan, file watcher, periodic rescan) publishes
//! change events. [`run`] consumes them, and for every changed file in a
//! source with content search switched on it:
//!
//! 1. skips it if it is a directory, too large, of an unsupported type, or
//!    unchanged since it was last indexed (same size and mtime);
//! 2. reads it through the [`ContentFetcher`] (never through a shell);
//! 3. extracts its text on a blocking thread;
//! 4. adds it to the index, committing in batches.
//!
//! Removes and renames are applied directly. Only one file is held in memory
//! at a time, so peak memory while indexing is roughly the writer arena plus
//! the largest file read plus its extracted text.

use std::collections::HashSet;
use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

use crate::extract::{detect_kind, extract_text};
use crate::index::ContentIndex;

/// A change reported by the file-name indexer. Paths are source-relative,
/// use forward slashes and have no leading slash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentEvent {
    /// A file or directory was found, created or changed.
    Upsert {
        /// Source name.
        source: String,
        /// Path inside the source.
        path: String,
        /// Size in bytes.
        size: u64,
        /// Modification time, Unix seconds.
        mtime: i64,
        /// Whether this is a directory (ignored by the content index).
        is_dir: bool,
    },
    /// A file, or a directory with everything below it, disappeared.
    Remove {
        /// Source name.
        source: String,
        /// Path inside the source.
        path: String,
    },
    /// A file or directory was renamed or moved within one source.
    Rename {
        /// Source name.
        source: String,
        /// Old path.
        from: String,
        /// New path.
        to: String,
    },
    /// A full scan of a source finished; pending changes are committed.
    ScanDone {
        /// Source name.
        source: String,
    },
}

/// One file as listed by [`ContentFetcher::list_files`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedFile {
    /// Path inside the source.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// Modification time, Unix seconds.
    pub mtime: i64,
}

/// What the worker needs from the server: the per-source switch, a way to
/// read file content, and a listing of known files for reconciliation.
pub trait ContentFetcher: Send + Sync + 'static {
    /// Whether content search is switched on for `source`.
    fn enabled(&self, source: &str) -> bool;

    /// The sources with content search switched on.
    fn sources(&self) -> Vec<String>;

    /// Reads at most `max_bytes` of a file. Returning more is an error the
    /// worker guards against; returning an error skips the file.
    fn read(
        &self,
        source: &str,
        path: &str,
        max_bytes: u64,
    ) -> impl Future<Output = io::Result<Vec<u8>>> + Send;

    /// Lists up to `limit` files of `source` whose path sorts after `after`
    /// (`""` for the first page), in path order. Directories are left out.
    ///
    /// The worker walks this listing to catch up on files it never got an
    /// event for: at startup (files indexed by name before content search
    /// was switched on) and after it fell behind the event stream.
    fn list_files(
        &self,
        source: &str,
        after: &str,
        limit: usize,
    ) -> impl Future<Output = io::Result<Vec<ListedFile>>> + Send;
}

/// Drops the documents of every indexed source for which `enabled` is false
/// (content search switched off, or the source no longer exists). Call once
/// at startup, before [`run`].
///
/// # Errors
///
/// Fails if the index cannot be read or written.
pub fn prune_disabled_sources(
    index: &ContentIndex,
    enabled: impl Fn(&str) -> bool,
) -> crate::IndexResult<Vec<String>> {
    let mut dropped = Vec::new();
    for s in index.indexed_sources()? {
        if !enabled(&s) {
            index.remove_source(&s)?;
            dropped.push(s);
        }
    }
    index.commit()?;
    Ok(dropped)
}

/// Runs a blocking index operation off the async runtime, logging failures.
async fn blocking(
    index: &Arc<ContentIndex>,
    what: &'static str,
    f: impl FnOnce(&ContentIndex) -> crate::IndexResult<()> + Send + 'static,
) {
    let index = Arc::clone(index);
    match tokio::task::spawn_blocking(move || f(&index)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "content index {what} failed"),
        Err(e) => tracing::error!(error = %e, "content index {what} task failed"),
    }
}

/// Batches commits: counts changes and remembers when the first
/// uncommitted one happened.
struct Batcher {
    index: Arc<ContentIndex>,
    every: usize,
    interval: Duration,
    pending: usize,
    deadline: Option<Instant>,
}

impl Batcher {
    async fn record(&mut self, h: Handled) {
        match h {
            Handled::Changed => {
                self.pending += 1;
                self.deadline
                    .get_or_insert_with(|| Instant::now() + self.interval);
            }
            Handled::CommitNow => self.pending = self.every,
            Handled::Skipped => {}
        }
        if self.pending >= self.every {
            self.commit().await;
        }
    }

    async fn commit(&mut self) {
        blocking(&self.index, "commit", |i| i.commit()).await;
        self.pending = 0;
        self.deadline = None;
    }
}

/// Page size used when reconciling a source against its file listing.
const RECONCILE_PAGE: usize = 1000;

/// Walks the full file listing of `source` and indexes whatever is missing
/// or changed (unchanged files cost one index lookup each).
async fn reconcile<F: ContentFetcher>(
    index: &Arc<ContentIndex>,
    fetcher: &F,
    batch: &mut Batcher,
    source: &str,
) {
    let cfg = index.config().clone();
    let mut after = String::new();
    let mut seen = 0u64;
    loop {
        let page = match fetcher.list_files(source, &after, RECONCILE_PAGE).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(%source, error = %e, "content index: cannot list files to reconcile");
                return;
            }
        };
        let Some(last) = page.last() else { break };
        after = last.path.clone();
        let full = page.len() >= RECONCILE_PAGE;
        for f in page {
            seen += 1;
            let ev = ContentEvent::Upsert {
                source: source.to_owned(),
                path: f.path,
                size: f.size,
                mtime: f.mtime,
                is_dir: false,
            };
            let h = handle(index, fetcher, ev, cfg.max_file_bytes(), cfg.max_chars).await;
            batch.record(h).await;
        }
        if !full {
            break;
        }
    }
    batch.commit().await;
    tracing::info!(%source, files = seen, "content index reconciled");
}

/// Consumes events until the channel closes, keeping `index` up to date.
///
/// `E` is the event type of the file-name indexer; `map` converts it into a
/// [`ContentEvent`] (returning `None` ignores the event).
///
/// Events alone are not enough to stay complete: the file-name indexer only
/// reports changes, and a slow worker can miss events when the broadcast
/// channel overflows. So every enabled source is *reconciled* (its full file
/// listing walked through [`ContentFetcher::list_files`]) after its first
/// [`ContentEvent::ScanDone`], and again after the next `ScanDone` following
/// any missed events. Stale documents never surface because search results
/// are always intersected with the file-name index.
pub async fn run<E, F, M>(
    index: Arc<ContentIndex>,
    fetcher: Arc<F>,
    mut rx: broadcast::Receiver<E>,
    map: M,
) where
    E: Clone + Send + 'static,
    F: ContentFetcher,
    M: Fn(E) -> Option<ContentEvent> + Send + 'static,
{
    let cfg = index.config().clone();
    let mut batch = Batcher {
        index: Arc::clone(&index),
        every: cfg.commit_every_docs.max(1),
        interval: Duration::from_secs(cfg.commit_interval_secs.max(1)),
        pending: 0,
        deadline: None,
    };
    let mut to_reconcile: HashSet<String> = fetcher.sources().into_iter().collect();

    loop {
        let event = match batch.deadline {
            Some(d) => tokio::select! {
                ev = rx.recv() => ev,
                () = tokio::time::sleep_until(d) => {
                    batch.commit().await;
                    continue;
                }
            },
            None => rx.recv().await,
        };
        let event = match event {
            Ok(e) => match map(e) {
                Some(e) => e,
                None => continue,
            },
            Err(RecvError::Lagged(n)) => {
                tracing::warn!(
                    missed = n,
                    "content indexer fell behind; sources are reconciled after their next scan"
                );
                to_reconcile.extend(fetcher.sources());
                continue;
            }
            Err(RecvError::Closed) => break,
        };

        let done = match &event {
            ContentEvent::ScanDone { source } => to_reconcile.take(source),
            _ => None,
        };
        let h = handle(
            &index,
            &*fetcher,
            event,
            cfg.max_file_bytes(),
            cfg.max_chars,
        )
        .await;
        batch.record(h).await;
        if let Some(source) = done
            && fetcher.enabled(&source)
        {
            reconcile(&index, &*fetcher, &mut batch, &source).await;
        }
    }
    batch.commit().await;
}

/// The outcome of one event.
enum Handled {
    /// The index changed; commit eventually.
    Changed,
    /// Commit as soon as possible (end of a scan, or a rename that already
    /// committed).
    CommitNow,
    /// Nothing to do.
    Skipped,
}

async fn handle<F: ContentFetcher>(
    index: &Arc<ContentIndex>,
    fetcher: &F,
    event: ContentEvent,
    max_bytes: u64,
    max_chars: usize,
) -> Handled {
    match event {
        ContentEvent::Upsert {
            source,
            path,
            size,
            mtime,
            is_dir,
        } => {
            if is_dir
                || size > max_bytes
                || !fetcher.enabled(&source)
                || detect_kind(&path).is_none()
            {
                return Handled::Skipped;
            }
            let check = {
                let (index, source, path) = (Arc::clone(index), source.clone(), path.clone());
                tokio::task::spawn_blocking(move || index.needs_update(&source, &path, size, mtime))
                    .await
            };
            if !matches!(check, Ok(Ok(true))) {
                return Handled::Skipped;
            }
            let bytes = match fetcher.read(&source, &path, max_bytes).await {
                Ok(b) if b.len() as u64 <= max_bytes => b,
                Ok(_) => return Handled::Skipped,
                Err(e) => {
                    tracing::debug!(%source, %path, error = %e, "content index: cannot read file");
                    return Handled::Skipped;
                }
            };
            let index = Arc::clone(index);
            let res = tokio::task::spawn_blocking(move || {
                // An unreadable or unsupported file is still recorded (with no
                // text) so it is not re-read on every rescan.
                let text = match extract_text(&path, &bytes, max_chars) {
                    Ok(t) => t.unwrap_or_default(),
                    Err(e) => {
                        tracing::debug!(%source, %path, error = %e, "content index: cannot extract text");
                        String::new()
                    }
                };
                drop(bytes);
                index.upsert(&source, &path, size, mtime, &text)
            })
            .await;
            match res {
                Ok(Ok(())) => Handled::Changed,
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "content index upsert failed");
                    Handled::Skipped
                }
                Err(e) => {
                    tracing::error!(error = %e, "content index extraction task failed");
                    Handled::Skipped
                }
            }
        }
        ContentEvent::Remove { source, path } => {
            blocking(index, "remove", move |i| i.remove(&source, &path)).await;
            Handled::Changed
        }
        ContentEvent::Rename { source, from, to } => {
            if !fetcher.enabled(&source) {
                return Handled::Skipped;
            }
            blocking(index, "rename", move |i| i.rename(&source, &from, &to)).await;
            Handled::Skipped
        }
        ContentEvent::ScanDone { .. } => {
            if index.has_pending() {
                Handled::CommitNow
            } else {
                Handled::Skipped
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FulltextConfig;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Fake {
        files: Mutex<HashMap<(String, String), Vec<u8>>>,
        reads: AtomicUsize,
    }

    impl ContentFetcher for Fake {
        fn enabled(&self, source: &str) -> bool {
            source != "off"
        }
        fn sources(&self) -> Vec<String> {
            vec!["s".into()]
        }
        async fn list_files(
            &self,
            source: &str,
            after: &str,
            limit: usize,
        ) -> io::Result<Vec<ListedFile>> {
            let f = self.files.lock().unwrap();
            let mut v: Vec<ListedFile> = f
                .iter()
                .filter(|((s, p), _)| s == source && p.as_str() > after)
                .map(|((_, p), b)| ListedFile {
                    path: p.clone(),
                    size: b.len() as u64,
                    mtime: 1,
                })
                .collect();
            v.sort_by(|a, b| a.path.cmp(&b.path));
            v.truncate(limit);
            Ok(v)
        }
        async fn read(&self, source: &str, path: &str, _max: u64) -> io::Result<Vec<u8>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.files
                .lock()
                .unwrap()
                .get(&(source.to_owned(), path.to_owned()))
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
    }

    fn up(source: &str, path: &str, size: u64, mtime: i64) -> ContentEvent {
        ContentEvent::Upsert {
            source: source.into(),
            path: path.into(),
            size,
            mtime,
            is_dir: false,
        }
    }

    #[tokio::test]
    async fn indexes_skips_and_follows_changes() {
        let index = Arc::new(ContentIndex::in_memory(FulltextConfig::default()).unwrap());
        let fake = Arc::new(Fake::default());
        {
            let mut f = fake.files.lock().unwrap();
            f.insert(("s".into(), "a.md".into()), b"hello world".to_vec());
            f.insert(("s".into(), "pic.jpg".into()), b"hello".to_vec());
            f.insert(("off".into(), "a.md".into()), b"hello".to_vec());
        }
        let (tx, rx) = broadcast::channel::<ContentEvent>(64);
        let task = tokio::spawn(run(Arc::clone(&index), Arc::clone(&fake), rx, Some));
        tx.send(up("s", "a.md", 11, 1)).unwrap();
        tx.send(up("s", "pic.jpg", 5, 1)).unwrap(); // unsupported type
        tx.send(up("off", "a.md", 5, 1)).unwrap(); // source switched off
        tx.send(ContentEvent::Upsert {
            source: "s".into(),
            path: "dir".into(),
            size: 0,
            mtime: 1,
            is_dir: true,
        })
        .unwrap();
        tx.send(up("s", "huge.txt", u64::MAX, 1)).unwrap(); // too large
        tx.send(ContentEvent::ScanDone { source: "s".into() })
            .unwrap();
        // Same version again: skipped without reading.
        tx.send(up("s", "a.md", 11, 1)).unwrap();
        tx.send(ContentEvent::Rename {
            source: "s".into(),
            from: "a.md".into(),
            to: "b/a.md".into(),
        })
        .unwrap();
        tx.send(ContentEvent::ScanDone { source: "s".into() })
            .unwrap();
        drop(tx);
        task.await.unwrap();

        assert_eq!(fake.reads.load(Ordering::SeqCst), 1);
        let hits = index.search("hello", &["s", "off"], 10, 0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "b/a.md");
    }

    #[tokio::test]
    async fn remove_and_prune() {
        let index = Arc::new(ContentIndex::in_memory(FulltextConfig::default()).unwrap());
        index.upsert("s", "d/x.txt", 1, 1, "kept?").unwrap();
        index.upsert("gone", "y.txt", 1, 1, "kept?").unwrap();
        index.commit().unwrap();
        let dropped = prune_disabled_sources(&index, |s| s == "s").unwrap();
        assert_eq!(dropped, ["gone"]);

        let (tx, rx) = broadcast::channel::<ContentEvent>(8);
        let task = tokio::spawn(run(Arc::clone(&index), Arc::new(Fake::default()), rx, Some));
        tx.send(ContentEvent::Remove {
            source: "s".into(),
            path: "d".into(),
        })
        .unwrap();
        drop(tx);
        task.await.unwrap();
        assert_eq!(index.doc_count(None).unwrap(), 0);
    }

    #[tokio::test]
    async fn commits_on_interval() {
        let cfg = FulltextConfig {
            commit_interval_secs: 1,
            ..Default::default()
        };
        let index = Arc::new(ContentIndex::in_memory(cfg).unwrap());
        let fake = Arc::new(Fake::default());
        fake.files
            .lock()
            .unwrap()
            .insert(("s".into(), "a.txt".into()), b"tick".to_vec());
        let (tx, rx) = broadcast::channel::<ContentEvent>(8);
        let task = tokio::spawn(run(Arc::clone(&index), fake, rx, Some));
        tx.send(up("s", "a.txt", 4, 1)).unwrap();
        let start = std::time::Instant::now();
        while index.doc_count(None).unwrap() == 0 {
            assert!(start.elapsed() < Duration::from_secs(10), "never committed");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        drop(tx);
        task.await.unwrap();
    }

    /// Files the worker never got an event for are picked up by the
    /// reconcile pass after the first scan, and again after a lag.
    #[tokio::test]
    async fn reconciles_after_first_scan() {
        let index = Arc::new(ContentIndex::in_memory(FulltextConfig::default()).unwrap());
        let fake = Arc::new(Fake::default());
        {
            let mut f = fake.files.lock().unwrap();
            for n in 0..2500 {
                f.insert(
                    ("s".into(), format!("d/{n:05}.txt")),
                    format!("needle {n}").into_bytes(),
                );
            }
        }
        let (tx, rx) = broadcast::channel::<ContentEvent>(8);
        let task = tokio::spawn(run(Arc::clone(&index), Arc::clone(&fake), rx, Some));
        tx.send(ContentEvent::ScanDone { source: "s".into() })
            .unwrap();
        // A second ScanDone does not reconcile again: nothing is re-read.
        tx.send(ContentEvent::ScanDone { source: "s".into() })
            .unwrap();
        drop(tx);
        task.await.unwrap();
        assert_eq!(index.doc_count(Some("s")).unwrap(), 2500);
        assert_eq!(fake.reads.load(Ordering::SeqCst), 2500);
    }
}
