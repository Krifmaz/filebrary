//! Full-text content search: glue between the server and
//! [`filebrary_fulltext`].
//!
//! * [`open_index`] opens the tantivy index under `<data_dir>/fulltext` at
//!   startup and drops sources whose `content_search` was switched off.
//! * [`StorageFetcher`] lets the indexing worker read files through the
//!   [`Storage`](crate::storage::Storage) trait (path-checked, no shell) and
//!   list known files from the file-name index.
//! * [`start`] runs the worker on the file-name indexer's events
//!   ([`crate::search::IndexEvent`]).
//! * [`candidates`] answers the `content=` parameter of `GET /api/search`;
//!   [`crate::search::query::run`] then intersects the candidates with the
//!   other filters and the caller's scope.
//!
//! Paths in the content index are stored without the leading `/`; paths
//! everywhere else in the server are in `VPath` display form (`/a/b.txt`).

use std::{collections::BTreeMap, collections::HashSet, io, sync::Arc};

use anyhow::Context;
use filebrary_fulltext::{
    ContentIndex,
    worker::{self, ContentEvent, ContentFetcher, ListedFile},
};
use futures::StreamExt;

use crate::{
    auth::{Perms, User},
    config::Config,
    error::{AppError, AppResult},
    search::IndexEvent,
    state::AppState,
    storage::{Source, VPath},
};

/// Most content matches fed into one `GET /api/search` query. Results beyond
/// the best this many are not reachable by paging; narrow the query instead.
pub const MAX_CANDIDATES: usize = 1000;

/// Longest accepted `content=` query, in characters.
pub const MAX_CONTENT_QUERY_CHARS: usize = 512;

/// A file whose text matches a content query.
#[derive(Clone, Debug, PartialEq)]
pub struct ContentMatch {
    /// Source name.
    pub source: String,
    /// Real path inside the source, `VPath` display form (`/a/b.pdf`).
    pub path: String,
    /// Relevance (BM25); higher is better.
    pub score: f32,
    /// HTML-escaped excerpt with matched terms in `<b>…</b>`.
    pub snippet: String,
}

/// The sources that get a content index: `content_search` on, and indexed
/// by name (content results are always intersected with the name index).
fn enabled_sources(config: &Config) -> HashSet<String> {
    if !config.fulltext.enabled || !config.search.enabled {
        return HashSet::new();
    }
    config
        .sources
        .iter()
        .filter(|s| s.content_search && !config.search.exclude_sources.contains(&s.name))
        .map(|s| s.name.clone())
        .collect()
}

/// Opens (or creates) the content index if any source wants it.
///
/// Returns `None` when `[fulltext] enabled = false`, search is disabled, or
/// every source has `content_search = false`; then no index memory is used.
///
/// # Errors
///
/// Fails if the index directory cannot be created or read.
pub fn open_index(config: &Config) -> anyhow::Result<Option<Arc<ContentIndex>>> {
    let enabled = enabled_sources(config);
    if enabled.is_empty() {
        return Ok(None);
    }
    let dir = config.data_dir.join("fulltext");
    let index = ContentIndex::open(&dir, config.fulltext.clone())
        .with_context(|| format!("opening content index at {}", dir.display()))?;
    let dropped = worker::prune_disabled_sources(&index, |s| enabled.contains(s))
        .context("pruning content index")?;
    if !dropped.is_empty() {
        tracing::info!(
            ?dropped,
            "dropped content index of sources with content search off"
        );
    }
    Ok(Some(Arc::new(index)))
}

/// Reads files for the indexing worker through each source's storage
/// backend, and lists files from the file-name index.
pub struct StorageFetcher {
    sources: BTreeMap<String, Source>,
    enabled: HashSet<String>,
    db: sqlx::SqlitePool,
}

impl StorageFetcher {
    /// Builds a fetcher over the state's sources, config and database.
    pub fn new(state: &AppState) -> Self {
        Self {
            sources: state.sources.clone(),
            enabled: enabled_sources(&state.config),
            db: state.db.clone(),
        }
    }
}

impl ContentFetcher for StorageFetcher {
    fn enabled(&self, source: &str) -> bool {
        self.enabled.contains(source)
    }

    fn sources(&self) -> Vec<String> {
        self.enabled.iter().cloned().collect()
    }

    async fn read(&self, source: &str, path: &str, max_bytes: u64) -> io::Result<Vec<u8>> {
        let src = self
            .sources
            .get(source)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown source"))?;
        let vpath = VPath::parse(path).map_err(io::Error::other)?;
        let mut stream = src
            .storage
            .read(&vpath, None)
            .await
            .map_err(io::Error::other)?;
        let mut buf = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if (buf.len() + chunk.len()) as u64 > max_bytes {
                return Err(io::Error::new(
                    io::ErrorKind::FileTooLarge,
                    "file grew past the content size limit",
                ));
            }
            buf.extend_from_slice(&chunk);
        }
        Ok(buf)
    }

    async fn list_files(
        &self,
        source: &str,
        after: &str,
        limit: usize,
    ) -> io::Result<Vec<ListedFile>> {
        let after = if after.is_empty() {
            String::new()
        } else {
            format!("/{after}")
        };
        let rows: Vec<(String, i64, Option<i64>)> = sqlx::query_as(
            "SELECT path, size, mtime FROM file_index
             WHERE source = ? AND path > ? AND is_dir = 0 AND target IS NULL
             ORDER BY path LIMIT ?",
        )
        .bind(source)
        .bind(after)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.db)
        .await
        .map_err(io::Error::other)?;
        Ok(rows
            .into_iter()
            .map(|(path, size, mtime)| ListedFile {
                path: path.trim_start_matches('/').to_owned(),
                size: u64::try_from(size).unwrap_or(0),
                mtime: mtime.unwrap_or(0),
            })
            .collect())
    }
}

/// Converts a file-name index event into a content index event.
pub fn map_event(ev: IndexEvent) -> Option<ContentEvent> {
    fn rel(p: String) -> String {
        p.trim_start_matches('/').to_owned()
    }
    Some(match ev {
        IndexEvent::Upsert {
            source,
            path,
            size,
            mtime,
            is_dir,
        } => ContentEvent::Upsert {
            source,
            path: rel(path),
            size,
            mtime: mtime.unwrap_or(0),
            is_dir,
        },
        IndexEvent::Remove { source, path } => ContentEvent::Remove {
            source,
            path: rel(path),
        },
        IndexEvent::Rename { source, from, to } => ContentEvent::Rename {
            source,
            from: rel(from),
            to: rel(to),
        },
        IndexEvent::ScanDone { source } => ContentEvent::ScanDone { source },
    })
}

/// Starts the content indexing worker. Call before
/// [`crate::search::Search::start`] so no event is missed. Does nothing (and
/// returns `None`) when there is no content index.
pub fn start(state: &AppState) -> Option<tokio::task::JoinHandle<()>> {
    let index = Arc::clone(state.content_index.as_ref()?);
    let fetcher = Arc::new(StorageFetcher::new(state));
    let rx = state.search.subscribe();
    Some(tokio::spawn(worker::run(index, fetcher, rx, map_event)))
}

/// Finds the files whose text matches `query`, among the sources `user`
/// may view, best first, at most [`MAX_CANDIDATES`].
///
/// The result is not yet confined to the user's folder scope; callers must
/// intersect it with the file-name index, which does that.
///
/// # Errors
///
/// `BadRequest` when content search is off or the query is too long,
/// `Internal` on index failures.
pub async fn candidates(
    state: &AppState,
    user: &User,
    query: &str,
) -> AppResult<Vec<ContentMatch>> {
    let index = state
        .content_index
        .clone()
        .ok_or_else(|| AppError::bad_request("content search is not enabled on this server"))?;
    if query.chars().count() > MAX_CONTENT_QUERY_CHARS {
        return Err(AppError::bad_request(format!(
            "content is too long (at most {MAX_CONTENT_QUERY_CHARS} characters)"
        )));
    }
    let sources: Vec<String> = user
        .grants
        .iter()
        .filter(|(_, g)| g.perms.contains(Perms::VIEW))
        .map(|(name, _)| name.clone())
        .collect();
    let q = query.to_owned();
    let hits = tokio::task::spawn_blocking(move || {
        let refs: Vec<&str> = sources.iter().map(String::as_str).collect();
        index.search(&q, &refs, MAX_CANDIDATES, 0)
    })
    .await
    .map_err(|e| AppError::Internal(e.into()))?
    .map_err(|e| AppError::Internal(e.into()))?;
    Ok(hits
        .into_iter()
        .map(|h| ContentMatch {
            source: h.source,
            path: format!("/{}", h.path),
            score: h.score,
            snippet: h.snippet,
        })
        .collect())
}
