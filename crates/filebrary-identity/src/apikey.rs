//! Long-lived API keys with scopes.
//!
//! A key looks like `fbk_<id>_<secret>`: `fbk_` makes keys easy to spot in
//! logs and secret scanners, `<id>` (12 hex characters) is the public lookup
//! key, and `<secret>` is 32 random bytes in lower-case base32. Only the
//! SHA-256 of the secret is stored; the full key is shown once, at creation.
//!
//! Clients send it as `Authorization: Bearer fbk_...`. A request made with a
//! key acts as the key's owner, limited to the key's scopes *and* the owner's
//! own permissions: a key can never do more than its owner.

use std::fmt;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use subtle::ConstantTimeEq;

use crate::{Error, Result, base32, now, random_bytes, sha256};

/// Prefix of every key.
pub const PREFIX: &str = "fbk_";
/// Most keys one user may hold.
pub const MAX_KEYS_PER_USER: i64 = 50;
/// `last_used_at` is written at most this often per key, to spare the database.
const TOUCH_INTERVAL_SECS: i64 = 60;

/// What a key may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Scope {
    /// List folders, read metadata, download and preview (`view` + `download`).
    #[serde(rename = "files:read")]
    FilesRead,
    /// Upload, create folders, rename, move, copy and overwrite (`upload` + `edit`).
    #[serde(rename = "files:write")]
    FilesWrite,
    /// Delete files and folders (`delete`).
    #[serde(rename = "files:delete")]
    FilesDelete,
    /// Create, list and revoke share links (`share`).
    #[serde(rename = "shares")]
    Shares,
    /// Administration endpoints (users, settings). Only administrators can
    /// create keys with this scope.
    #[serde(rename = "admin")]
    Admin,
}

impl Scope {
    /// Every scope, in display order.
    pub const ALL: [Scope; 5] = [
        Scope::FilesRead,
        Scope::FilesWrite,
        Scope::FilesDelete,
        Scope::Shares,
        Scope::Admin,
    ];

    /// The scope's name as used in the API and database.
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::FilesRead => "files:read",
            Scope::FilesWrite => "files:write",
            Scope::FilesDelete => "files:delete",
            Scope::Shares => "shares",
            Scope::Admin => "admin",
        }
    }

    /// Parses a scope name.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|sc| sc.as_str() == s)
    }

    /// The file permission names (as in the server's `Perms`) this scope allows.
    pub fn permissions(self) -> &'static [&'static str] {
        match self {
            Scope::FilesRead => &["view", "download"],
            Scope::FilesWrite => &["view", "upload", "edit"],
            Scope::FilesDelete => &["view", "delete"],
            Scope::Shares => &["view", "share"],
            Scope::Admin => &[],
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parses and de-duplicates scope names.
///
/// # Errors
///
/// `InvalidInput` on an unknown name or an empty list.
pub fn parse_scopes<S: AsRef<str>>(names: &[S]) -> Result<Vec<Scope>> {
    let mut out = Vec::new();
    for n in names {
        let sc = Scope::parse(n.as_ref())
            .ok_or_else(|| Error::invalid(format!("unknown scope {:?}", n.as_ref())))?;
        if !out.contains(&sc) {
            out.push(sc);
        }
    }
    if out.is_empty() {
        return Err(Error::invalid("a key needs at least one scope"));
    }
    out.sort();
    Ok(out)
}

/// The union of file permission names that `scopes` allow.
pub fn allowed_permissions(scopes: &[Scope]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for sc in scopes {
        for p in sc.permissions() {
            if !out.contains(p) {
                out.push(p);
            }
        }
    }
    out
}

/// A parsed key as sent by a client.
#[derive(Clone, PartialEq, Eq)]
pub struct Token {
    /// The public id.
    pub id: String,
    secret: String,
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Token")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Token {
    /// Parses `fbk_<id>_<secret>`. Returns `None` for anything else, including
    /// strings that are not Filebrary keys at all.
    pub fn parse(s: &str) -> Option<Self> {
        let rest = s.strip_prefix(PREFIX)?;
        let (id, secret) = rest.split_once('_')?;
        let id_ok = id.len() == 12 && id.bytes().all(|b| b.is_ascii_hexdigit());
        let secret_ok = secret.len() == 52
            && secret
                .bytes()
                .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b));
        (id_ok && secret_ok).then(|| Self {
            id: id.to_ascii_lowercase(),
            secret: secret.to_owned(),
        })
    }

    fn generate() -> (Self, String) {
        let id = hex::encode(random_bytes::<6>());
        let secret = base32::encode(&random_bytes::<32>()).to_ascii_lowercase();
        let full = format!("{PREFIX}{id}_{secret}");
        (Self { id, secret }, full)
    }
}

/// A key as listed to its owner (never includes the secret).
#[derive(Clone, Debug, Serialize, sqlx::FromRow)]
pub struct KeyInfo {
    /// Public id (also the first part of the key after `fbk_`).
    pub id: String,
    /// Owner.
    pub user_id: i64,
    /// Label chosen by the owner.
    pub name: String,
    /// Space-separated scope names.
    #[serde(serialize_with = "split_scopes")]
    pub scopes: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds; `None` means the key does not expire.
    pub expires_at: Option<i64>,
    /// Unix seconds, accurate to about a minute.
    pub last_used_at: Option<i64>,
}

fn split_scopes<S: serde::Serializer>(s: &str, ser: S) -> std::result::Result<S::Ok, S::Error> {
    ser.collect_seq(s.split(' ').filter(|x| !x.is_empty()))
}

impl KeyInfo {
    /// The key's scopes (unknown names, from a newer version, are dropped).
    pub fn scope_list(&self) -> Vec<Scope> {
        self.scopes.split(' ').filter_map(Scope::parse).collect()
    }
}

/// A newly created key: the full secret is in `token` and is never shown again.
#[derive(Clone, Debug, Serialize)]
pub struct NewKey {
    /// The key's details.
    #[serde(flatten)]
    pub info: KeyInfo,
    /// The full key to hand to the client.
    pub token: String,
}

/// Creates a key for `user_id`.
///
/// `owner_is_admin` gates the `admin` scope. `expires_at` must be in the future.
///
/// # Errors
///
/// `InvalidInput` for a bad name, scope or expiry; `Conflict` past
/// [`MAX_KEYS_PER_USER`]; `InvalidInput` for `admin` on a non-admin.
pub async fn create(
    db: &SqlitePool,
    user_id: i64,
    owner_is_admin: bool,
    name: &str,
    scopes: &[Scope],
    expires_at: Option<i64>,
) -> Result<NewKey> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        return Err(Error::invalid(
            "key name must be 1-100 printable characters",
        ));
    }
    if scopes.is_empty() {
        return Err(Error::invalid("a key needs at least one scope"));
    }
    if scopes.contains(&Scope::Admin) && !owner_is_admin {
        return Err(Error::invalid("only administrators can create admin keys"));
    }
    let created_at = now();
    if expires_at.is_some_and(|e| e <= created_at) {
        return Err(Error::invalid("expiry must be in the future"));
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_keys WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(db)
        .await?;
    if count >= MAX_KEYS_PER_USER {
        return Err(Error::Conflict(format!(
            "at most {MAX_KEYS_PER_USER} API keys per user"
        )));
    }
    let mut scopes = scopes.to_vec();
    scopes.sort();
    scopes.dedup();
    let scope_str = scopes
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let (tok, full) = Token::generate();
    sqlx::query(
        "INSERT INTO api_keys (id, user_id, name, secret_hash, scopes, created_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&tok.id)
    .bind(user_id)
    .bind(name)
    .bind(sha256(tok.secret.as_bytes()))
    .bind(&scope_str)
    .bind(created_at)
    .bind(expires_at)
    .execute(db)
    .await?;
    Ok(NewKey {
        info: KeyInfo {
            id: tok.id,
            user_id,
            name: name.to_owned(),
            scopes: scope_str,
            created_at,
            expires_at,
            last_used_at: None,
        },
        token: full,
    })
}

/// Lists a user's keys, newest first.
///
/// # Errors
///
/// Database errors.
pub async fn list(db: &SqlitePool, user_id: i64) -> Result<Vec<KeyInfo>> {
    Ok(sqlx::query_as(
        "SELECT id, user_id, name, scopes, created_at, expires_at, last_used_at
         FROM api_keys WHERE user_id = ? ORDER BY created_at DESC, id",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?)
}

/// Revokes (deletes) one of a user's keys. Pass `user_id = None` to let an
/// administrator revoke anyone's key.
///
/// # Errors
///
/// `NotFound` if no such key belongs to the user.
pub async fn revoke(db: &SqlitePool, user_id: Option<i64>, id: &str) -> Result<()> {
    let done = sqlx::query("DELETE FROM api_keys WHERE id = ? AND (? IS NULL OR user_id = ?)")
        .bind(id)
        .bind(user_id)
        .bind(user_id)
        .execute(db)
        .await?;
    if done.rows_affected() == 0 {
        return Err(Error::NotFound);
    }
    Ok(())
}

/// `user_id, secret_hash, scopes, expires_at, last_used_at`.
type KeyRow = (i64, Vec<u8>, String, Option<i64>, Option<i64>);

/// Resolves a bearer token to its owner and scopes.
///
/// Returns `Ok(None)` if `token` is not a Filebrary key at all (so the caller
/// can try other schemes), and `InvalidCredentials` for a well-formed key that
/// is unknown, expired or has the wrong secret.
///
/// # Errors
///
/// `InvalidCredentials` as above, or database errors.
pub async fn resolve(db: &SqlitePool, token: &str) -> Result<Option<(i64, Vec<Scope>)>> {
    let Some(tok) = Token::parse(token) else {
        return Ok(None);
    };
    let row: Option<KeyRow> = sqlx::query_as(
        "SELECT user_id, secret_hash, scopes, expires_at, last_used_at FROM api_keys WHERE id = ?",
    )
    .bind(&tok.id)
    .fetch_optional(db)
    .await?;
    let Some((user_id, hash, scopes, expires_at, last_used)) = row else {
        return Err(Error::InvalidCredentials);
    };
    let ok: bool = sha256(tok.secret.as_bytes()).ct_eq(&hash).into();
    let t = now();
    if !ok || expires_at.is_some_and(|e| e <= t) {
        return Err(Error::InvalidCredentials);
    }
    if last_used.is_none_or(|l| t - l >= TOUCH_INTERVAL_SECS) {
        sqlx::query("UPDATE api_keys SET last_used_at = ? WHERE id = ?")
            .bind(t)
            .bind(&tok.id)
            .execute(db)
            .await?;
    }
    Ok(Some((
        user_id,
        scopes.split(' ').filter_map(Scope::parse).collect(),
    )))
}

/// Deletes keys that expired more than `grace_secs` ago. Returns how many.
///
/// # Errors
///
/// Database errors.
pub async fn purge_expired(db: &SqlitePool, grace_secs: i64) -> Result<u64> {
    let done = sqlx::query("DELETE FROM api_keys WHERE expires_at IS NOT NULL AND expires_at < ?")
        .bind(now() - grace_secs)
        .execute(db)
        .await?;
    Ok(done.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_round_trip_and_rejects() {
        let (tok, full) = Token::generate();
        assert!(full.starts_with("fbk_"));
        assert_eq!(Token::parse(&full).unwrap(), tok);
        assert!(Token::parse("fbk_abc_def").is_none());
        assert!(Token::parse("Bearer something").is_none());
        let upper = full.to_uppercase().replacen("FBK_", "fbk_", 1);
        assert!(Token::parse(&upper).is_none());
        assert!(!format!("{tok:?}").contains(&tok.secret));
    }

    #[test]
    fn scopes_parse_and_map() {
        let s = parse_scopes(&["files:write", "files:read", "files:read"]).unwrap();
        assert_eq!(s, [Scope::FilesRead, Scope::FilesWrite]);
        assert_eq!(
            allowed_permissions(&s),
            ["view", "download", "upload", "edit"]
        );
        assert!(parse_scopes(&["root"]).is_err());
        assert!(parse_scopes::<&str>(&[]).is_err());
        assert_eq!(
            serde_json::to_string(&Scope::FilesDelete).unwrap(),
            "\"files:delete\""
        );
    }
}
