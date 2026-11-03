//! Comments on files and folders.
//!
//! Nested at `/api/comments`:
//!
//! | Method   | Path                          | What                                      |
//! |----------|-------------------------------|-------------------------------------------|
//! | `GET`    | `?source=&path=`              | The thread on one file, oldest first.     |
//! | `POST`   | `` (body `{source,path,body}`)| Add a comment.                            |
//! | `GET`    | `/counts?source=&dir=`        | Comment counts for the children of `dir`. |
//! | `PATCH`  | `/{id}` (body `{body}`)       | Edit one's own comment.                   |
//! | `DELETE` | `/{id}`                       | Delete (author, admin, or `edit` holder). |
//!
//! # Permissions
//!
//! Reading a thread needs `view` on the file; writing needs `download`, so a
//! user who can only see names cannot post. Authors may edit and delete their
//! own comments; anyone with `edit` on the file (and administrators) may
//! delete any comment on it, which is how threads are moderated.
//!
//! Comments are keyed by the *real* path inside the source, so users with
//! different scopes share one thread, and every path in a response is mapped
//! back to what the caller sees.
//!
//! # Keeping threads attached
//!
//! The files code calls [`on_rename`] after a successful move/rename and
//! [`on_delete`] after a permanent delete (not when moving to the trash, so a
//! restored file gets its comments back).

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, patch},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{
    auth::{AuthUser, Perms, User},
    db,
    error::{AppError, AppResult},
    state::AppState,
    storage::VPath,
};

/// Longest comment accepted, in characters.
pub const MAX_BODY_CHARS: usize = 10_000;

/// Routes nested at `/api/comments`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/counts", get(counts))
        .route("/{id}", patch(edit).delete(remove))
}

/// A comment as returned by the API.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Comment {
    /// Database id.
    pub id: i64,
    /// Author's user id, `None` if the account was deleted.
    pub user_id: Option<i64>,
    /// Author's login name, `None` if the account was deleted.
    pub username: Option<String>,
    /// Plain text; clients must render it as text, never as HTML.
    pub body: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds of the last edit, if any.
    pub edited_at: Option<i64>,
    /// Whether the caller may edit this comment.
    pub can_edit: bool,
    /// Whether the caller may delete this comment.
    pub can_delete: bool,
}

/// Identifies a file: `source` plus the path as the caller sees it.
#[derive(Debug, Deserialize)]
pub struct Target {
    /// Source name.
    pub source: String,
    /// Path as the caller sees it.
    pub path: String,
}

/// Body of `POST /api/comments`.
#[derive(Debug, Deserialize)]
pub struct CreateComment {
    /// Source name.
    pub source: String,
    /// Path as the caller sees it.
    pub path: String,
    /// Comment text.
    pub body: String,
}

/// Body of `PATCH /api/comments/{id}`.
#[derive(Debug, Deserialize)]
pub struct EditComment {
    /// New text.
    pub body: String,
}

/// Query of `GET /api/comments/counts`.
#[derive(Debug, Deserialize)]
pub struct CountsQuery {
    /// Source name.
    pub source: String,
    /// Directory as the caller sees it.
    pub dir: String,
}

/// Trims and checks a comment body.
fn clean_body(body: &str) -> AppResult<String> {
    let body = body.trim();
    if body.is_empty() {
        return Err(AppError::bad_request("the comment is empty"));
    }
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(AppError::bad_request(format!(
            "comments are limited to {MAX_BODY_CHARS} characters"
        )));
    }
    Ok(body.to_owned())
}

/// Maps a user-visible path to the real one after checking `perm`.
fn resolve(user: &User, source: &str, path: &str, perm: Perms) -> AppResult<VPath> {
    user.require(source, perm, &VPath::parse(path)?)
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    user_id: Option<i64>,
    username: Option<String>,
    body: String,
    created_at: i64,
    edited_at: Option<i64>,
}

fn to_comment(r: Row, me: &User, moderator: bool) -> Comment {
    let mine = r.user_id == Some(me.id);
    Comment {
        id: r.id,
        user_id: r.user_id,
        username: r.username,
        body: r.body,
        created_at: r.created_at,
        edited_at: r.edited_at,
        can_edit: mine,
        can_delete: mine || moderator,
    }
}

/// Whether `user` may moderate comments on `source:real`.
fn is_moderator(user: &User, source: &str, real: &VPath) -> bool {
    user.is_admin
        || user
            .grant(source)
            .is_some_and(|g| g.perms.contains(Perms::EDIT) && real.starts_with(&g.scope))
}

/// `GET /api/comments?source=&path=`: the thread on one file, oldest first.
async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(t): Query<Target>,
) -> AppResult<Json<Vec<Comment>>> {
    let real = resolve(&auth.user, &t.source, &t.path, Perms::VIEW)?;
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT c.id, c.user_id, u.username, c.body, c.created_at, c.edited_at
         FROM comments c LEFT JOIN users u ON u.id = c.user_id
         WHERE c.source = ? AND c.path = ? ORDER BY c.created_at, c.id",
    )
    .bind(&t.source)
    .bind(real.to_string())
    .fetch_all(&state.db)
    .await?;
    let moderator = is_moderator(&auth.user, &t.source, &real);
    Ok(Json(
        rows.into_iter()
            .map(|r| to_comment(r, &auth.user, moderator))
            .collect(),
    ))
}

/// `POST /api/comments`: adds a comment to an existing file or folder.
async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreateComment>,
) -> AppResult<(StatusCode, Json<Comment>)> {
    let body = clean_body(&req.body)?;
    let visible = VPath::parse(&req.path)?;
    let real = auth.user.require(&req.source, Perms::DOWNLOAD, &visible)?;
    crate::api::files::scoped(&state, &auth.user, &req.source, Perms::DOWNLOAD, &visible)
        .await?
        .stat(&visible)
        .await?;
    let now = db::now();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO comments (source, path, user_id, body, created_at)
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(&req.source)
    .bind(real.to_string())
    .bind(auth.user.id)
    .bind(&body)
    .bind(now)
    .fetch_one(&state.db)
    .await?;
    tracing::info!(user = auth.user.id, source = %req.source, comment = id, "comment added");
    Ok((
        StatusCode::CREATED,
        Json(Comment {
            id,
            user_id: Some(auth.user.id),
            username: Some(auth.user.username.clone()),
            body,
            created_at: now,
            edited_at: None,
            can_edit: true,
            can_delete: true,
        }),
    ))
}

/// Loads a comment's target and author, checking the caller can still see it.
async fn load_for_change(
    state: &AppState,
    user: &User,
    id: i64,
) -> AppResult<(String, VPath, Option<i64>)> {
    let (source, path, author): (String, String, Option<i64>) =
        sqlx::query_as("SELECT source, path, user_id FROM comments WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .ok_or(AppError::NotFound)?;
    let real = VPath::parse(&path).map_err(|_| AppError::NotFound)?;
    // The caller must still be able to see the file, or the comment is hidden.
    let visible = user
        .visible_path(&source, &real)
        .ok_or(AppError::NotFound)?;
    user.require(&source, Perms::VIEW, &visible)?;
    Ok((source, real, author))
}

/// `PATCH /api/comments/{id}`: edits one's own comment.
async fn edit(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<EditComment>,
) -> AppResult<StatusCode> {
    let body = clean_body(&req.body)?;
    let (_, _, author) = load_for_change(&state, &auth.user, id).await?;
    if author != Some(auth.user.id) {
        return Err(AppError::forbidden("you can only edit your own comments"));
    }
    sqlx::query("UPDATE comments SET body = ?, edited_at = ? WHERE id = ?")
        .bind(body)
        .bind(db::now())
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/comments/{id}`.
async fn remove(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let (source, real, author) = load_for_change(&state, &auth.user, id).await?;
    if author != Some(auth.user.id) && !is_moderator(&auth.user, &source, &real) {
        return Err(AppError::forbidden(
            "you can only delete your own comments here",
        ));
    }
    sqlx::query("DELETE FROM comments WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/comments/counts?source=&dir=`: `{ "<child name>": count }` for
/// the direct children of `dir` that have comments, for list badges.
async fn counts(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<CountsQuery>,
) -> AppResult<Json<BTreeMap<String, i64>>> {
    let dir = resolve(&auth.user, &q.source, &q.dir, Perms::VIEW)?;
    let prefix = if dir.is_root() {
        "/".to_owned()
    } else {
        format!("{dir}/")
    };
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT path, COUNT(*) FROM comments
         WHERE source = ? AND substr(path, 1, length(?)) = ? GROUP BY path",
    )
    .bind(&q.source)
    .bind(&prefix)
    .bind(&prefix)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .filter_map(|(p, n)| {
                let rest = p.strip_prefix(&prefix)?;
                (!rest.is_empty() && !rest.contains('/')).then(|| (rest.to_owned(), n))
            })
            .collect(),
    ))
}

/// Re-points comments after `from` was moved to `to` in `source` (both real
/// paths). Comments on everything below a moved folder move with it.
///
/// # Errors
///
/// Database failures.
pub async fn on_rename(
    db: &sqlx::SqlitePool,
    source: &str,
    from: &VPath,
    to: &VPath,
) -> AppResult<()> {
    let (from, to) = (from.to_string(), to.to_string());
    let from_prefix = format!("{from}/");
    sqlx::query(
        "UPDATE comments SET path = CASE WHEN path = ?1 THEN ?2
                                          ELSE ?2 || substr(path, length(?1) + 1) END
         WHERE source = ?3 AND (path = ?1 OR substr(path, 1, length(?4)) = ?4)",
    )
    .bind(&from)
    .bind(&to)
    .bind(source)
    .bind(&from_prefix)
    .execute(db)
    .await?;
    Ok(())
}

/// Removes the comments on `path` (a real path) and everything below it,
/// after a permanent delete.
///
/// # Errors
///
/// Database failures.
pub async fn on_delete(db: &sqlx::SqlitePool, source: &str, path: &VPath) -> AppResult<()> {
    let p = path.to_string();
    let prefix = format!("{p}/");
    sqlx::query(
        "DELETE FROM comments WHERE source = ? AND (path = ? OR substr(path, 1, length(?)) = ?)",
    )
    .bind(source)
    .bind(&p)
    .bind(&prefix)
    .bind(&prefix)
    .execute(db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_rules() {
        assert!(clean_body("   ").is_err());
        assert_eq!(clean_body("  hi \n").unwrap(), "hi");
        assert!(clean_body(&"x".repeat(MAX_BODY_CHARS + 1)).is_err());
        assert!(clean_body(&"é".repeat(MAX_BODY_CHARS)).is_ok());
    }

    async fn seed(db: &sqlx::SqlitePool, path: &str) {
        sqlx::query(
            "INSERT INTO comments (source, path, body, created_at) VALUES ('s', ?, 'x', 0)",
        )
        .bind(path)
        .execute(db)
        .await
        .unwrap();
    }

    async fn paths(db: &sqlx::SqlitePool) -> Vec<String> {
        sqlx::query_scalar("SELECT path FROM comments ORDER BY path")
            .fetch_all(db)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn rename_and_delete_follow_files() {
        let db = db::open_memory().await.unwrap();
        for p in ["/a", "/a/b.txt", "/ab", "/c"] {
            seed(&db, p).await;
        }
        let v = |s| VPath::parse(s).unwrap();
        on_rename(&db, "s", &v("a"), &v("z/a2")).await.unwrap();
        // "/ab" shares a string prefix with "/a" but is not inside it.
        assert_eq!(paths(&db).await, ["/ab", "/c", "/z/a2", "/z/a2/b.txt"]);
        on_delete(&db, "s", &v("z")).await.unwrap();
        assert_eq!(paths(&db).await, ["/ab", "/c"]);
    }
}
