//! Login sessions stored in SQLite.
//!
//! The cookie carries a random 256-bit token; the database keeps only its
//! SHA-256, so a leaked database cannot be replayed as cookies. Each session
//! has its own CSRF token. Expiry slides: a session used after half its
//! lifetime is extended.

use axum_extra::extract::cookie::{Cookie, SameSite};

use crate::{crypto, db, error::AppResult, state::AppState};

/// Name of the session cookie.
pub const SESSION_COOKIE: &str = "fb_session";

/// A valid session as found in the database.
#[derive(Debug, Clone)]
pub struct Session {
    /// SHA-256 of the cookie token.
    pub token_hash: Vec<u8>,
    /// Owner.
    pub user_id: i64,
    /// CSRF token for this session.
    pub csrf_token: String,
    /// Unix expiry time.
    pub expires_at: i64,
}

/// A freshly created session: the raw token goes into the cookie.
#[derive(Debug, Clone)]
pub struct NewSession {
    /// Raw cookie token (never stored).
    pub token: String,
    /// CSRF token to hand to the client.
    pub csrf_token: String,
}

fn ttl_secs(state: &AppState) -> i64 {
    i64::try_from(state.config.session_ttl_hours.saturating_mul(3600)).unwrap_or(i64::MAX / 2)
}

/// Creates a session for `user_id`.
///
/// # Errors
///
/// Database failures.
pub async fn create(
    state: &AppState,
    user_id: i64,
    ip: Option<&str>,
    user_agent: Option<&str>,
) -> AppResult<NewSession> {
    let token = crypto::random_token::<32>();
    let csrf_token = crypto::random_token::<32>();
    let now = db::now();
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, csrf_token, created_at, expires_at, ip, user_agent)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(crypto::sha256(token.as_bytes()))
    .bind(user_id)
    .bind(&csrf_token)
    .bind(now)
    .bind(now + ttl_secs(state))
    .bind(ip)
    .bind(user_agent.map(|u| u.chars().take(256).collect::<String>()))
    .execute(&state.db)
    .await?;
    Ok(NewSession { token, csrf_token })
}

/// Finds the live session for a cookie token, extending it if needed.
///
/// # Errors
///
/// Database failures.
pub async fn lookup(state: &AppState, token: &str) -> AppResult<Option<Session>> {
    if token.len() > 128 {
        return Ok(None);
    }
    let hash = crypto::sha256(token.as_bytes());
    let now = db::now();
    let row: Option<(i64, String, i64)> = sqlx::query_as(
        "SELECT user_id, csrf_token, expires_at FROM sessions WHERE token_hash = ? AND expires_at > ?",
    )
    .bind(&hash)
    .bind(now)
    .fetch_optional(&state.db)
    .await?;
    let Some((user_id, csrf_token, mut expires_at)) = row else {
        return Ok(None);
    };
    let ttl = ttl_secs(state);
    if expires_at - now < ttl / 2 {
        expires_at = now + ttl;
        sqlx::query("UPDATE sessions SET expires_at = ? WHERE token_hash = ?")
            .bind(expires_at)
            .bind(&hash)
            .execute(&state.db)
            .await?;
    }
    Ok(Some(Session {
        token_hash: hash,
        user_id,
        csrf_token,
        expires_at,
    }))
}

/// Deletes one session.
///
/// # Errors
///
/// Database failures.
pub async fn delete(state: &AppState, token_hash: &[u8]) -> AppResult<()> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(token_hash)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Deletes every session of a user (after a password change, for example).
///
/// # Errors
///
/// Database failures.
pub async fn delete_for_user(state: &AppState, user_id: i64) -> AppResult<()> {
    sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(user_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Removes expired sessions. Called periodically.
///
/// # Errors
///
/// Database failures.
pub async fn purge_expired(state: &AppState) -> AppResult<u64> {
    let r = sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(db::now())
        .execute(&state.db)
        .await?;
    Ok(r.rows_affected())
}

/// Builds the session cookie (`HttpOnly`, `SameSite=Lax`, `Secure` per config).
pub fn cookie(state: &AppState, token: String) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, token))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(state.config.cookies_secure())
        .max_age(time::Duration::seconds(ttl_secs(state)))
        .build()
}

/// Builds a cookie that clears the session cookie.
pub fn clear_cookie(state: &AppState) -> Cookie<'static> {
    let mut c = cookie(state, String::new());
    c.make_removal();
    c
}
