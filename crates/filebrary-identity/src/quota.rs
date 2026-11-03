//! Per-user storage quotas.
//!
//! Each user may have a limit in bytes. Usage is a running total that the
//! server adjusts as files are written ([`reserve`]) and removed
//! ([`release`]), and resets from a real scan of the user's folders now and
//! then ([`set_used`]), which corrects any drift (files changed outside
//! Filebrary, overwrites, failed uploads).
//!
//! [`reserve`] is a single conditional `UPDATE`, so two uploads racing for the
//! last free bytes cannot both get them.

use serde::Serialize;
use sqlx::SqlitePool;

use crate::{Error, Result, now};

/// A user's quota and usage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Quota {
    /// Limit in bytes; `None` means unlimited.
    pub limit: Option<i64>,
    /// Bytes in use.
    pub used: i64,
}

impl Quota {
    /// Bytes still free (`None` when unlimited).
    pub fn free(&self) -> Option<i64> {
        self.limit.map(|l| (l - self.used).max(0))
    }

    /// Whether `bytes` more would fit.
    pub fn allows(&self, bytes: i64) -> bool {
        self.limit
            .is_none_or(|l| self.used.saturating_add(bytes) <= l)
    }

    /// Usage as a fraction of the limit (0.0 when unlimited).
    pub fn fraction_used(&self) -> f64 {
        match self.limit {
            Some(l) if l > 0 => self.used as f64 / l as f64,
            Some(_) => 1.0,
            None => 0.0,
        }
    }
}

/// Reads a user's quota (unlimited with zero usage if never set).
///
/// # Errors
///
/// Database errors.
pub async fn get(db: &SqlitePool, user_id: i64) -> Result<Quota> {
    let row: Option<(Option<i64>, i64)> =
        sqlx::query_as("SELECT limit_bytes, used_bytes FROM user_quotas WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(db)
            .await?;
    Ok(row
        .map(|(limit, used)| Quota { limit, used })
        .unwrap_or_default())
}

/// Sets or clears (`None`) a user's limit. Usage is kept.
///
/// # Errors
///
/// `InvalidInput` for a negative limit.
pub async fn set_limit(db: &SqlitePool, user_id: i64, limit: Option<i64>) -> Result<()> {
    if limit.is_some_and(|l| l < 0) {
        return Err(Error::invalid("quota cannot be negative"));
    }
    sqlx::query(
        "INSERT INTO user_quotas (user_id, limit_bytes, used_bytes, updated_at) VALUES (?, ?, 0, ?)
         ON CONFLICT(user_id) DO UPDATE SET limit_bytes = excluded.limit_bytes,
             updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(limit)
    .bind(now())
    .execute(db)
    .await?;
    Ok(())
}

/// Atomically adds `bytes` to the user's usage if it fits under the limit.
///
/// Returns the quota after the reservation.
///
/// # Errors
///
/// `QuotaExceeded` if it does not fit, `InvalidInput` for negative `bytes`.
pub async fn reserve(db: &SqlitePool, user_id: i64, bytes: i64) -> Result<Quota> {
    if bytes < 0 {
        return Err(Error::invalid("cannot reserve a negative size"));
    }
    let t = now();
    sqlx::query(
        "INSERT INTO user_quotas (user_id, limit_bytes, used_bytes, updated_at)
         VALUES (?, NULL, 0, ?) ON CONFLICT(user_id) DO NOTHING",
    )
    .bind(user_id)
    .bind(t)
    .execute(db)
    .await?;
    let done = sqlx::query(
        "UPDATE user_quotas SET used_bytes = used_bytes + ?, updated_at = ?
         WHERE user_id = ? AND (limit_bytes IS NULL OR used_bytes + ? <= limit_bytes)",
    )
    .bind(bytes)
    .bind(t)
    .bind(user_id)
    .bind(bytes)
    .execute(db)
    .await?;
    let q = get(db, user_id).await?;
    if done.rows_affected() == 0 {
        return Err(Error::QuotaExceeded {
            limit: q.limit.unwrap_or(0),
            used: q.used,
            requested: bytes,
        });
    }
    Ok(q)
}

/// Subtracts `bytes` from the user's usage (never below zero).
///
/// # Errors
///
/// Database errors.
pub async fn release(db: &SqlitePool, user_id: i64, bytes: i64) -> Result<()> {
    sqlx::query(
        "UPDATE user_quotas SET used_bytes = MAX(used_bytes - ?, 0), updated_at = ? WHERE user_id = ?",
    )
    .bind(bytes.max(0))
    .bind(now())
    .bind(user_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Replaces the user's usage with a measured value (after a rescan).
///
/// # Errors
///
/// Database errors.
pub async fn set_used(db: &SqlitePool, user_id: i64, bytes: i64) -> Result<()> {
    sqlx::query(
        "INSERT INTO user_quotas (user_id, limit_bytes, used_bytes, updated_at) VALUES (?, NULL, ?, ?)
         ON CONFLICT(user_id) DO UPDATE SET used_bytes = excluded.used_bytes,
             updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(bytes.max(0))
    .bind(now())
    .execute(db)
    .await?;
    Ok(())
}

/// Users that have a limit, for the periodic rescan.
///
/// # Errors
///
/// Database errors.
pub async fn limited_users(db: &SqlitePool) -> Result<Vec<i64>> {
    Ok(
        sqlx::query_scalar("SELECT user_id FROM user_quotas WHERE limit_bytes IS NOT NULL")
            .fetch_all(db)
            .await?,
    )
}

/// Parses a human size such as `500M`, `10 GiB`, `1.5T` or `1048576`.
///
/// Units are binary (`K` = 1024). Returns `None` for anything else.
pub fn parse_size(s: &str) -> Option<i64> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().ok()?;
    let mult: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024f64.powi(2),
        "g" | "gb" | "gib" => 1024f64.powi(3),
        "t" | "tb" | "tib" => 1024f64.powi(4),
        "p" | "pb" | "pib" => 1024f64.powi(5),
        _ => return None,
    };
    let v = (n * mult).round();
    (v.is_finite() && v >= 0.0 && v <= i64::MAX as f64).then_some(v as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_math() {
        let q = Quota {
            limit: Some(100),
            used: 60,
        };
        assert!(q.allows(40));
        assert!(!q.allows(41));
        assert_eq!(q.free(), Some(40));
        assert!((q.fraction_used() - 0.6).abs() < 1e-9);
        let unlimited = Quota::default();
        assert!(unlimited.allows(i64::MAX));
        assert_eq!(unlimited.free(), None);
        let over = Quota {
            limit: Some(10),
            used: 20,
        };
        assert_eq!(over.free(), Some(0));
        assert!(!over.allows(0));
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_size("1048576"), Some(1_048_576));
        assert_eq!(parse_size("500M"), Some(500 * 1024 * 1024));
        assert_eq!(parse_size("10 GiB"), Some(10 * 1024 * 1024 * 1024));
        assert_eq!(parse_size("1.5k"), Some(1536));
        assert_eq!(parse_size("5 parsecs"), None);
        assert_eq!(parse_size(""), None);
    }
}
