//! The tantivy index that holds the extracted text of every indexed file.
//!
//! One document per file, keyed by `(source, path)`. Besides the text, each
//! document stores the file's size and modification time (so unchanged files
//! are skipped on rescans) and one `parents` term per ancestor directory (so
//! a removed or renamed directory can be dropped with a single term delete).
//!
//! All methods are blocking (tantivy does disk I/O and CPU work); call them
//! from [`tokio::task::spawn_blocking`] when on an async runtime.
//!
//! # Memory
//!
//! The [`IndexWriter`] owns a fixed in-memory arena
//! ([`FulltextConfig::writer_memory_mib`]). It is created lazily on the first
//! write and dropped again by [`ContentIndex::commit`], so an idle server only
//! pays for the reader (memory-mapped segments, which the OS can evict).

use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tantivy::collector::TopDocs;
use tantivy::directory::MmapDirectory;
use tantivy::query::{BooleanQuery, Occur, Query, QueryParser, TermQuery};
use tantivy::schema::{
    Field, INDEXED, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions,
    Value,
};
use tantivy::snippet::SnippetGenerator;
use tantivy::tokenizer::{
    AsciiFoldingFilter, LowerCaser, RemoveLongFilter, SimpleTokenizer, TextAnalyzer,
};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term, doc};
use thiserror::Error;

/// Name under which the body analyzer is registered with tantivy.
const BODY_TOKENIZER: &str = "filebrary_body";

/// Bumped whenever the schema changes; a mismatching on-disk index is
/// discarded and rebuilt from the next scan.
const SCHEMA_VERSION: &str = "1";

/// Settings for the content index (the `[fulltext]` config section).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct FulltextConfig {
    /// Master switch. When off no content index is opened at all, whatever
    /// the per-source `content_search` settings say.
    pub enabled: bool,
    /// Files larger than this (in MiB) are not read for content.
    pub max_file_mib: u64,
    /// At most this many characters of text are indexed per file.
    pub max_chars: usize,
    /// Memory arena of the index writer, in MiB (tantivy's minimum is 15).
    /// This is the main contributor to peak memory while indexing.
    pub writer_memory_mib: usize,
    /// Commit (make new documents searchable) after this many changes, or
    /// [`Self::commit_interval_secs`] after the first uncommitted one.
    pub commit_every_docs: usize,
    /// See [`Self::commit_every_docs`].
    pub commit_interval_secs: u64,
}

impl Default for FulltextConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_file_mib: 32,
            max_chars: 1_000_000,
            writer_memory_mib: 24,
            commit_every_docs: 500,
            commit_interval_secs: 5,
        }
    }
}

impl FulltextConfig {
    /// [`Self::max_file_mib`] in bytes.
    pub fn max_file_bytes(&self) -> u64 {
        self.max_file_mib.saturating_mul(1024 * 1024)
    }
}

/// Errors from the content index.
#[derive(Debug, Error)]
pub enum IndexError {
    /// An error from tantivy.
    #[error("content index: {0}")]
    Tantivy(#[from] tantivy::TantivyError),
    /// The index directory could not be opened.
    #[error("content index directory: {0}")]
    Directory(#[from] tantivy::directory::error::OpenDirectoryError),
    /// Filesystem error while preparing the index directory.
    #[error("content index directory: {0}")]
    Io(#[from] std::io::Error),
}

/// Result alias for index operations.
pub type IndexResult<T> = Result<T, IndexError>;

/// One search result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Hit {
    /// Source name.
    pub source: String,
    /// Source-relative path, forward slashes, no leading slash.
    pub path: String,
    /// Relevance score (BM25); higher is better.
    pub score: f32,
    /// A short HTML fragment around the best match. Text is HTML-escaped and
    /// matched terms are wrapped in `<b>…</b>`, so it is safe to render.
    pub snippet: String,
}

/// The fields of the schema.
#[derive(Clone, Copy)]
struct Fields {
    id: Field,
    source: Field,
    path: Field,
    parents: Field,
    size: Field,
    mtime: Field,
    body: Field,
}

/// The full-text content index. Cheap to share behind an `Arc`.
pub struct ContentIndex {
    index: Index,
    reader: IndexReader,
    writer: Mutex<Option<IndexWriter>>,
    fields: Fields,
    config: FulltextConfig,
}

impl std::fmt::Debug for ContentIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContentIndex").finish_non_exhaustive()
    }
}

/// Builds the key term value for a document.
fn key(source: &str, path: &str) -> String {
    format!("{source}\0{path}")
}

fn build_schema() -> (Schema, Fields) {
    let mut b = Schema::builder();
    let body_opts = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(BODY_TOKENIZER)
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        )
        .set_stored();
    let fields = Fields {
        id: b.add_text_field("id", STRING | STORED),
        source: b.add_text_field("source", STRING | STORED),
        path: b.add_text_field("path", STRING | STORED),
        parents: b.add_text_field("parents", STRING),
        size: b.add_u64_field("size", STORED),
        mtime: b.add_i64_field("mtime", STORED | INDEXED),
        body: b.add_text_field("body", body_opts),
    };
    (b.build(), fields)
}

fn body_analyzer() -> TextAnalyzer {
    TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(RemoveLongFilter::limit(48))
        .filter(LowerCaser)
        .filter(AsciiFoldingFilter)
        .build()
}

impl ContentIndex {
    /// Opens the index stored in `dir`, creating it if needed.
    ///
    /// If `dir` holds an index from an older schema version it is wiped and
    /// recreated empty; the next scan fills it again.
    ///
    /// # Errors
    ///
    /// Fails if the directory cannot be created or the index is unreadable.
    pub fn open(dir: &Path, config: FulltextConfig) -> IndexResult<Self> {
        let marker = dir.join("filebrary-schema-version");
        if dir.exists() {
            let current = std::fs::read_to_string(&marker).unwrap_or_default();
            if current.trim() != SCHEMA_VERSION {
                tracing::warn!(dir = %dir.display(), "content index schema changed; rebuilding");
                std::fs::remove_dir_all(dir)?;
            }
        }
        std::fs::create_dir_all(dir)?;
        let (schema, fields) = build_schema();
        let index = Index::open_or_create(MmapDirectory::open(dir)?, schema)?;
        std::fs::write(&marker, SCHEMA_VERSION)?;
        Self::from_index(index, fields, config)
    }

    /// Creates a throw-away index held entirely in memory (for tests).
    ///
    /// # Errors
    ///
    /// Fails only if tantivy cannot build the reader.
    pub fn in_memory(config: FulltextConfig) -> IndexResult<Self> {
        let (schema, fields) = build_schema();
        Self::from_index(Index::create_in_ram(schema), fields, config)
    }

    fn from_index(index: Index, fields: Fields, config: FulltextConfig) -> IndexResult<Self> {
        index.tokenizers().register(BODY_TOKENIZER, body_analyzer());
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        Ok(Self {
            index,
            reader,
            writer: Mutex::new(None),
            fields,
            config,
        })
    }

    /// The settings this index was opened with.
    pub fn config(&self) -> &FulltextConfig {
        &self.config
    }

    /// Runs `f` with the (lazily created) writer.
    fn with_writer<T>(&self, f: impl FnOnce(&mut IndexWriter) -> IndexResult<T>) -> IndexResult<T> {
        let mut guard = self.writer.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            let budget = self.config.writer_memory_mib.max(15) * 1024 * 1024;
            *guard = Some(self.index.writer_with_num_threads(1, budget)?);
        }
        f(guard.as_mut().expect("writer was just created"))
    }

    /// Looks up the stored `(size, mtime)` of a file, if it is indexed
    /// (as of the last commit).
    pub fn indexed_version(&self, source: &str, path: &str) -> IndexResult<Option<(u64, i64)>> {
        let searcher = self.reader.searcher();
        let q = TermQuery::new(
            Term::from_field_text(self.fields.id, &key(source, path)),
            IndexRecordOption::Basic,
        );
        let top = searcher.search(&q, &TopDocs::with_limit(1).order_by_score())?;
        let Some((_, addr)) = top.first() else {
            return Ok(None);
        };
        let d: TantivyDocument = searcher.doc(*addr)?;
        let size = d.get_first(self.fields.size).and_then(|v| v.as_u64());
        let mtime = d.get_first(self.fields.mtime).and_then(|v| v.as_i64());
        Ok(size.zip(mtime))
    }

    /// Whether `(source, path)` must be (re)indexed: it is missing, or its
    /// size or modification time changed since it was indexed.
    pub fn needs_update(
        &self,
        source: &str,
        path: &str,
        size: u64,
        mtime: i64,
    ) -> IndexResult<bool> {
        Ok(self.indexed_version(source, path)? != Some((size, mtime)))
    }

    /// Adds or replaces the text of one file. Visible after [`Self::commit`].
    pub fn upsert(
        &self,
        source: &str,
        path: &str,
        size: u64,
        mtime: i64,
        text: &str,
    ) -> IndexResult<()> {
        let f = self.fields;
        let id = key(source, path);
        let mut d = doc!(
            f.id => id.clone(),
            f.source => source,
            f.path => path,
            f.size => size,
            f.mtime => mtime,
            f.body => text,
        );
        let mut end = 0;
        while let Some(i) = path[end..].find('/') {
            end += i;
            d.add_text(f.parents, key(source, &path[..end]));
            end += 1;
        }
        d.add_text(f.parents, source);
        self.with_writer(|w| {
            w.delete_term(Term::from_field_text(f.id, &id));
            w.add_document(d)?;
            Ok(())
        })
    }

    /// Removes a file, or a directory and everything below it.
    pub fn remove(&self, source: &str, path: &str) -> IndexResult<()> {
        let f = self.fields;
        let k = key(source, path);
        self.with_writer(|w| {
            w.delete_term(Term::from_field_text(f.id, &k));
            w.delete_term(Term::from_field_text(f.parents, &k));
            Ok(())
        })
    }

    /// Removes every document of a source (used when content search is
    /// switched off for it, or the source is removed from the config).
    pub fn remove_source(&self, source: &str) -> IndexResult<()> {
        let f = self.fields;
        self.with_writer(|w| {
            w.delete_term(Term::from_field_text(f.parents, source));
            Ok(())
        })
    }

    /// Moves a file or a directory tree from `from` to `to` within a source,
    /// keeping the indexed text (no re-extraction).
    ///
    /// Works on committed documents: changes still pending in the writer are
    /// committed first.
    pub fn rename(&self, source: &str, from: &str, to: &str) -> IndexResult<()> {
        self.commit()?;
        let f = self.fields;
        let searcher = self.reader.searcher();
        let from_key = key(source, from);
        let q = BooleanQuery::new(vec![
            (
                Occur::Should,
                Box::new(TermQuery::new(
                    Term::from_field_text(f.id, &from_key),
                    IndexRecordOption::Basic,
                )) as Box<dyn Query>,
            ),
            (
                Occur::Should,
                Box::new(TermQuery::new(
                    Term::from_field_text(f.parents, &from_key),
                    IndexRecordOption::Basic,
                )),
            ),
        ]);
        let n = searcher.num_docs() as usize;
        if n == 0 {
            return Ok(());
        }
        let found = searcher.search(&q, &TopDocs::with_limit(n).order_by_score())?;
        let mut moved = Vec::with_capacity(found.len());
        for (_, addr) in found {
            let d: TantivyDocument = searcher.doc(addr)?;
            let old_path = d
                .get_first(f.path)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned();
            let Some(rest) = old_path.strip_prefix(from) else {
                continue;
            };
            let new_path = format!("{to}{rest}");
            let size = d.get_first(f.size).and_then(|v| v.as_u64()).unwrap_or(0);
            let mtime = d.get_first(f.mtime).and_then(|v| v.as_i64()).unwrap_or(0);
            let body = d
                .get_first(f.body)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned();
            moved.push((old_path, new_path, size, mtime, body));
        }
        for (old, new, size, mtime, body) in moved {
            self.with_writer(|w| {
                w.delete_term(Term::from_field_text(f.id, &key(source, &old)));
                Ok(())
            })?;
            self.upsert(source, &new, size, mtime, &body)?;
        }
        self.commit()
    }

    /// Makes all pending changes searchable and releases the writer's memory.
    ///
    /// Blocks until background segment merges finish.
    pub fn commit(&self) -> IndexResult<()> {
        let writer = {
            let mut guard = self.writer.lock().unwrap_or_else(|p| p.into_inner());
            guard.take()
        };
        if let Some(mut w) = writer {
            w.commit()?;
            w.wait_merging_threads()?;
            self.reader.reload()?;
        }
        Ok(())
    }

    /// Whether there are changes not yet committed.
    pub fn has_pending(&self) -> bool {
        self.writer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    /// Number of indexed files, in total or for one source.
    pub fn doc_count(&self, source: Option<&str>) -> IndexResult<u64> {
        let searcher = self.reader.searcher();
        Ok(match source {
            None => searcher.num_docs(),
            Some(s) => searcher.search(
                &TermQuery::new(
                    Term::from_field_text(self.fields.parents, s),
                    IndexRecordOption::Basic,
                ),
                &tantivy::collector::Count,
            )? as u64,
        })
    }

    /// The names of all sources that have at least one indexed document.
    ///
    /// Used at startup to drop sources whose content search was switched
    /// off or which were removed from the config.
    pub fn indexed_sources(&self) -> IndexResult<Vec<String>> {
        let searcher = self.reader.searcher();
        let mut out = std::collections::BTreeSet::new();
        for seg in searcher.segment_readers() {
            let inv = seg.inverted_index(self.fields.source)?;
            let mut terms = inv.terms().stream()?;
            while terms.advance() {
                if let Ok(s) = std::str::from_utf8(terms.key()) {
                    out.insert(s.to_owned());
                }
            }
        }
        Ok(out.into_iter().collect())
    }

    /// Searches the text of files in `sources`.
    ///
    /// `query` uses tantivy's query syntax leniently: plain words are
    /// AND-ed, `"quoted phrases"` match in order, `-word` excludes, and
    /// syntax errors are ignored rather than reported. Results are sorted by
    /// relevance. An empty `sources` list or a blank query returns nothing.
    pub fn search(
        &self,
        query: &str,
        sources: &[&str],
        limit: usize,
        offset: usize,
    ) -> IndexResult<Vec<Hit>> {
        if sources.is_empty() || query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let f = self.fields;
        let mut parser = QueryParser::for_index(&self.index, vec![f.body]);
        parser.set_conjunction_by_default();
        let (user_query, _errors) = parser.parse_query_lenient(query);
        let source_filter = BooleanQuery::new(
            sources
                .iter()
                .map(|s| {
                    (
                        Occur::Should,
                        Box::new(TermQuery::new(
                            Term::from_field_text(f.source, s),
                            IndexRecordOption::Basic,
                        )) as Box<dyn Query>,
                    )
                })
                .collect(),
        );
        let full = BooleanQuery::new(vec![
            (Occur::Must, user_query.box_clone()),
            (Occur::Must, Box::new(source_filter)),
        ]);
        let searcher = self.reader.searcher();
        let top = searcher.search(
            &full,
            &TopDocs::with_limit(limit)
                .and_offset(offset)
                .order_by_score(),
        )?;
        let mut snippets = SnippetGenerator::create(&searcher, &*user_query, f.body)?;
        snippets.set_max_num_chars(240);
        let mut hits = Vec::with_capacity(top.len());
        for (score, addr) in top {
            let d: TantivyDocument = searcher.doc(addr)?;
            let text = |field| {
                d.get_first(field)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_owned()
            };
            hits.push(Hit {
                source: text(f.source),
                path: text(f.path),
                score,
                snippet: snippets.snippet_from_doc(&d).to_html(),
            });
        }
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx() -> ContentIndex {
        ContentIndex::in_memory(FulltextConfig::default()).unwrap()
    }

    fn paths(hits: &[Hit]) -> Vec<String> {
        let mut v: Vec<_> = hits
            .iter()
            .map(|h| format!("{}:{}", h.source, h.path))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn upsert_search_and_snippet() {
        let i = idx();
        i.upsert(
            "files",
            "a/notes.txt",
            10,
            1,
            "The quick brown fox jumps over the lazy dog",
        )
        .unwrap();
        i.upsert("files", "b.txt", 10, 1, "Nothing to see <script> here")
            .unwrap();
        i.commit().unwrap();
        let hits = i.search("FOX", &["files"], 10, 0).unwrap();
        assert_eq!(paths(&hits), ["files:a/notes.txt"]);
        assert!(
            hits[0].snippet.contains("<b>fox</b>"),
            "{}",
            hits[0].snippet
        );
        // Snippets are escaped.
        let hits = i.search("script", &["files"], 10, 0).unwrap();
        assert!(hits[0].snippet.contains("&lt;"), "{}", hits[0].snippet);
    }

    #[test]
    fn accents_and_phrases() {
        let i = idx();
        i.upsert("s", "x.txt", 1, 1, "Café crème brûlée").unwrap();
        i.upsert("s", "y.txt", 1, 1, "brûlée crème").unwrap();
        i.commit().unwrap();
        assert_eq!(
            paths(&i.search("creme brulee", &["s"], 10, 0).unwrap()).len(),
            2
        );
        assert_eq!(
            paths(&i.search("\"creme brulee\"", &["s"], 10, 0).unwrap()),
            ["s:x.txt"]
        );
        // Lenient parsing: unbalanced syntax does not error.
        assert!(i.search("\"unclosed (", &["s"], 10, 0).is_ok());
    }

    #[test]
    fn source_filter() {
        let i = idx();
        i.upsert("a", "x.txt", 1, 1, "shared word").unwrap();
        i.upsert("b", "x.txt", 1, 1, "shared word").unwrap();
        i.commit().unwrap();
        assert_eq!(
            paths(&i.search("shared", &["a"], 10, 0).unwrap()),
            ["a:x.txt"]
        );
        assert_eq!(i.search("shared", &["a", "b"], 10, 0).unwrap().len(), 2);
        assert!(i.search("shared", &[], 10, 0).unwrap().is_empty());
    }

    #[test]
    fn replace_and_version() {
        let i = idx();
        i.upsert("s", "f.txt", 5, 100, "old text").unwrap();
        i.commit().unwrap();
        assert!(!i.needs_update("s", "f.txt", 5, 100).unwrap());
        assert!(i.needs_update("s", "f.txt", 6, 100).unwrap());
        assert!(i.needs_update("s", "other.txt", 5, 100).unwrap());
        i.upsert("s", "f.txt", 6, 200, "new text").unwrap();
        i.commit().unwrap();
        assert_eq!(i.doc_count(None).unwrap(), 1);
        assert!(i.search("old", &["s"], 10, 0).unwrap().is_empty());
        assert_eq!(i.indexed_version("s", "f.txt").unwrap(), Some((6, 200)));
    }

    #[test]
    fn remove_file_dir_and_source() {
        let i = idx();
        for p in ["d/a.txt", "d/sub/b.txt", "dd/c.txt", "e.txt"] {
            i.upsert("s", p, 1, 1, "word").unwrap();
        }
        i.upsert("t", "d/a.txt", 1, 1, "word").unwrap();
        i.commit().unwrap();
        i.remove("s", "d").unwrap();
        i.commit().unwrap();
        // "dd" is not inside "d"; source t is untouched.
        assert_eq!(
            paths(&i.search("word", &["s", "t"], 10, 0).unwrap()),
            ["s:dd/c.txt", "s:e.txt", "t:d/a.txt"]
        );
        assert_eq!(i.indexed_sources().unwrap(), ["s", "t"]);
        i.remove("s", "e.txt").unwrap();
        i.remove_source("t").unwrap();
        i.commit().unwrap();
        assert_eq!(
            paths(&i.search("word", &["s", "t"], 10, 0).unwrap()),
            ["s:dd/c.txt"]
        );
        assert_eq!(i.doc_count(Some("s")).unwrap(), 1);
    }

    #[test]
    fn rename_file_and_dir() {
        let i = idx();
        i.upsert("s", "docs/a.txt", 3, 7, "alpha").unwrap();
        i.upsert("s", "docs/deep/b.txt", 3, 7, "beta").unwrap();
        i.upsert("s", "docsx/c.txt", 3, 7, "gamma").unwrap();
        i.commit().unwrap();
        i.rename("s", "docs", "archive/2026").unwrap();
        assert_eq!(
            paths(&i.search("alpha OR beta OR gamma", &["s"], 10, 0).unwrap()),
            [
                "s:archive/2026/a.txt",
                "s:archive/2026/deep/b.txt",
                "s:docsx/c.txt"
            ]
        );
        assert_eq!(
            i.indexed_version("s", "archive/2026/a.txt").unwrap(),
            Some((3, 7))
        );
        i.rename("s", "docsx/c.txt", "c.txt").unwrap();
        assert_eq!(
            paths(&i.search("gamma", &["s"], 10, 0).unwrap()),
            ["s:c.txt"]
        );
        // Removing the new parent removes the moved children.
        i.remove("s", "archive").unwrap();
        i.commit().unwrap();
        assert_eq!(i.doc_count(None).unwrap(), 1);
    }

    #[test]
    fn paging() {
        let i = idx();
        for n in 0..25 {
            i.upsert("s", &format!("{n}.txt"), 1, 1, "page").unwrap();
        }
        i.commit().unwrap();
        assert_eq!(i.search("page", &["s"], 10, 0).unwrap().len(), 10);
        assert_eq!(i.search("page", &["s"], 10, 20).unwrap().len(), 5);
    }

    #[test]
    fn on_disk_reopen_and_schema_reset() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("fulltext");
        {
            let i = ContentIndex::open(&p, FulltextConfig::default()).unwrap();
            i.upsert("s", "a.txt", 1, 1, "persisted").unwrap();
            i.commit().unwrap();
        }
        let i = ContentIndex::open(&p, FulltextConfig::default()).unwrap();
        assert_eq!(i.search("persisted", &["s"], 10, 0).unwrap().len(), 1);
        drop(i);
        std::fs::write(p.join("filebrary-schema-version"), "0").unwrap();
        let i = ContentIndex::open(&p, FulltextConfig::default()).unwrap();
        assert_eq!(i.doc_count(None).unwrap(), 0);
    }
}
