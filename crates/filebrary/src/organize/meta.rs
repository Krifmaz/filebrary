//! Keeping per-path metadata (tags, stars and comments) attached to files as they move.
//!
//! Tags and stars are stored by `(source, path)` with the *real* path inside
//! the source. Whenever a file or folder is renamed, moved, copied or deleted,
//! the code that did it must call the matching hook here so the rows follow:
//!
//! | Storage operation          | Hook          |
//! |----------------------------|---------------|
//! | rename / move              | [`on_move`]   |
//! | copy                       | [`on_copy`]   |
//! | permanent delete           | [`on_delete`] |
//!
//! Every hook applies to the path itself *and* everything below it, so moving
//! a folder carries the tags of every file inside it. The hooks only touch the
//! database; call them after the storage operation succeeded.

use serde::Serialize;
use sqlx::SqlitePool;

use crate::{db, storage::VPath};

/// The database key for a path: its display form, with the root as `""` so
/// that the prefix test `substr(path, 1, len + 1) = key || '/'` matches every
/// row when applied to the root.
pub(crate) fn key(p: &VPath) -> String {
    if p.is_root() {
        String::new()
    } else {
        p.to_string()
    }
}

/// SQL condition with three positional parameters (bind the [`key`] three
/// times) that matches `path` equal to the key or lying below it.
pub(crate) const UNDER: &str = "(path = ? OR substr(path, 1, length(?) + 1) = ? || '/')";

/// Tables whose rows are keyed by `(source, path)` and follow the file.
/// (`comments` threads follow their file too.)
const TABLES: [&str; 3] = ["file_tags", "stars", "comments"];

/// Moves tags and stars from `from` (and below) to `to` (possibly in another
/// source). Rows already at `to` are replaced, matching an overwriting move.
///
/// # Errors
///
/// Database failures.
pub async fn on_move(
    pool: &SqlitePool,
    from_source: &str,
    from: &VPath,
    to_source: &str,
    to: &VPath,
) -> sqlx::Result<()> {
    if from_source == to_source && from == to {
        return Ok(());
    }
    let (fk, tk) = (key(from), key(to));
    let mut tx = pool.begin().await?;
    for table in TABLES {
        // Drop whatever was at the destination (an overwritten file's tags).
        sqlx::query(crate::sql_fmt!(
            "DELETE FROM {table} WHERE source = ? AND {UNDER}"
        ))
        .bind(to_source)
        .bind(&tk)
        .bind(&tk)
        .bind(&tk)
        .execute(&mut *tx)
        .await?;
        sqlx::query(crate::sql_fmt!(
            "UPDATE {table} SET source = ?, path = ? || substr(path, length(?) + 1)
             WHERE source = ? AND {UNDER}"
        ))
        .bind(to_source)
        .bind(&tk)
        .bind(&fk)
        .bind(from_source)
        .bind(&fk)
        .bind(&fk)
        .bind(&fk)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// Copies the tags (not the stars, which are personal bookmarks of the
/// original) of `from` and everything below it to `to`.
///
/// # Errors
///
/// Database failures.
pub async fn on_copy(
    pool: &SqlitePool,
    from_source: &str,
    from: &VPath,
    to_source: &str,
    to: &VPath,
) -> sqlx::Result<()> {
    if from_source == to_source && from == to {
        return Ok(());
    }
    let (fk, tk) = (key(from), key(to));
    sqlx::query(crate::sql_fmt!(
        "INSERT OR IGNORE INTO file_tags (tag_id, source, path, added_by, added_at)
         SELECT tag_id, ?, ? || substr(path, length(?) + 1), added_by, ?
         FROM file_tags WHERE source = ? AND {UNDER}"
    ))
    .bind(to_source)
    .bind(&tk)
    .bind(&fk)
    .bind(db::now())
    .bind(from_source)
    .bind(&fk)
    .bind(&fk)
    .bind(&fk)
    .execute(pool)
    .await?;
    Ok(())
}

/// Forgets the tags and stars of `path` and everything below it.
///
/// # Errors
///
/// Database failures.
pub async fn on_delete(pool: &SqlitePool, source: &str, path: &VPath) -> sqlx::Result<()> {
    let k = key(path);
    let mut tx = pool.begin().await?;
    for table in TABLES {
        sqlx::query(crate::sql_fmt!(
            "DELETE FROM {table} WHERE source = ? AND {UNDER}"
        ))
        .bind(source)
        .bind(&k)
        .bind(&k)
        .bind(&k)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// A tag as shown next to a file.
#[derive(Clone, Debug, Serialize, PartialEq, Eq, sqlx::FromRow)]
pub struct TagRef {
    /// Tag id.
    pub id: i64,
    /// Tag name.
    pub name: String,
    /// Display colour (`#rrggbb`), if set.
    pub color: Option<String>,
}

/// Star and tags for one entry of a directory listing.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct Annotation {
    /// Whether the current user starred it.
    pub starred: bool,
    /// Its tags, sorted by name.
    pub tags: Vec<TagRef>,
}

/// Returns the star and tags of every direct child of the directory `dir`
/// (a real path) that has any, keyed by the child's file name.
///
/// Used by `GET /api/annotations` so the web UI can decorate a listing with
/// one extra request; the files work stream may also call it directly.
///
/// # Errors
///
/// Database failures.
pub async fn annotate_dir(
    pool: &SqlitePool,
    user_id: i64,
    source: &str,
    dir: &VPath,
) -> sqlx::Result<std::collections::BTreeMap<String, Annotation>> {
    let prefix = format!("{}/", key(dir));
    // Direct children: start with "<dir>/" and contain no further '/'.
    const CHILD: &str =
        "substr(path, 1, length(?)) = ? AND instr(substr(path, length(?) + 1), '/') = 0";
    let mut out = std::collections::BTreeMap::<String, Annotation>::new();

    let starred: Vec<(String,)> = sqlx::query_as(crate::sql_fmt!(
        "SELECT path FROM stars WHERE user_id = ? AND source = ? AND {CHILD}"
    ))
    .bind(user_id)
    .bind(source)
    .bind(&prefix)
    .bind(&prefix)
    .bind(&prefix)
    .fetch_all(pool)
    .await?;
    for (p,) in starred {
        out.entry(p[prefix.len()..].to_owned()).or_default().starred = true;
    }

    let tagged: Vec<(String, i64, String, Option<String>)> = sqlx::query_as(crate::sql_fmt!(
        "SELECT ft.path, t.id, t.name, t.color FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
         WHERE ft.source = ? AND {}
         ORDER BY t.name COLLATE NOCASE",
        CHILD.replace("path", "ft.path")
    ))
    .bind(source)
    .bind(&prefix)
    .bind(&prefix)
    .bind(&prefix)
    .fetch_all(pool)
    .await?;
    for (p, id, name, color) in tagged {
        out.entry(p[prefix.len()..].to_owned())
            .or_default()
            .tags
            .push(TagRef { id, name, color });
    }
    Ok(out)
}

/// The tags of one path, sorted by name.
///
/// # Errors
///
/// Database failures.
pub async fn tags_of(pool: &SqlitePool, source: &str, path: &VPath) -> sqlx::Result<Vec<TagRef>> {
    sqlx::query_as(
        "SELECT t.id, t.name, t.color FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
         WHERE ft.source = ? AND ft.path = ? ORDER BY t.name COLLATE NOCASE",
    )
    .bind(source)
    .bind(path.to_string())
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp(s: &str) -> VPath {
        VPath::parse(s).unwrap()
    }

    async fn setup() -> SqlitePool {
        let pool = db::open_memory().await.unwrap();
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, created_at, updated_at) VALUES (1, 'a', 'x', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO tags (id, name, created_at) VALUES (1, 'red', 0), (2, 'blue', 0)")
            .execute(&pool)
            .await
            .unwrap();
        for (tag, path) in [
            (1, "/d"),
            (1, "/d/x"),
            (2, "/d/sub/y"),
            (1, "/dd"),
            (2, "/other"),
        ] {
            sqlx::query(
                "INSERT INTO file_tags (tag_id, source, path, added_at) VALUES (?, 's', ?, 0)",
            )
            .bind(tag)
            .bind(path)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO stars (user_id, source, path, created_at) VALUES (1, 's', '/d/x', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn paths(pool: &SqlitePool, table: &str) -> Vec<String> {
        let rows: Vec<(String, String)> = sqlx::query_as(crate::sql_fmt!(
            "SELECT source, path FROM {table} ORDER BY source, path"
        ))
        .fetch_all(pool)
        .await
        .unwrap();
        rows.into_iter().map(|(s, p)| format!("{s}:{p}")).collect()
    }

    #[tokio::test]
    async fn move_carries_subtree_but_not_siblings() {
        let pool = setup().await;
        on_move(&pool, "s", &vp("d"), "s", &vp("e/f"))
            .await
            .unwrap();
        assert_eq!(
            paths(&pool, "file_tags").await,
            ["s:/dd", "s:/e/f", "s:/e/f/sub/y", "s:/e/f/x", "s:/other"]
        );
        assert_eq!(paths(&pool, "stars").await, ["s:/e/f/x"]);
    }

    #[tokio::test]
    async fn move_across_sources_and_overwrite() {
        let pool = setup().await;
        // Moving /d/x over /other replaces /other's tags.
        on_move(&pool, "s", &vp("d/x"), "s", &vp("other"))
            .await
            .unwrap();
        assert_eq!(
            paths(&pool, "file_tags").await,
            ["s:/d", "s:/d/sub/y", "s:/dd", "s:/other"]
        );
        on_move(&pool, "s", &vp("dd"), "t", &vp("z")).await.unwrap();
        assert!(paths(&pool, "file_tags").await.contains(&"t:/z".to_owned()));
    }

    #[tokio::test]
    async fn copy_and_delete() {
        let pool = setup().await;
        on_copy(&pool, "s", &vp("d"), "s", &vp("c")).await.unwrap();
        assert_eq!(paths(&pool, "file_tags").await.len(), 8);
        assert_eq!(
            paths(&pool, "stars").await,
            ["s:/d/x"],
            "stars are not copied"
        );
        on_delete(&pool, "s", &vp("d")).await.unwrap();
        assert_eq!(
            paths(&pool, "file_tags").await,
            ["s:/c", "s:/c/sub/y", "s:/c/x", "s:/dd", "s:/other"]
        );
        assert!(paths(&pool, "stars").await.is_empty());
    }

    #[tokio::test]
    async fn annotates_direct_children_only() {
        let pool = setup().await;
        let a = annotate_dir(&pool, 1, "s", &vp("d")).await.unwrap();
        assert_eq!(a.keys().collect::<Vec<_>>(), ["x"]);
        assert!(a["x"].starred);
        assert_eq!(a["x"].tags[0].name, "red");
        let root = annotate_dir(&pool, 1, "s", &VPath::root()).await.unwrap();
        assert_eq!(root.keys().collect::<Vec<_>>(), ["d", "dd", "other"]);
        assert!(!root["d"].starred);
    }
}
