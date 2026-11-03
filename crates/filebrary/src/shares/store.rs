//! Database access for share links.
//!
//! Every query lives here so the HTTP layer only deals with [`Share`] values.

use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use super::error::ShareError;
use crate::{crypto, storage::VPath};

pub use crate::db::now;

/// Random bytes in a share token; 18 bytes encode to 24 URL-safe characters.
const TOKEN_BYTES: usize = 18;
/// Length of an encoded share token.
const TOKEN_LEN: usize = 24;
/// How long an unlock grant stays valid after a correct password, in seconds.
pub const ACCESS_TTL_SECS: i64 = 12 * 60 * 60;

/// A share link as stored in the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    /// Database id, used by the management API.
    pub id: i64,
    /// Public token that appears in the share URL.
    pub token: String,
    /// User who created the share.
    pub owner_id: i64,
    /// Source the shared paths belong to.
    pub source: String,
    /// Paths (absolute within the source) covered by the share.
    pub paths: Vec<VPath>,
    /// argon2id hash of the share password, if one is set.
    pub password_hash: Option<String>,
    /// Unix time after which the share no longer works.
    pub expires_at: Option<i64>,
    /// Maximum number of downloads, if limited.
    pub max_downloads: Option<i64>,
    /// Downloads served so far.
    pub downloads: i64,
    /// Whether this is an upload-only drop link.
    pub upload_only: bool,
    /// Unix time the share was created.
    pub created_at: i64,
    /// Collection this share serves instead of `source`/`paths`.
    pub collection_id: Option<i64>,
}

impl Share {
    /// Whether the share has passed its expiry time at `now` (unix seconds).
    pub fn is_expired(&self, now: i64) -> bool {
        self.expires_at.is_some_and(|t| now >= t)
    }

    /// Whether the download limit has been used up.
    pub fn is_exhausted(&self) -> bool {
        self.max_downloads.is_some_and(|m| self.downloads >= m)
    }
}

/// Fields needed to create a share. Paths must already be normalised.
#[derive(Debug, Clone)]
pub struct NewShare {
    /// Creator of the share.
    pub owner_id: i64,
    /// Source id.
    pub source: String,
    /// Paths inside the source (at least one).
    pub paths: Vec<VPath>,
    /// Pre-computed argon2id hash, if a password is set.
    pub password_hash: Option<String>,
    /// Expiry as unix seconds.
    pub expires_at: Option<i64>,
    /// Download limit.
    pub max_downloads: Option<i64>,
    /// Upload-only drop link.
    pub upload_only: bool,
    /// Collection to serve (then `source` is empty and `paths` empty).
    pub collection_id: Option<i64>,
}

/// A partial update. `None` leaves a field unchanged; `Some(None)` clears it.
#[derive(Debug, Clone, Default)]
pub struct ShareUpdate {
    /// New password hash, or `Some(None)` to remove the password.
    pub password_hash: Option<Option<String>>,
    /// New expiry, or `Some(None)` to remove it.
    pub expires_at: Option<Option<i64>>,
    /// New download limit, or `Some(None)` to remove it.
    pub max_downloads: Option<Option<i64>>,
    /// Switch between normal and upload-only.
    pub upload_only: Option<bool>,
}

/// Generates an unguessable URL-safe token (144 bits of entropy).
pub fn new_token() -> String {
    crypto::random_token::<TOKEN_BYTES>()
}

/// Checks that a string looks like a token we issued, so junk never reaches SQL.
pub fn is_valid_token(token: &str) -> bool {
    token.len() == TOKEN_LEN
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// SHA-256 hex digest, used to store access grants without keeping the secret.
fn sha256_hex(s: &str) -> String {
    hex::encode(crypto::sha256(s.as_bytes()))
}

fn from_row(row: &SqliteRow) -> Result<Share, ShareError> {
    let paths_json: String = row.try_get("paths")?;
    let paths: Vec<VPath> = serde_json::from_str(&paths_json)
        .map_err(|e| ShareError::internal(format!("corrupt share paths: {e}")))?;
    Ok(Share {
        id: row.try_get("id")?,
        token: row.try_get("token")?,
        owner_id: row.try_get("owner_id")?,
        source: row.try_get("source")?,
        paths,
        password_hash: row.try_get("password_hash")?,
        expires_at: row.try_get("expires_at")?,
        max_downloads: row.try_get("max_downloads")?,
        downloads: row.try_get("downloads")?,
        upload_only: row.try_get::<i64, _>("upload_only")? != 0,
        created_at: row.try_get("created_at")?,
        collection_id: row.try_get("collection_id")?,
    })
}

/// Column list shared by every `SELECT`/`RETURNING`, as a macro so queries stay `&'static str`.
macro_rules! cols {
    () => {
        "id, token, owner_id, source, paths, password_hash, expires_at, \
         max_downloads, downloads, upload_only, created_at, collection_id"
    };
}

/// Repository of share links backed by SQLite.
#[derive(Debug, Clone)]
pub struct ShareStore {
    pool: SqlitePool,
}

impl ShareStore {
    /// Wraps a pool. The shares migration must already have run.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Inserts a new share with a fresh token and returns it.
    pub async fn create(&self, new: NewShare) -> Result<Share, ShareError> {
        let paths =
            serde_json::to_string(&new.paths).map_err(|e| ShareError::internal(e.to_string()))?;
        let created_at = now();
        // A token collision is astronomically unlikely, but retry rather than fail.
        for _ in 0..3 {
            let token = new_token();
            let res = sqlx::query(concat!(
                "INSERT INTO shares (token, owner_id, source, paths, password_hash, expires_at, \
                 max_downloads, upload_only, created_at, collection_id) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
                 RETURNING ",
                cols!()
            ))
            .bind(&token)
            .bind(new.owner_id)
            .bind(&new.source)
            .bind(&paths)
            .bind(&new.password_hash)
            .bind(new.expires_at)
            .bind(new.max_downloads)
            .bind(new.upload_only as i64)
            .bind(created_at)
            .bind(new.collection_id)
            .fetch_one(&self.pool)
            .await;
            match res {
                Ok(row) => return from_row(&row),
                Err(sqlx::Error::Database(e)) if e.is_unique_violation() => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(ShareError::internal("could not allocate a unique token"))
    }

    /// Looks a share up by id.
    pub async fn get(&self, id: i64) -> Result<Option<Share>, ShareError> {
        let row = sqlx::query(concat!("SELECT ", cols!(), " FROM shares WHERE id = ?"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(from_row).transpose()
    }

    /// Looks a share up by its public token. Malformed tokens return `None`.
    pub async fn get_by_token(&self, token: &str) -> Result<Option<Share>, ShareError> {
        if !is_valid_token(token) {
            return Ok(None);
        }
        let row = sqlx::query(concat!("SELECT ", cols!(), " FROM shares WHERE token = ?"))
            .bind(token)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(from_row).transpose()
    }

    /// Lists shares, newest first. `owner` restricts to one user's shares.
    pub async fn list(&self, owner: Option<i64>) -> Result<Vec<Share>, ShareError> {
        let rows = match owner {
            Some(o) => {
                sqlx::query(concat!(
                    "SELECT ",
                    cols!(),
                    " FROM shares WHERE owner_id = ? ORDER BY id DESC"
                ))
                .bind(o)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(concat!("SELECT ", cols!(), " FROM shares ORDER BY id DESC"))
                    .fetch_all(&self.pool)
                    .await?
            }
        };
        rows.iter().map(from_row).collect()
    }

    /// Applies a partial update and returns the new row. Changing the password
    /// revokes every unlock grant issued so far.
    pub async fn update(&self, id: i64, upd: ShareUpdate) -> Result<Option<Share>, ShareError> {
        let mut tx = self.pool.begin().await?;
        if let Some(ph) = &upd.password_hash {
            sqlx::query("UPDATE shares SET password_hash = ? WHERE id = ?")
                .bind(ph)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM share_access WHERE share_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(e) = upd.expires_at {
            sqlx::query("UPDATE shares SET expires_at = ? WHERE id = ?")
                .bind(e)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(m) = upd.max_downloads {
            sqlx::query("UPDATE shares SET max_downloads = ? WHERE id = ?")
                .bind(m)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(u) = upd.upload_only {
            sqlx::query("UPDATE shares SET upload_only = ? WHERE id = ?")
                .bind(u as i64)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        self.get(id).await
    }

    /// Deletes a share (and its grants). Returns whether a row was removed.
    pub async fn delete(&self, id: i64) -> Result<bool, ShareError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM share_access WHERE share_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let n = sqlx::query("DELETE FROM shares WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(n > 0)
    }

    /// Atomically counts one download, refusing if the limit is already reached.
    /// Returns `false` when the share is exhausted (or gone).
    pub async fn record_download(&self, id: i64) -> Result<bool, ShareError> {
        let n = sqlx::query(
            "UPDATE shares SET downloads = downloads + 1 \
             WHERE id = ? AND (max_downloads IS NULL OR downloads < max_downloads)",
        )
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(n == 1)
    }

    /// Issues an unlock grant for a share and returns the secret to hand out.
    pub async fn grant_access(&self, share_id: i64) -> Result<String, ShareError> {
        let secret = crypto::random_token::<32>();
        let now = now();
        sqlx::query("DELETE FROM share_access WHERE expires_at <= ?")
            .bind(now)
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "INSERT INTO share_access (access_hash, share_id, expires_at) VALUES (?, ?, ?)",
        )
        .bind(sha256_hex(&secret))
        .bind(share_id)
        .bind(now + ACCESS_TTL_SECS)
        .execute(&self.pool)
        .await?;
        Ok(secret)
    }

    /// Checks an unlock grant for a share.
    pub async fn check_access(&self, share_id: i64, secret: &str) -> Result<bool, ShareError> {
        if secret.is_empty() || secret.len() > 128 {
            return Ok(false);
        }
        let row = sqlx::query(
            "SELECT 1 FROM share_access WHERE access_hash = ? AND share_id = ? AND expires_at > ?",
        )
        .bind(sha256_hex(secret))
        .bind(share_id)
        .bind(now())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_share(max: Option<i64>) -> NewShare {
        NewShare {
            owner_id: 1,
            source: "files".into(),
            paths: vec![VPath::parse("docs").unwrap()],
            password_hash: None,
            expires_at: None,
            max_downloads: max,
            upload_only: false,
            collection_id: None,
        }
    }

    #[test]
    fn tokens() {
        let t = new_token();
        assert!(is_valid_token(&t), "{t}");
        assert_ne!(t, new_token());
        assert!(!is_valid_token("short"));
        assert!(!is_valid_token("AAAAAAAAAAAAAAAAAAAAAAA'"));
    }

    #[tokio::test]
    async fn crud_and_limits() {
        let store = ShareStore::new(crate::db::open_memory().await.unwrap());
        let s = store.create(new_share(Some(2))).await.unwrap();
        assert_eq!(store.get_by_token(&s.token).await.unwrap().unwrap(), s);
        assert!(store.get_by_token("nope").await.unwrap().is_none());
        assert!(store.record_download(s.id).await.unwrap());
        assert!(store.record_download(s.id).await.unwrap());
        assert!(!store.record_download(s.id).await.unwrap());
        assert!(store.get(s.id).await.unwrap().unwrap().is_exhausted());

        let grant = store.grant_access(s.id).await.unwrap();
        assert!(store.check_access(s.id, &grant).await.unwrap());
        assert!(!store.check_access(s.id + 1, &grant).await.unwrap());
        assert!(!store.check_access(s.id, "wrong").await.unwrap());
        let upd = ShareUpdate {
            password_hash: Some(Some("$argon2id$x".into())),
            max_downloads: Some(None),
            ..Default::default()
        };
        let s2 = store.update(s.id, upd).await.unwrap().unwrap();
        assert_eq!(s2.max_downloads, None);
        assert!(
            !store.check_access(s.id, &grant).await.unwrap(),
            "grants revoked"
        );

        let other = store.create(new_share(None)).await.unwrap();
        assert_eq!(store.list(None).await.unwrap().len(), 2);
        assert_eq!(store.list(Some(1)).await.unwrap()[0].id, other.id);
        assert!(store.list(Some(9)).await.unwrap().is_empty());
        assert!(store.delete(s.id).await.unwrap());
        assert!(!store.delete(s.id).await.unwrap());
    }

    #[test]
    fn expiry() {
        let mut s = Share {
            id: 1,
            token: String::new(),
            owner_id: 1,
            source: String::new(),
            paths: vec![],
            password_hash: None,
            expires_at: Some(100),
            max_downloads: None,
            downloads: 0,
            upload_only: false,
            collection_id: None,
            created_at: 0,
        };
        assert!(!s.is_expired(99));
        assert!(s.is_expired(100));
        s.expires_at = None;
        assert!(!s.is_expired(i64::MAX));
    }
}
