//! The SQLite side of the index: the `file_index` table and its FTS5 mirror.
//!
//! All writes for a batch run in one transaction. Paths are stored in
//! [`VPath`] display form (`/a/b.txt`) so that "everything under a folder" is
//! the half-open string range `["/a/", "/a0")`: `0` is the byte after `/`, so
//! the range holds exactly the descendants and uses the `(source, path)` index.

use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::storage::{Entry, VPath};

/// One row to write into the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexRow {
    /// Real path inside the source, in display form (`/a/b.txt`).
    pub path: String,
    /// Final path component.
    pub name: String,
    /// `true` for folders.
    pub is_dir: bool,
    /// Size in bytes (0 for folders).
    pub size: u64,
    /// Modification time as Unix seconds, if known.
    pub mtime: Option<i64>,
    /// For a symlink, the real path of its target inside the source
    /// (display form). Search hides the link from users who cannot see it.
    pub target: Option<String>,
}

impl IndexRow {
    /// Builds a row from a storage [`Entry`].
    pub fn from_entry(e: &Entry) -> Self {
        Self {
            path: e.path.to_string(),
            name: e.name.clone(),
            is_dir: e.is_dir(),
            size: e.size,
            mtime: e.modified.map(|t| t.unix_timestamp()),
            target: None,
        }
    }
}

/// What a batch write changed, so callers can publish events.
#[derive(Debug, Default)]
pub struct BatchOutcome {
    /// Rows that were new or whose size, mtime or kind changed.
    pub changed: Vec<IndexRow>,
}

/// Lower-case extension of a file name, without the dot.
///
/// Names that only start with a dot (`.bashrc`) have no extension.
pub fn extension(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name[i + 1..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// The coarse category used by the `type` filter.
///
/// One of `dir`, `image`, `video`, `audio`, `document`, `text`, `code`,
/// `archive` or `other`.
pub fn kind_of(ext: &str, is_dir: bool) -> &'static str {
    if is_dir {
        return "dir";
    }
    match ext {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "svg" | "ico"
        | "heic" | "heif" | "avif" | "jxl" | "raw" | "dng" | "cr2" | "cr3" | "nef" | "arw"
        | "orf" | "rw2" | "raf" | "srw" | "pef" | "psd" => "image",
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "wmv" | "flv" | "m4v" | "mpg" | "mpeg"
        | "m2ts" | "3gp" | "ogv" => "video",
        "mp3" | "flac" | "wav" | "ogg" | "oga" | "opus" | "m4a" | "aac" | "wma" | "aiff"
        | "alac" | "mka" => "audio",
        "pdf" | "doc" | "docx" | "odt" | "rtf" | "xls" | "xlsx" | "ods" | "ppt" | "pptx"
        | "odp" | "epub" | "pages" | "numbers" | "key" => "document",
        "txt" | "md" | "markdown" | "csv" | "tsv" | "log" | "rst" | "org" | "tex" | "srt"
        | "vtt" | "nfo" => "text",
        "rs" | "py" | "js" | "mjs" | "ts" | "tsx" | "jsx" | "go" | "c" | "h" | "cpp" | "hpp"
        | "cc" | "java" | "kt" | "swift" | "rb" | "php" | "cs" | "sh" | "bash" | "zsh" | "ps1"
        | "lua" | "sql" | "html" | "htm" | "css" | "scss" | "json" | "yaml" | "yml" | "toml"
        | "xml" | "ini" | "conf" | "vue" | "svelte" | "dart" | "zig" | "nix" => "code",
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "iso" | "dmg"
        | "deb" | "rpm" => "archive",
        _ => "other",
    }
}

/// Every value [`kind_of`] can return, for validating the `type` filter.
pub const KINDS: [&str; 9] = [
    "dir", "image", "video", "audio", "document", "text", "code", "archive", "other",
];

/// The `[lo, hi)` string range holding every descendant of `dir`.
///
/// For the root this is every stored path.
pub fn descendant_range(dir: &VPath) -> (String, String) {
    if dir.is_root() {
        ("/".to_owned(), "0".to_owned())
    } else {
        let p = dir.to_string();
        (format!("{p}/"), format!("{p}0"))
    }
}

/// Handle to the index tables.
#[derive(Clone, Debug)]
pub struct IndexStore {
    db: SqlitePool,
}

/// SQLite's default limit on bound parameters is 32766; stay well below.
const MAX_ROWS_PER_STATEMENT: usize = 500;

impl IndexStore {
    /// Wraps a pool whose schema includes migration `0030_search`.
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    /// The underlying pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.db
    }

    /// The highest scan generation stored for `source` (0 if none).
    ///
    /// # Errors
    ///
    /// Database failures.
    pub async fn max_generation(&self, source: &str) -> sqlx::Result<i64> {
        let (g,): (Option<i64>,) =
            sqlx::query_as("SELECT MAX(seen) FROM file_index WHERE source = ?")
                .bind(source)
                .fetch_one(&self.db)
                .await?;
        Ok(g.unwrap_or(0))
    }

    /// Number of indexed entries in `source`.
    ///
    /// # Errors
    ///
    /// Database failures.
    pub async fn count(&self, source: &str) -> sqlx::Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM file_index WHERE source = ?")
            .bind(source)
            .fetch_one(&self.db)
            .await?;
        Ok(n)
    }

    /// Writes a batch of rows seen in generation `seen`.
    ///
    /// Unchanged rows only get their `seen` stamp refreshed, which does not
    /// touch the FTS table. Returns the rows that were new or changed.
    ///
    /// # Errors
    ///
    /// Database failures.
    pub async fn upsert_batch(
        &self,
        source: &str,
        rows: &[IndexRow],
        seen: i64,
    ) -> sqlx::Result<BatchOutcome> {
        let mut out = BatchOutcome::default();
        let mut tx = self.db.begin().await?;
        for chunk in rows.chunks(MAX_ROWS_PER_STATEMENT) {
            // What is already there?
            let paths = serde_json::to_string(&chunk.iter().map(|r| &r.path).collect::<Vec<_>>())
                .unwrap_or_else(|_| "[]".into());
            let existing: Vec<Existing> = sqlx::query_as(
                "SELECT path, name, is_dir, size, mtime, target FROM file_index
                 WHERE source = ? AND path IN (SELECT value FROM json_each(?))",
            )
            .bind(source)
            .bind(&paths)
            .fetch_all(&mut *tx)
            .await?;
            let existing: std::collections::HashMap<String, Existing> =
                existing.into_iter().map(|e| (e.path.clone(), e)).collect();

            let mut unchanged = Vec::new();
            let mut changed = Vec::new();
            for r in chunk {
                match existing.get(&r.path) {
                    Some(e)
                        if e.name == r.name
                            && e.is_dir == r.is_dir
                            && e.size == size_i64(r.size)
                            && e.mtime == r.mtime
                            && e.target == r.target =>
                    {
                        unchanged.push(&r.path);
                    }
                    _ => changed.push(r),
                }
            }

            if !unchanged.is_empty() {
                let list = serde_json::to_string(&unchanged).unwrap_or_else(|_| "[]".into());
                sqlx::query(
                    "UPDATE file_index SET seen = ?
                     WHERE source = ? AND path IN (SELECT value FROM json_each(?))",
                )
                .bind(seen)
                .bind(source)
                .bind(list)
                .execute(&mut *tx)
                .await?;
            }
            if !changed.is_empty() {
                let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
                    "INSERT INTO file_index (source, path, name, ext, kind, is_dir, size, mtime, target, seen) ",
                );
                qb.push_values(changed.iter(), |mut b, r| {
                    let ext = if r.is_dir {
                        String::new()
                    } else {
                        extension(&r.name)
                    };
                    let kind = kind_of(&ext, r.is_dir);
                    b.push_bind(source)
                        .push_bind(&r.path)
                        .push_bind(&r.name)
                        .push_bind(ext)
                        .push_bind(kind)
                        .push_bind(r.is_dir)
                        .push_bind(size_i64(r.size))
                        .push_bind(r.mtime)
                        .push_bind(&r.target)
                        .push_bind(seen);
                });
                qb.push(
                    " ON CONFLICT (source, path) DO UPDATE SET
                        name = excluded.name, ext = excluded.ext, kind = excluded.kind,
                        is_dir = excluded.is_dir, size = excluded.size,
                        mtime = excluded.mtime, target = excluded.target,
                        seen = excluded.seen",
                );
                qb.build().execute(&mut *tx).await?;
                out.changed.extend(changed.into_iter().cloned());
            }
        }
        tx.commit().await?;
        Ok(out)
    }

    /// Removes `path` and everything under it. Returns the number of rows removed.
    ///
    /// # Errors
    ///
    /// Database failures.
    pub async fn remove(&self, source: &str, path: &VPath) -> sqlx::Result<u64> {
        let (lo, hi) = descendant_range(path);
        let r = if path.is_root() {
            sqlx::query("DELETE FROM file_index WHERE source = ?")
                .bind(source)
                .execute(&self.db)
                .await?
        } else {
            sqlx::query(
                "DELETE FROM file_index WHERE source = ? AND (path = ? OR (path >= ? AND path < ?))",
            )
            .bind(source)
            .bind(path.to_string())
            .bind(lo)
            .bind(hi)
            .execute(&self.db)
            .await?
        };
        Ok(r.rows_affected())
    }

    /// Moves `from` (and everything under it) to `to`, keeping row ids.
    ///
    /// Anything already indexed at `to` is replaced. Returns `false` if
    /// nothing was indexed at `from`.
    ///
    /// # Errors
    ///
    /// Database failures.
    pub async fn rename(&self, source: &str, from: &VPath, to: &VPath) -> sqlx::Result<bool> {
        if from.is_root() || to.is_root() || from == to || to.starts_with(from) {
            return Ok(false);
        }
        let (from_s, to_s) = (from.to_string(), to.to_string());
        let (lo, hi) = descendant_range(from);
        let (to_lo, to_hi) = descendant_range(to);
        let new_name = to.name().unwrap_or_default().to_owned();
        let mut tx = self.db.begin().await?;
        let exists: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM file_index WHERE source = ? AND path = ?")
                .bind(source)
                .bind(&from_s)
                .fetch_optional(&mut *tx)
                .await?;
        if exists.is_none() {
            return Ok(false);
        }
        sqlx::query(
            "DELETE FROM file_index WHERE source = ? AND (path = ? OR (path >= ? AND path < ?))",
        )
        .bind(source)
        .bind(&to_s)
        .bind(&to_lo)
        .bind(&to_hi)
        .execute(&mut *tx)
        .await?;
        let ext = extension(&new_name);
        sqlx::query(
            "UPDATE file_index
             SET path = ?, name = ?,
                 ext  = CASE WHEN is_dir THEN '' ELSE ? END,
                 kind = CASE WHEN is_dir THEN 'dir' ELSE ? END
             WHERE source = ? AND path = ?",
        )
        .bind(&to_s)
        .bind(&new_name)
        .bind(&ext)
        .bind(kind_of(&ext, false))
        .bind(source)
        .bind(&from_s)
        .execute(&mut *tx)
        .await?;
        // Descendants keep their names; only the prefix changes.
        sqlx::query(
            "UPDATE file_index SET path = ? || substr(path, ?)
             WHERE source = ? AND path >= ? AND path < ?",
        )
        .bind(&to_s)
        .bind(i64::try_from(from_s.len() + 1).unwrap_or(i64::MAX))
        .bind(source)
        .bind(&lo)
        .bind(&hi)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Stamps every row under `dir` with `generation` so a sweep keeps them.
    ///
    /// # Errors
    ///
    /// Database failures.
    pub async fn touch(&self, source: &str, dir: &VPath, generation: i64) -> sqlx::Result<()> {
        let (lo, hi) = descendant_range(dir);
        sqlx::query(
            "UPDATE file_index SET seen = ? WHERE source = ? AND path >= ? AND path < ? AND seen < ?",
        )
        .bind(generation)
        .bind(source)
        .bind(lo)
        .bind(hi)
        .bind(generation)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Deletes rows of `source` under `under` that the scan of generation
    /// `generation` did not see. Returns the removed paths.
    ///
    /// # Errors
    ///
    /// Database failures.
    pub async fn sweep(
        &self,
        source: &str,
        under: &VPath,
        generation: i64,
    ) -> sqlx::Result<Vec<String>> {
        let (lo, hi) = descendant_range(under);
        let gone: Vec<(String,)> = sqlx::query_as(
            "DELETE FROM file_index
             WHERE source = ? AND path >= ? AND path < ? AND seen < ?
             RETURNING path",
        )
        .bind(source)
        .bind(lo)
        .bind(hi)
        .bind(generation)
        .fetch_all(&self.db)
        .await?;
        Ok(gone.into_iter().map(|(p,)| p).collect())
    }

    /// Drops every row of sources not in `keep` (sources removed from the config).
    ///
    /// # Errors
    ///
    /// Database failures.
    pub async fn retain_sources(&self, keep: &[&str]) -> sqlx::Result<u64> {
        let list = serde_json::to_string(keep).unwrap_or_else(|_| "[]".into());
        let r = sqlx::query(
            "DELETE FROM file_index WHERE source NOT IN (SELECT value FROM json_each(?))",
        )
        .bind(list)
        .execute(&self.db)
        .await?;
        Ok(r.rows_affected())
    }
}

/// The stored state of a row, to detect changes.
#[derive(sqlx::FromRow)]
struct Existing {
    path: String,
    name: String,
    is_dir: bool,
    size: i64,
    mtime: Option<i64>,
    target: Option<String>,
}

fn size_i64(size: u64) -> i64 {
    i64::try_from(size).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, is_dir: bool, size: u64) -> IndexRow {
        let vp = VPath::parse(path).unwrap();
        IndexRow {
            path: vp.to_string(),
            name: vp.name().unwrap().to_owned(),
            is_dir,
            size,
            mtime: Some(1_700_000_000),
            target: None,
        }
    }

    async fn paths(store: &IndexStore) -> Vec<String> {
        sqlx::query_as::<_, (String,)>("SELECT path FROM file_index ORDER BY path")
            .fetch_all(store.pool())
            .await
            .unwrap()
            .into_iter()
            .map(|(p,)| p)
            .collect()
    }

    #[test]
    fn extensions_and_kinds() {
        assert_eq!(extension("a.TXT"), "txt");
        assert_eq!(extension(".bashrc"), "");
        assert_eq!(extension("noext"), "");
        assert_eq!(extension("trailing."), "");
        assert_eq!(extension("x.tar.gz"), "gz");
        assert_eq!(kind_of("jpg", false), "image");
        assert_eq!(kind_of("heic", false), "image");
        assert_eq!(kind_of("mkv", false), "video");
        assert_eq!(kind_of("flac", false), "audio");
        assert_eq!(kind_of("docx", false), "document");
        assert_eq!(kind_of("md", false), "text");
        assert_eq!(kind_of("rs", false), "code");
        assert_eq!(kind_of("7z", false), "archive");
        assert_eq!(kind_of("xyz", false), "other");
        assert_eq!(kind_of("jpg", true), "dir");
    }

    #[test]
    fn ranges_cover_descendants_only() {
        let (lo, hi) = descendant_range(&VPath::parse("a/b").unwrap());
        let inside = |p: &str| p >= lo.as_str() && p < hi.as_str();
        assert!(inside("/a/b/c"));
        assert!(inside("/a/b/c/d.txt"));
        assert!(!inside("/a/b"));
        assert!(!inside("/a/b.txt"));
        assert!(!inside("/a/b!"));
        assert!(!inside("/a/bc"));
        assert!(!inside("/a/b0"));
    }

    #[tokio::test]
    async fn upsert_reports_changes_and_sweeps() {
        let store = IndexStore::new(crate::db::open_memory().await.unwrap());
        let rows = vec![row("a", true, 0), row("a/x.txt", false, 3)];
        let out = store.upsert_batch("s", &rows, 1).await.unwrap();
        assert_eq!(out.changed.len(), 2);

        // Same rows again: nothing changed, but the generation moves on.
        let out = store.upsert_batch("s", &rows, 2).await.unwrap();
        assert!(out.changed.is_empty());
        assert_eq!(store.max_generation("s").await.unwrap(), 2);

        // A size change is reported.
        let out = store
            .upsert_batch("s", &[row("a/x.txt", false, 4)], 3)
            .await
            .unwrap();
        assert_eq!(out.changed.len(), 1);

        // Generation 3 only saw x.txt, so the sweep drops /a.
        let gone = store.sweep("s", &VPath::root(), 3).await.unwrap();
        assert_eq!(gone, ["/a"]);
        assert_eq!(paths(&store).await, ["/a/x.txt"]);
    }

    #[tokio::test]
    async fn rename_moves_subtree_and_fts_follows() {
        let store = IndexStore::new(crate::db::open_memory().await.unwrap());
        let rows = vec![
            row("docs", true, 0),
            row("docs/report.pdf", false, 10),
            row("docs/sub", true, 0),
            row("docs/sub/notes.md", false, 5),
            row("docs.txt", false, 1),
            row("other", true, 0),
            row("other/old", false, 1),
        ];
        store.upsert_batch("s", &rows, 1).await.unwrap();
        let moved = store
            .rename(
                "s",
                &VPath::parse("docs").unwrap(),
                &VPath::parse("other/papers").unwrap(),
            )
            .await
            .unwrap();
        assert!(moved);
        assert_eq!(
            paths(&store).await,
            [
                "/docs.txt",
                "/other",
                "/other/old",
                "/other/papers",
                "/other/papers/report.pdf",
                "/other/papers/sub",
                "/other/papers/sub/notes.md",
            ]
        );
        let hits: Vec<(String,)> = sqlx::query_as(
            "SELECT f.path FROM file_index f JOIN file_index_fts t ON t.rowid = f.id
             WHERE file_index_fts MATCH '\"paper\"'",
        )
        .fetch_all(store.pool())
        .await
        .unwrap();
        assert_eq!(hits, [("/other/papers".to_owned(),)]);

        // Removing a folder removes its subtree only.
        let n = store
            .remove("s", &VPath::parse("other/papers").unwrap())
            .await
            .unwrap();
        assert_eq!(n, 4);
        assert_eq!(paths(&store).await, ["/docs.txt", "/other", "/other/old"]);
    }

    #[tokio::test]
    async fn rename_of_unknown_path_is_a_no_op() {
        let store = IndexStore::new(crate::db::open_memory().await.unwrap());
        let a = VPath::parse("a").unwrap();
        let b = VPath::parse("b").unwrap();
        assert!(!store.rename("s", &a, &b).await.unwrap());
        assert!(!store.rename("s", &a, &a.join("c").unwrap()).await.unwrap());
    }
}
