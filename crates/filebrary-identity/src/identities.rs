//! Links between external accounts and local users.
//!
//! An external account is identified by its provider kind (`oidc` or `ldap`),
//! an issuer (the OIDC issuer URL, or the LDAP server URL) and a subject (the
//! OIDC `sub` claim, or the user's LDAP DN). Linking by these, never by
//! username or e-mail, stops someone who can choose their name at one provider
//! from taking over a local account with the same name.

use sqlx::SqlitePool;

use crate::{Result, now};

/// Kind of external provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    /// OpenID Connect.
    Oidc,
    /// LDAP directory.
    Ldap,
}

impl Provider {
    /// The name stored in the database and in `users.auth_source`.
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Oidc => "oidc",
            Provider::Ldap => "ldap",
        }
    }
}

/// Finds the local user linked to an external account.
///
/// # Errors
///
/// Database errors.
pub async fn find(
    db: &SqlitePool,
    provider: Provider,
    issuer: &str,
    subject: &str,
) -> Result<Option<i64>> {
    let id = sqlx::query_scalar(
        "SELECT user_id FROM external_identities WHERE provider = ? AND issuer = ? AND subject = ?",
    )
    .bind(provider.as_str())
    .bind(issuer)
    .bind(subject)
    .fetch_optional(db)
    .await?;
    if id.is_some() {
        sqlx::query(
            "UPDATE external_identities SET last_login_at = ?
             WHERE provider = ? AND issuer = ? AND subject = ?",
        )
        .bind(now())
        .bind(provider.as_str())
        .bind(issuer)
        .bind(subject)
        .execute(db)
        .await?;
    }
    Ok(id)
}

/// Links an external account to a local user (replacing any earlier link of
/// that account).
///
/// # Errors
///
/// Database errors.
pub async fn link(
    db: &SqlitePool,
    provider: Provider,
    issuer: &str,
    subject: &str,
    user_id: i64,
) -> Result<()> {
    let t = now();
    sqlx::query(
        "INSERT INTO external_identities (provider, issuer, subject, user_id, created_at, last_login_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(provider, issuer, subject) DO UPDATE SET user_id = excluded.user_id",
    )
    .bind(provider.as_str())
    .bind(issuer)
    .bind(subject)
    .bind(user_id)
    .bind(t)
    .bind(t)
    .execute(db)
    .await?;
    Ok(())
}

/// Removes every link for a user (when the user is deleted the foreign key
/// does this too).
///
/// # Errors
///
/// Database errors.
pub async fn unlink_user(db: &SqlitePool, user_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM external_identities WHERE user_id = ?")
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}
