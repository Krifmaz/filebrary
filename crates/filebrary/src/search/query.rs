//! Turning search filters into one SQL query over `file_index`.
//!
//! Every query is confined to what the caller may see: for each source the
//! user can view, only paths inside their grant's scope (and inside the
//! optional `path` folder) qualify. Symlinks are indexed with their target's
//! real path and only match when that target lies in the same area, so a link
//! never reveals a file outside the caller's scope (links to places outside
//! the source are not indexed at all, and linked folders are not descended). Results carry *user-visible* paths, with
//! the scope stripped.
//!
//! # Name matching
//!
//! `q` is split on whitespace; every term must occur somewhere in the name,
//! case-insensitively. Terms of three or more characters go through the
//! trigram FTS index; shorter ones use `LIKE`, which scans the rows left by
//! the other filters.

use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Sqlite};
use time::{
    Date, OffsetDateTime, format_description::well_known::Rfc3339, macros::format_description,
};

use super::store::{KINDS, descendant_range};
use crate::{
    auth::{Perms, User},
    error::{AppError, AppResult},
    storage::{VPath, guess_mime},
};

/// Default number of results.
pub const DEFAULT_LIMIT: u32 = 100;
/// Most results per request.
pub const MAX_LIMIT: u32 = 1000;
/// Highest accepted `offset` (deep paging should narrow the filters instead).
pub const MAX_OFFSET: u32 = 100_000;
/// Longest accepted `q`, in characters.
pub const MAX_QUERY_CHARS: usize = 256;
/// Most terms in `q`, and most values in a list filter.
pub const MAX_TERMS: usize = 16;

/// Query-string parameters of `GET /api/search`. Every filter is optional and
/// they combine with AND; list filters take comma-separated values (OR).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchParams {
    /// Words that must all appear in the name.
    pub q: Option<String>,
    /// Sources to search (default: every source the caller can view).
    pub source: Option<String>,
    /// Only search under this folder (user-visible path). Needs exactly one `source`.
    pub path: Option<String>,
    /// Types: `dir`, `image`, `video`, `audio`, `document`, `text`, `code`,
    /// `archive`, `other`, or `file` (anything but `dir`).
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// Extensions without the dot, e.g. `jpg,png`.
    pub ext: Option<String>,
    /// Smallest size: bytes, or with a unit (`10k`, `1.5MB`, `2GiB`; powers of 1024).
    pub min_size: Option<String>,
    /// Largest size (inclusive), same format as `min_size`.
    pub max_size: Option<String>,
    /// Modified at or after: `YYYY-MM-DD`, RFC 3339, or Unix seconds.
    pub after: Option<String>,
    /// Modified strictly before, same formats as `after`.
    pub before: Option<String>,
    /// Tags that must all be on the item (names, case-insensitive).
    pub tag: Option<String>,
    /// `name` (default), `path`, `size` or `modified`.
    pub sort: Option<String>,
    /// `asc` (default for name and path) or `desc` (default for size and modified).
    pub order: Option<String>,
    /// Results per page, 1 to [`MAX_LIMIT`].
    pub limit: Option<u32>,
    /// Results to skip.
    pub offset: Option<u32>,
    /// Full-text content query (served by the content index when it is enabled).
    pub content: Option<String>,
}

/// One result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SearchHit {
    /// Source name.
    pub source: String,
    /// Path as the caller sees it (their scope removed).
    pub path: String,
    /// File or folder name.
    pub name: String,
    /// Coarse type (see [`SearchParams::kind`]).
    #[serde(rename = "type")]
    pub kind: String,
    /// `true` for folders.
    pub is_dir: bool,
    /// Size in bytes (0 for folders).
    pub size: u64,
    /// Last modification time.
    #[serde(with = "time::serde::rfc3339::option")]
    pub modified: Option<OffsetDateTime>,
    /// Best-guess MIME type for files.
    pub mime: Option<String>,
    /// For `content=` queries: an HTML-escaped excerpt of the file's text
    /// with matched terms in `<b>…</b>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

/// The response of `GET /api/search`.
#[derive(Clone, Debug, Serialize)]
pub struct SearchResults {
    /// Matches, in the requested order.
    pub hits: Vec<SearchHit>,
    /// More matches exist beyond this page.
    pub has_more: bool,
    /// A scan is running, so results may be incomplete.
    pub indexing: bool,
}

/// Validated filters, ready to run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    /// Terms matched through FTS (3+ characters).
    pub fts_terms: Vec<String>,
    /// Terms matched with LIKE (1-2 characters).
    pub like_terms: Vec<String>,
    /// Sources requested (empty: all visible).
    pub sources: Vec<String>,
    /// Folder to search under (user-visible).
    pub folder: Option<VPath>,
    /// Types (empty: any).
    pub kinds: Vec<String>,
    /// Exclude folders (`type=file`).
    pub files_only: bool,
    /// Extensions (empty: any).
    pub exts: Vec<String>,
    /// Size bounds in bytes, inclusive.
    pub min_size: Option<u64>,
    /// See `min_size`.
    pub max_size: Option<u64>,
    /// Modified at or after (Unix seconds).
    pub after: Option<i64>,
    /// Modified before (Unix seconds).
    pub before: Option<i64>,
    /// Required tags.
    pub tags: Vec<String>,
    /// SQL ORDER BY clause (built from a fixed list, never from input).
    pub order_by: &'static str,
    /// Page size.
    pub limit: u32,
    /// Page start.
    pub offset: u32,
    /// Files matching the `content=` query (see [`crate::content::candidates`]);
    /// when set, only these files can match. `None` means no content filter.
    pub content: Option<Vec<crate::content::ContentMatch>>,
    /// Order by content relevance (a `content=` query without `sort=`).
    pub by_relevance: bool,
}

fn list(v: Option<&str>, what: &str) -> AppResult<Vec<String>> {
    let items: Vec<String> = v
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if items.len() > MAX_TERMS {
        return Err(AppError::bad_request(format!(
            "too many values for {what} (at most {MAX_TERMS})"
        )));
    }
    Ok(items)
}

/// Parses a size such as `512`, `10k`, `1.5MB` or `2GiB` (powers of 1024).
///
/// # Errors
///
/// Fails on malformed input.
pub fn parse_size(s: &str) -> AppResult<u64> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num
        .parse()
        .map_err(|_| AppError::bad_request(format!("invalid size {s:?}")))?;
    let mult: u64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1 << 10,
        "m" | "mb" | "mib" => 1 << 20,
        "g" | "gb" | "gib" => 1 << 30,
        "t" | "tb" | "tib" => 1 << 40,
        _ => return Err(AppError::bad_request(format!("invalid size unit in {s:?}"))),
    };
    let v = n * mult as f64;
    if !v.is_finite() || v < 0.0 || v > u64::MAX as f64 {
        return Err(AppError::bad_request(format!("invalid size {s:?}")));
    }
    Ok(v as u64)
}

/// Parses a date: `YYYY-MM-DD` (midnight UTC), RFC 3339, or Unix seconds.
///
/// # Errors
///
/// Fails on malformed input.
pub fn parse_time(s: &str) -> AppResult<i64> {
    let s = s.trim();
    if let Ok(n) = s.parse::<i64>() {
        return Ok(n);
    }
    if let Ok(t) = OffsetDateTime::parse(s, &Rfc3339) {
        return Ok(t.unix_timestamp());
    }
    if let Ok(d) = Date::parse(s, format_description!("[year]-[month]-[day]")) {
        return Ok(d.midnight().assume_utc().unix_timestamp());
    }
    Err(AppError::bad_request(format!(
        "invalid date {s:?}: use YYYY-MM-DD, RFC 3339 or Unix seconds"
    )))
}

impl Filters {
    /// Validates the raw parameters.
    ///
    /// # Errors
    ///
    /// `BadRequest` describing the first invalid parameter.
    pub fn from_params(p: &SearchParams) -> AppResult<Self> {
        let mut f = Filters::default();
        if let Some(q) = &p.q {
            if q.chars().count() > MAX_QUERY_CHARS {
                return Err(AppError::bad_request(format!(
                    "q is too long (at most {MAX_QUERY_CHARS} characters)"
                )));
            }
            let terms: Vec<&str> = q.split_whitespace().collect();
            if terms.len() > MAX_TERMS {
                return Err(AppError::bad_request(format!(
                    "too many words in q (at most {MAX_TERMS})"
                )));
            }
            for t in terms {
                if t.chars().count() >= 3 {
                    f.fts_terms.push(t.to_owned());
                } else {
                    f.like_terms.push(t.to_owned());
                }
            }
        }
        f.sources = list(p.source.as_deref(), "source")?;
        if let Some(path) = p.path.as_deref().filter(|s| !s.is_empty()) {
            if f.sources.len() != 1 {
                return Err(AppError::bad_request(
                    "path needs exactly one source to search in",
                ));
            }
            f.folder = Some(VPath::parse(path)?);
        }
        for k in list(p.kind.as_deref(), "type")? {
            let k = k.to_ascii_lowercase();
            if k == "file" {
                f.files_only = true;
            } else if KINDS.contains(&k.as_str()) {
                f.kinds.push(k);
            } else {
                return Err(AppError::bad_request(format!(
                    "unknown type {k:?}: use file or one of {}",
                    KINDS.join(", ")
                )));
            }
        }
        f.exts = list(p.ext.as_deref(), "ext")?
            .into_iter()
            .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
            .collect();
        f.min_size = p.min_size.as_deref().map(parse_size).transpose()?;
        f.max_size = p.max_size.as_deref().map(parse_size).transpose()?;
        f.after = p.after.as_deref().map(parse_time).transpose()?;
        f.before = p.before.as_deref().map(parse_time).transpose()?;
        f.tags = list(p.tag.as_deref(), "tag")?;
        let sort = p.sort.as_deref().unwrap_or("name");
        let desc = match p.order.as_deref() {
            None => matches!(sort, "size" | "modified"),
            Some("asc") => false,
            Some("desc") => true,
            Some(o) => {
                return Err(AppError::bad_request(format!(
                    "invalid order {o:?}: use asc or desc"
                )));
            }
        };
        f.order_by = match (sort, desc) {
            ("name", false) => "f.name COLLATE NOCASE ASC, f.path ASC",
            ("name", true) => "f.name COLLATE NOCASE DESC, f.path DESC",
            ("path", false) => "f.source ASC, f.path ASC",
            ("path", true) => "f.source DESC, f.path DESC",
            ("size", false) => "f.size ASC, f.path ASC",
            ("size", true) => "f.size DESC, f.path ASC",
            ("modified", false) => "f.mtime ASC, f.path ASC",
            ("modified", true) => "f.mtime DESC, f.path ASC",
            (s, _) => {
                return Err(AppError::bad_request(format!(
                    "invalid sort {s:?}: use name, path, size or modified"
                )));
            }
        };
        f.limit = p.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&f.limit) {
            return Err(AppError::bad_request(format!(
                "limit must be between 1 and {MAX_LIMIT}"
            )));
        }
        f.offset = p.offset.unwrap_or(0);
        if f.offset > MAX_OFFSET {
            return Err(AppError::bad_request(format!(
                "offset must be at most {MAX_OFFSET}"
            )));
        }
        Ok(f)
    }
}

/// Builds an FTS5 query that matches every term as a literal substring.
fn fts_query(terms: &[String]) -> String {
    terms
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// Escapes `%`, `_` and `\` for `LIKE ... ESCAPE '\'`.
fn like_pattern(term: &str) -> String {
    let mut s = String::with_capacity(term.len() + 2);
    s.push('%');
    for c in term.chars() {
        if matches!(c, '%' | '_' | '\\') {
            s.push('\\');
        }
        s.push(c);
    }
    s.push('%');
    s
}

/// A row of the result query.
#[derive(sqlx::FromRow)]
struct HitRow {
    source: String,
    path: String,
    name: String,
    kind: String,
    is_dir: bool,
    size: i64,
    mtime: Option<i64>,
}

/// Where one source may be searched: the real folder to search under.
#[derive(Clone, Debug)]
struct Area {
    source: String,
    scope: VPath,
    under: VPath,
}

/// Runs a search for `user`. `indexed` says whether a source is indexed.
///
/// # Errors
///
/// `BadRequest` for filters that cannot apply, `NotFound` for a source or
/// folder the user cannot see, and database failures.
pub async fn run(
    db: &sqlx::SqlitePool,
    user: &User,
    f: &Filters,
    indexed: impl Fn(&str) -> bool,
) -> AppResult<(Vec<SearchHit>, bool)> {
    // Which (source, folder) pairs the caller may search.
    let mut areas = Vec::new();
    if f.sources.is_empty() {
        for (name, g) in &user.grants {
            if g.perms.contains(Perms::VIEW) && indexed(name) {
                areas.push(Area {
                    source: name.clone(),
                    scope: g.scope.clone(),
                    under: g.scope.clone(),
                });
            }
        }
    } else {
        for name in &f.sources {
            let folder = f.folder.clone().unwrap_or_default();
            let under = user.require(name, Perms::VIEW, &folder)?;
            if !indexed(name) {
                return Err(AppError::bad_request(format!(
                    "source {name:?} is not indexed"
                )));
            }
            let scope = user
                .grant(name)
                .map(|g| g.scope.clone())
                .unwrap_or_default();
            areas.push(Area {
                source: name.clone(),
                scope,
                under,
            });
        }
    }
    if areas.is_empty() {
        return Ok((Vec::new(), false));
    }

    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
        "SELECT f.source, f.path, f.name, f.kind, f.is_dir, f.size, f.mtime FROM file_index f WHERE (",
    );
    for (i, a) in areas.iter().enumerate() {
        if i > 0 {
            qb.push(" OR ");
        }
        qb.push("(f.source = ").push_bind(a.source.clone());
        if !a.under.is_root() {
            let (lo, hi) = descendant_range(&a.under);
            qb.push(" AND f.path >= ")
                .push_bind(lo)
                .push(" AND f.path < ")
                .push_bind(hi);
        }
        if !a.scope.is_root() {
            // A symlink is only shown if its target is inside the scope too.
            let (lo, hi) = descendant_range(&a.scope);
            qb.push(" AND (f.target IS NULL OR f.target = ")
                .push_bind(a.scope.to_string())
                .push(" OR (f.target >= ")
                .push_bind(lo)
                .push(" AND f.target < ")
                .push_bind(hi)
                .push("))");
        }
        qb.push(")");
    }
    qb.push(")");
    if !f.fts_terms.is_empty() {
        qb.push(" AND f.id IN (SELECT rowid FROM file_index_fts WHERE file_index_fts MATCH ")
            .push_bind(fts_query(&f.fts_terms))
            .push(")");
    }
    for t in &f.like_terms {
        qb.push(" AND f.name LIKE ")
            .push_bind(like_pattern(t))
            .push(" ESCAPE '\\'");
    }
    if !f.kinds.is_empty() {
        qb.push(" AND f.kind IN (");
        let mut sep = qb.separated(", ");
        for k in &f.kinds {
            sep.push_bind(k.clone());
        }
        qb.push(")");
    }
    if f.files_only {
        qb.push(" AND f.is_dir = 0");
    }
    if !f.exts.is_empty() {
        qb.push(" AND f.ext IN (");
        let mut sep = qb.separated(", ");
        for e in &f.exts {
            sep.push_bind(e.clone());
        }
        qb.push(")");
    }
    if let Some(n) = f.min_size {
        qb.push(" AND f.size >= ")
            .push_bind(i64::try_from(n).unwrap_or(i64::MAX));
    }
    if let Some(n) = f.max_size {
        qb.push(" AND f.size <= ")
            .push_bind(i64::try_from(n).unwrap_or(i64::MAX));
    }
    if let Some(t) = f.after {
        qb.push(" AND f.mtime >= ").push_bind(t);
    }
    if let Some(t) = f.before {
        qb.push(" AND f.mtime < ").push_bind(t);
    }
    if !f.tags.is_empty() {
        // tags.name is COLLATE NOCASE, so IN matches case-insensitively.
        qb.push(
            " AND (SELECT COUNT(DISTINCT t.id) FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
               WHERE ft.source = f.source AND ft.path = f.path AND t.name IN (",
        );
        let mut sep = qb.separated(", ");
        for t in &f.tags {
            sep.push_bind(t.clone());
        }
        qb.push(")) = ")
            .push_bind(i64::try_from(f.tags.len()).unwrap_or(i64::MAX));
    }
    if let Some(content) = &f.content {
        if content.is_empty() {
            return Ok((Vec::new(), false));
        }
        // Row-value IN: at most MAX_CANDIDATES pairs, well under SQLite's
        // bound-parameter limit.
        qb.push(" AND (f.source, f.path) IN (VALUES ");
        let mut sep = qb.separated(", ");
        for m in content {
            sep.push("(")
                .push_bind_unseparated(m.source.clone())
                .push_unseparated(", ")
                .push_bind_unseparated(m.path.clone())
                .push_unseparated(")");
        }
        qb.push(")");
    }
    qb.push(" ORDER BY ").push(f.order_by);
    if !f.by_relevance {
        qb.push(" LIMIT ")
            .push_bind(i64::from(f.limit) + 1)
            .push(" OFFSET ")
            .push_bind(i64::from(f.offset));
    }

    let mut rows: Vec<HitRow> = qb.build_query_as().fetch_all(db).await?;
    let content_of = |r: &HitRow| {
        f.content
            .as_ref()?
            .iter()
            .find(|m| m.source == r.source && m.path == r.path)
    };
    if f.by_relevance {
        // The candidates are already in relevance order; keep that order and
        // page here (there are at most MAX_CANDIDATES rows).
        let rank = |r: &HitRow| {
            f.content
                .as_ref()
                .and_then(|c| {
                    c.iter()
                        .position(|m| m.source == r.source && m.path == r.path)
                })
                .unwrap_or(usize::MAX)
        };
        rows.sort_by_key(|r| rank(r));
        rows.drain(..(f.offset as usize).min(rows.len()));
        rows.truncate(f.limit as usize + 1);
    }
    let has_more = rows.len() > f.limit as usize;
    let hits = rows
        .into_iter()
        .take(f.limit as usize)
        .filter_map(|r| {
            let scope = &areas.iter().find(|a| a.source == r.source)?.scope;
            let visible = VPath::parse(&r.path).ok()?.strip_prefix(scope)?;
            let snippet = content_of(&r).map(|m| m.snippet.clone());
            Some(SearchHit {
                snippet,
                mime: (!r.is_dir).then(|| guess_mime(&r.name)),
                source: r.source,
                path: visible.to_string(),
                name: r.name,
                kind: r.kind,
                is_dir: r.is_dir,
                size: u64::try_from(r.size).unwrap_or(0),
                modified: r
                    .mtime
                    .and_then(|t| OffsetDateTime::from_unix_timestamp(t).ok()),
            })
        })
        .collect();
    Ok((hits, has_more))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("512").unwrap(), 512);
        assert_eq!(parse_size("10k").unwrap(), 10 * 1024);
        assert_eq!(parse_size("1.5MB").unwrap(), 1024 * 1024 * 3 / 2);
        assert_eq!(parse_size("2 GiB").unwrap(), 2 << 30);
        assert!(parse_size("ten").is_err());
        assert!(parse_size("5 parsecs").is_err());
        assert!(parse_size("").is_err());
    }

    #[test]
    fn times() {
        assert_eq!(parse_time("1700000000").unwrap(), 1_700_000_000);
        assert_eq!(parse_time("2024-01-02").unwrap(), 1_704_153_600);
        assert_eq!(parse_time("2024-01-02T00:00:00Z").unwrap(), 1_704_153_600);
        assert_eq!(
            parse_time("2024-01-02T01:00:00+01:00").unwrap(),
            1_704_153_600
        );
        assert!(parse_time("yesterday").is_err());
    }

    #[test]
    fn escaping() {
        assert_eq!(
            fts_query(&["ab\"c".into(), "def".into()]),
            "\"ab\"\"c\" AND \"def\""
        );
        assert_eq!(like_pattern("a%_\\"), "%a\\%\\_\\\\%");
    }

    #[test]
    fn filters_validate() {
        let ok = Filters::from_params(&SearchParams {
            q: Some("ab report".into()),
            kind: Some("image, file".into()),
            ext: Some(".JPG,png".into()),
            sort: Some("size".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(ok.like_terms, ["ab"]);
        assert_eq!(ok.fts_terms, ["report"]);
        assert_eq!(ok.kinds, ["image"]);
        assert!(ok.files_only);
        assert_eq!(ok.exts, ["jpg", "png"]);
        assert!(ok.order_by.contains("size DESC"));

        for bad in [
            SearchParams {
                kind: Some("pictures".into()),
                ..Default::default()
            },
            SearchParams {
                sort: Some("random()".into()),
                ..Default::default()
            },
            SearchParams {
                order: Some("sideways".into()),
                ..Default::default()
            },
            SearchParams {
                limit: Some(0),
                ..Default::default()
            },
            SearchParams {
                limit: Some(5000),
                ..Default::default()
            },
            SearchParams {
                path: Some("/a".into()),
                ..Default::default()
            },
            SearchParams {
                path: Some("/../x".into()),
                source: Some("s".into()),
                ..Default::default()
            },
            SearchParams {
                q: Some("x".repeat(300)),
                ..Default::default()
            },
        ] {
            assert!(Filters::from_params(&bad).is_err(), "{bad:?}");
        }
    }
}
