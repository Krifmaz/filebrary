//! User accounts and their per-source scopes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::{password, session};
use crate::{
    db,
    error::{AppError, AppResult},
    storage::VPath,
};

/// Longest username accepted.
pub const MAX_USERNAME_LEN: usize = 64;

/// A user account as stored (never includes the password hash).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct UserRecord {
    /// Database id.
    pub id: i64,
    /// Login name: unique, compared case-insensitively.
    pub username: String,
    /// Where the account authenticates: `local` (password), `oidc`, `ldap`...
    pub auth_source: String,
    /// Administrators have every permission and manage users.
    pub is_admin: bool,
    /// Disabled accounts cannot log in and their sessions stop working.
    pub disabled: bool,
    /// Unix creation time.
    pub created_at: i64,
}

macro_rules! user_cols {
    () => {
        "id, username, auth_source, is_admin, disabled, created_at"
    };
}

/// Checks a user or group name: 1 to 64 ASCII letters, digits, `.`, `_`,
/// `-` or `@`, not starting with `.` or `-`.
///
/// # Errors
///
/// `BadRequest` describing the rule.
pub fn validate_name(name: &str) -> AppResult<()> {
    let ok = !name.is_empty()
        && name.len() <= MAX_USERNAME_LEN
        && !name.starts_with(['.', '-'])
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@'));
    if ok {
        Ok(())
    } else {
        Err(AppError::bad_request(format!(
            "names must be 1-{MAX_USERNAME_LEN} letters, digits, '.', '_', '-' or '@', \
             not starting with '.' or '-'"
        )))
    }
}

fn map_unique(e: sqlx::Error) -> AppError {
    match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            AppError::conflict("that name is already taken")
        }
        _ => e.into(),
    }
}

/// Creates a local user with a password (validated and hashed here).
///
/// # Errors
///
/// `BadRequest` for an invalid name or password, `Conflict` if the name is taken.
pub async fn create(
    db: &SqlitePool,
    username: &str,
    password: &str,
    is_admin: bool,
) -> AppResult<UserRecord> {
    validate_name(username)?;
    password::validate(password)?;
    let hash = password::hash(password.to_owned()).await?;
    let now = db::now();
    sqlx::query_as(concat!("INSERT INTO users (username, password_hash, auth_source, is_admin, disabled, created_at, updated_at)
         VALUES (?, ?, 'local', ?, 0, ?, ?) RETURNING ", user_cols!()))
    .bind(username)
    .bind(hash)
    .bind(is_admin)
    .bind(now)
    .bind(now)
    .fetch_one(db)
    .await
    .map_err(map_unique)
}

/// Creates a user without a password for an external identity provider
/// (`auth_source` such as `oidc` or `ldap`).
///
/// # Errors
///
/// `BadRequest` for an invalid name or `auth_source` of `local`, `Conflict`
/// if the name is taken.
pub async fn create_external(
    db: &SqlitePool,
    username: &str,
    auth_source: &str,
    is_admin: bool,
) -> AppResult<UserRecord> {
    validate_name(username)?;
    if auth_source == "local" || auth_source.is_empty() {
        return Err(AppError::bad_request(
            "external users need a non-local auth source",
        ));
    }
    let now = db::now();
    sqlx::query_as(concat!("INSERT INTO users (username, password_hash, auth_source, is_admin, disabled, created_at, updated_at)
         VALUES (?, NULL, ?, ?, 0, ?, ?) RETURNING ", user_cols!()))
    .bind(username)
    .bind(auth_source)
    .bind(is_admin)
    .bind(now)
    .bind(now)
    .fetch_one(db)
    .await
    .map_err(map_unique)
}

/// Fetches a user by id.
///
/// # Errors
///
/// `NotFound` if there is no such user.
pub async fn get(db: &SqlitePool, id: i64) -> AppResult<UserRecord> {
    sqlx::query_as(concat!("SELECT ", user_cols!(), " FROM users WHERE id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound)
}

/// Fetches a user and password hash by (case-insensitive) username.
///
/// # Errors
///
/// Database failures.
pub(crate) async fn get_with_hash(
    db: &SqlitePool,
    username: &str,
) -> AppResult<Option<(UserRecord, Option<String>)>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        #[sqlx(flatten)]
        user: UserRecord,
        password_hash: Option<String>,
    }
    let row: Option<Row> = sqlx::query_as(concat!(
        "SELECT ",
        user_cols!(),
        ", password_hash FROM users WHERE username = ?"
    ))
    .bind(username)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|r| (r.user, r.password_hash)))
}

/// Lists all users by username.
///
/// # Errors
///
/// Database failures.
pub async fn list(db: &SqlitePool) -> AppResult<Vec<UserRecord>> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        user_cols!(),
        " FROM users ORDER BY username"
    ))
    .fetch_all(db)
    .await?)
}

/// Number of user accounts.
///
/// # Errors
///
/// Database failures.
pub async fn count(db: &SqlitePool) -> AppResult<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(db)
        .await?)
}

async fn enabled_admins(db: &SqlitePool) -> AppResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE is_admin = 1 AND disabled = 0")
            .fetch_one(db)
            .await?,
    )
}

/// Changes for [`update`]; `None` fields stay as they are.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserUpdate {
    /// New username.
    pub username: Option<String>,
    /// Grant or revoke administrator rights.
    pub is_admin: Option<bool>,
    /// Disable or re-enable the account.
    pub disabled: Option<bool>,
    /// New password (local accounts only). Ends all the user's sessions.
    pub password: Option<String>,
}

/// Updates a user. Disabling an account or setting a password ends all of
/// its sessions. Refuses to leave the server without an enabled administrator.
///
/// # Errors
///
/// `NotFound`, `Conflict` for a taken name, `BadRequest` for invalid values
/// or when the last enabled administrator would be lost.
pub async fn update(db: &SqlitePool, id: i64, upd: &UserUpdate) -> AppResult<UserRecord> {
    let cur = get(db, id).await?;
    if let Some(name) = &upd.username {
        validate_name(name)?;
    }
    if let Some(pw) = &upd.password {
        if cur.auth_source != "local" {
            return Err(AppError::bad_request(
                "this account signs in through an external provider",
            ));
        }
        password::validate(pw)?;
    }
    let is_admin = upd.is_admin.unwrap_or(cur.is_admin);
    let disabled = upd.disabled.unwrap_or(cur.disabled);
    if cur.is_admin && !cur.disabled && (!is_admin || disabled) && enabled_admins(db).await? <= 1 {
        return Err(AppError::bad_request(
            "cannot remove the last enabled administrator",
        ));
    }
    let user: UserRecord = sqlx::query_as(concat!(
        "UPDATE users SET username = ?, is_admin = ?, disabled = ?, updated_at = ?
         WHERE id = ? RETURNING ",
        user_cols!()
    ))
    .bind(upd.username.as_deref().unwrap_or(&cur.username))
    .bind(is_admin)
    .bind(disabled)
    .bind(db::now())
    .bind(id)
    .fetch_one(db)
    .await
    .map_err(map_unique)?;
    if let Some(pw) = &upd.password {
        set_password(db, id, pw).await?;
    }
    if (disabled && !cur.disabled) || upd.password.is_some() {
        session::delete_for_user(db, id, None).await?;
    }
    Ok(user)
}

/// Stores a new password (validated and hashed here). Does not end
/// sessions; the caller decides which to keep.
///
/// # Errors
///
/// `BadRequest` for an invalid password, `NotFound`.
pub async fn set_password(db: &SqlitePool, id: i64, password: &str) -> AppResult<()> {
    password::validate(password)?;
    let hash = password::hash(password.to_owned()).await?;
    let r = sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?")
        .bind(hash)
        .bind(db::now())
        .bind(id)
        .execute(db)
        .await?;
    if r.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// Deletes a user with their sessions, memberships, rules and scopes.
///
/// # Errors
///
/// `NotFound`, or `BadRequest` for the last enabled administrator.
pub async fn delete(db: &SqlitePool, id: i64) -> AppResult<()> {
    let u = get(db, id).await?;
    if u.is_admin && !u.disabled && enabled_admins(db).await? <= 1 {
        return Err(AppError::bad_request(
            "cannot delete the last enabled administrator",
        ));
    }
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// The user's scope (home folder) on each source that has one.
///
/// # Errors
///
/// Database failures.
pub async fn scopes(db: &SqlitePool, id: i64) -> AppResult<BTreeMap<String, VPath>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT source, scope FROM user_scopes WHERE user_id = ?")
            .bind(id)
            .fetch_all(db)
            .await?;
    let mut out = BTreeMap::new();
    for (source, scope) in rows {
        match VPath::parse(&scope) {
            Ok(p) => {
                out.insert(source, p);
            }
            Err(_) => tracing::warn!(user = id, %source, "ignoring invalid scope"),
        }
    }
    Ok(out)
}

/// Sets (or with the root path, clears) the user's scope on a source.
///
/// # Errors
///
/// `NotFound` if the user does not exist; database failures.
pub async fn set_scope(db: &SqlitePool, id: i64, source: &str, scope: &VPath) -> AppResult<()> {
    get(db, id).await?;
    if scope.is_root() {
        sqlx::query("DELETE FROM user_scopes WHERE user_id = ? AND source = ?")
            .bind(id)
            .bind(source)
            .execute(db)
            .await?;
    } else {
        sqlx::query(
            "INSERT INTO user_scopes (user_id, source, scope) VALUES (?, ?, ?)
             ON CONFLICT (user_id, source) DO UPDATE SET scope = excluded.scope",
        )
        .bind(id)
        .bind(source)
        .bind(scope.to_string())
        .execute(db)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_name;

    #[test]
    fn names() {
        for ok in ["alice", "a.b", "a_b-c", "me@example.org", "A1"] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        let long = "a".repeat(65);
        for bad in [
            "",
            ".hidden",
            "-x",
            "a b",
            "a/b",
            "../x",
            "ü",
            long.as_str(),
        ] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
    }
}
