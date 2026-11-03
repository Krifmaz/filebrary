//! Groups of users. Permission rules can target a group, and every member
//! receives what the group is granted.

use serde::Serialize;
use sqlx::SqlitePool;

use super::users::UserRecord;
use crate::{
    db,
    error::{AppError, AppResult},
};

fn map_unique(e: sqlx::Error) -> AppError {
    match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            AppError::conflict("that group name is already taken")
        }
        _ => e.into(),
    }
}

/// A named group of users.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Group {
    /// Database id.
    pub id: i64,
    /// Unique name, compared case-insensitively.
    pub name: String,
    /// Unix time the group was created.
    pub created_at: i64,
}

/// Creates a group. Names follow the same rules as usernames.
///
/// # Errors
/// `BadRequest` for an invalid name, `Conflict` if taken.
pub async fn create(pool: &SqlitePool, name: &str) -> AppResult<Group> {
    super::users::validate_name(name)?;
    sqlx::query_as(
        "INSERT INTO groups (name, created_at) VALUES (?, ?) RETURNING id, name, created_at",
    )
    .bind(name)
    .bind(db::now())
    .fetch_one(pool)
    .await
    .map_err(map_unique)
}

/// Lists all groups ordered by name.
///
/// # Errors
/// Database failures.
pub async fn list(pool: &SqlitePool) -> AppResult<Vec<Group>> {
    Ok(
        sqlx::query_as("SELECT id, name, created_at FROM groups ORDER BY name")
            .fetch_all(pool)
            .await?,
    )
}

/// Fetches a group by id.
///
/// # Errors
/// [`AppError::NotFound`] if there is no such group.
pub async fn get(pool: &SqlitePool, id: i64) -> AppResult<Group> {
    sqlx::query_as("SELECT id, name, created_at FROM groups WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
}

/// Deletes a group, its memberships and its permission rules.
///
/// # Errors
/// [`AppError::NotFound`] if there is no such group.
pub async fn delete(pool: &SqlitePool, id: i64) -> AppResult<()> {
    let res = sqlx::query("DELETE FROM groups WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// Adds a user to a group. Adding an existing member is a no-op.
///
/// # Errors
/// [`AppError::NotFound`] if the group or user does not exist.
pub async fn add_member(pool: &SqlitePool, group_id: i64, user_id: i64) -> AppResult<()> {
    get(pool, group_id).await?;
    super::users::get(pool, user_id).await?;
    sqlx::query("INSERT OR IGNORE INTO group_members (group_id, user_id) VALUES (?, ?)")
        .bind(group_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Removes a user from a group.
///
/// # Errors
/// [`AppError::NotFound`] if the user was not a member.
pub async fn remove_member(pool: &SqlitePool, group_id: i64, user_id: i64) -> AppResult<()> {
    let res = sqlx::query("DELETE FROM group_members WHERE group_id = ? AND user_id = ?")
        .bind(group_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// Lists the members of a group.
///
/// # Errors
/// [`AppError::NotFound`] if the group does not exist.
pub async fn members(pool: &SqlitePool, group_id: i64) -> AppResult<Vec<UserRecord>> {
    get(pool, group_id).await?;
    Ok(sqlx::query_as(
        "SELECT u.id, u.username, u.auth_source, u.is_admin, u.disabled, u.created_at
         FROM users u JOIN group_members m ON m.user_id = u.id
         WHERE m.group_id = ? ORDER BY u.username",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await?)
}

/// Lists the groups a user belongs to.
///
/// # Errors
/// Database failures.
pub async fn for_user(pool: &SqlitePool, user_id: i64) -> AppResult<Vec<Group>> {
    Ok(sqlx::query_as(
        "SELECT g.id, g.name, g.created_at
         FROM groups g JOIN group_members m ON m.group_id = g.id
         WHERE m.user_id = ? ORDER BY g.name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}
