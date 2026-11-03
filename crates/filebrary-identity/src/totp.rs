//! Time-based one-time passwords (RFC 6238) as a second login factor.
//!
//! # Flow
//!
//! 1. [`begin_enrollment`] creates a fresh secret for the user and returns it
//!    with an `otpauth://` URI for a QR code. 2FA is not active yet.
//! 2. [`confirm_enrollment`] checks one code from the user's app, switches
//!    2FA on and returns ten single-use recovery codes (shown once).
//! 3. At login, after the password is accepted, [`verify_login`] checks a
//!    6-digit code or a recovery code.
//!
//! # Security
//!
//! * Codes are 6 digits over HMAC-SHA1 with a 30 second step; one step of
//!   clock drift either way is accepted.
//! * A code is accepted once: the last accepted step is stored and anything at
//!   or before it is refused, so a code seen over someone's shoulder cannot be
//!   reused within its window.
//! * Secrets are encrypted at rest with a key derived from the server secret
//!   (HMAC-SHA256 used as a stream cipher with a random 16-byte nonce per
//!   secret), so a copy of the database alone does not reveal them.
//! * Recovery codes are stored as SHA-256 hashes (they carry 60 random bits,
//!   so a slow hash is not needed) and are marked used on first use.

use hmac::{Hmac, KeyInit, Mac};
use sqlx::SqlitePool;
use subtle::ConstantTimeEq;

use crate::{Error, Result, base32, now, random_bytes, sha256};

/// Number of digits in a code.
pub const DIGITS: u32 = 6;
/// Length of one time step in seconds.
pub const STEP_SECS: u64 = 30;
/// Steps of clock drift accepted on either side of the current one.
pub const SKEW_STEPS: u64 = 1;
/// Secret length in bytes (160 bits, as RFC 4226 recommends).
pub const SECRET_LEN: usize = 20;
/// Number of recovery codes issued when 2FA is switched on.
pub const RECOVERY_CODES: usize = 10;

/// Computes the HOTP value (RFC 4226) of `secret` at `counter`.
pub fn hotp(secret: &[u8], counter: u64) -> u32 {
    let mut mac = <Hmac<sha1::Sha1> as KeyInit>::new_from_slice(secret)
        .expect("HMAC accepts keys of any length");
    mac.update(&counter.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let off = usize::from(h[h.len() - 1] & 0x0f);
    let bin = (u32::from(h[off]) & 0x7f) << 24
        | u32::from(h[off + 1]) << 16
        | u32::from(h[off + 2]) << 8
        | u32::from(h[off + 3]);
    bin % 10u32.pow(DIGITS)
}

/// The code an authenticator shows for `secret` at Unix time `unix`.
pub fn code_at(secret: &[u8], unix: u64) -> String {
    format!(
        "{:0width$}",
        hotp(secret, unix / STEP_SECS),
        width = DIGITS as usize
    )
}

/// Checks `code` against `secret` at Unix time `unix`.
///
/// Returns the time step that matched, or `None`. Steps at or before
/// `last_step` are never accepted (replay protection). Spaces in the code are
/// ignored; anything but 6 digits is refused.
pub fn check(secret: &[u8], code: &str, unix: u64, last_step: Option<u64>) -> Option<u64> {
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let now_step = unix / STEP_SECS;
    let mut found = None;
    // Check every candidate step without returning early, so timing does not
    // reveal which step matched.
    for step in now_step.saturating_sub(SKEW_STEPS)..=now_step + SKEW_STEPS {
        let want = format!("{:0width$}", hotp(secret, step), width = DIGITS as usize);
        let ok: bool = want.as_bytes().ct_eq(code.as_bytes()).into();
        if ok && last_step.is_none_or(|l| step > l) && found.is_none() {
            found = Some(step);
        }
    }
    found
}

/// Builds the `otpauth://totp/...` URI that authenticator apps read from a QR code.
pub fn provisioning_uri(issuer: &str, account: &str, secret: &[u8]) -> String {
    use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
    let enc = |s: &str| utf8_percent_encode(s, NON_ALPHANUMERIC).to_string();
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECS}",
        enc(issuer),
        enc(account),
        base32::encode(secret),
        enc(issuer),
    )
}

/// Generates recovery codes of the form `abcd-efgh-ijkl` (60 random bits each).
pub fn generate_recovery_codes(n: usize) -> Vec<String> {
    (0..n)
        .map(|_| {
            let raw = base32::encode(&random_bytes::<8>()).to_ascii_lowercase();
            format!("{}-{}-{}", &raw[0..4], &raw[4..8], &raw[8..12])
        })
        .collect()
}

/// Normalises a recovery code as typed (case, spaces, dashes) before hashing.
fn normalise_recovery(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Encrypts or decrypts (the operation is its own inverse) a secret with the
/// server key and a per-secret nonce.
fn crypt(server_key: &[u8; 32], nonce: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = <Hmac<sha2::Sha256> as KeyInit>::new_from_slice(server_key)
        .expect("HMAC accepts keys of any length");
    mac.update(b"filebrary-totp-secret-v1");
    mac.update(nonce);
    let stream = mac.finalize().into_bytes();
    debug_assert!(data.len() <= stream.len());
    data.iter().zip(stream.iter()).map(|(d, k)| d ^ k).collect()
}

/// What the user needs to add Filebrary to an authenticator app.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Enrollment {
    /// The secret in base32, for typing in by hand.
    pub secret: String,
    /// The `otpauth://` URI, for a QR code.
    pub uri: String,
}

/// Status of a user's second factor.
#[derive(Clone, Copy, Debug, Default, serde::Serialize, PartialEq, Eq)]
pub struct Status {
    /// 2FA is switched on and required at login.
    pub enabled: bool,
    /// Unused recovery codes left.
    pub recovery_codes_left: i64,
}

/// Starts (or restarts) enrollment: stores a new, not yet active secret.
///
/// `issuer` is the name shown in the app (the branding name) and `account` is
/// usually the username.
///
/// # Errors
///
/// `Conflict` if 2FA is already on (switch it off first), or a database error.
pub async fn begin_enrollment(
    db: &SqlitePool,
    server_key: &[u8; 32],
    user_id: i64,
    issuer: &str,
    account: &str,
) -> Result<Enrollment> {
    if status(db, user_id).await?.enabled {
        return Err(Error::Conflict(
            "two-factor authentication is already on".into(),
        ));
    }
    let secret = random_bytes::<SECRET_LEN>();
    let nonce = random_bytes::<16>();
    sqlx::query(
        "INSERT INTO user_totp (user_id, secret_enc, nonce, enabled, last_step, created_at)
         VALUES (?, ?, ?, 0, NULL, ?)
         ON CONFLICT(user_id) DO UPDATE SET secret_enc = excluded.secret_enc,
             nonce = excluded.nonce, enabled = 0, last_step = NULL, created_at = excluded.created_at",
    )
    .bind(user_id)
    .bind(crypt(server_key, &nonce, &secret))
    .bind(nonce.as_slice())
    .bind(now())
    .execute(db)
    .await?;
    Ok(Enrollment {
        secret: base32::encode(&secret),
        uri: provisioning_uri(issuer, account, &secret),
    })
}

/// Finishes enrollment with a code from the app and switches 2FA on.
///
/// Returns the recovery codes in clear text. They are not stored in clear and
/// cannot be shown again.
///
/// # Errors
///
/// `NotFound` without a pending enrollment, `Conflict` if already on,
/// `InvalidCredentials` for a wrong code.
pub async fn confirm_enrollment(
    db: &SqlitePool,
    server_key: &[u8; 32],
    user_id: i64,
    code: &str,
) -> Result<Vec<String>> {
    let (secret, enabled, last_step) = load(db, server_key, user_id)
        .await?
        .ok_or(Error::NotFound)?;
    if enabled {
        return Err(Error::Conflict(
            "two-factor authentication is already on".into(),
        ));
    }
    let step = check(&secret, code, now() as u64, last_step).ok_or(Error::InvalidCredentials)?;
    let codes = generate_recovery_codes(RECOVERY_CODES);
    let mut tx = db.begin().await?;
    sqlx::query("UPDATE user_totp SET enabled = 1, last_step = ? WHERE user_id = ?")
        .bind(step as i64)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM totp_recovery_codes WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    for c in &codes {
        sqlx::query("INSERT INTO totp_recovery_codes (user_id, code_hash) VALUES (?, ?)")
            .bind(user_id)
            .bind(sha256(normalise_recovery(c).as_bytes()))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(codes)
}

/// Replaces the recovery codes with a fresh set (2FA must be on).
///
/// # Errors
///
/// `NotFound` if 2FA is off.
pub async fn regenerate_recovery_codes(db: &SqlitePool, user_id: i64) -> Result<Vec<String>> {
    if !status(db, user_id).await?.enabled {
        return Err(Error::NotFound);
    }
    let codes = generate_recovery_codes(RECOVERY_CODES);
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM totp_recovery_codes WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    for c in &codes {
        sqlx::query("INSERT INTO totp_recovery_codes (user_id, code_hash) VALUES (?, ?)")
            .bind(user_id)
            .bind(sha256(normalise_recovery(c).as_bytes()))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(codes)
}

/// Whether 2FA is on for the user, and how many recovery codes are left.
///
/// # Errors
///
/// Database errors.
pub async fn status(db: &SqlitePool, user_id: i64) -> Result<Status> {
    let enabled: Option<bool> =
        sqlx::query_scalar("SELECT enabled FROM user_totp WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(db)
            .await?;
    let left: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM totp_recovery_codes WHERE user_id = ? AND used_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?;
    Ok(Status {
        enabled: enabled.unwrap_or(false),
        recovery_codes_left: left,
    })
}

/// Checks a login code (6 digits) or a recovery code for a user with 2FA on.
///
/// A matching TOTP code advances the stored step atomically, so two requests
/// racing with the same code cannot both succeed. A matching recovery code is
/// marked used.
///
/// # Errors
///
/// `InvalidCredentials` if the code is wrong or reused, `NotFound` if 2FA is
/// off for the user.
pub async fn verify_login(
    db: &SqlitePool,
    server_key: &[u8; 32],
    user_id: i64,
    code: &str,
) -> Result<()> {
    let (secret, enabled, last_step) = load(db, server_key, user_id)
        .await?
        .ok_or(Error::NotFound)?;
    if !enabled {
        return Err(Error::NotFound);
    }
    if let Some(step) = check(&secret, code, now() as u64, last_step) {
        let done = sqlx::query(
            "UPDATE user_totp SET last_step = ?
             WHERE user_id = ? AND enabled = 1 AND (last_step IS NULL OR last_step < ?)",
        )
        .bind(step as i64)
        .bind(user_id)
        .bind(step as i64)
        .execute(db)
        .await?;
        return if done.rows_affected() == 1 {
            Ok(())
        } else {
            Err(Error::InvalidCredentials)
        };
    }
    let norm = normalise_recovery(code);
    if norm.len() != 12 {
        return Err(Error::InvalidCredentials);
    }
    let done = sqlx::query(
        "UPDATE totp_recovery_codes SET used_at = ?
         WHERE user_id = ? AND code_hash = ? AND used_at IS NULL",
    )
    .bind(now())
    .bind(user_id)
    .bind(sha256(norm.as_bytes()))
    .execute(db)
    .await?;
    if done.rows_affected() == 1 {
        tracing::info!(user = user_id, "recovery code used for login");
        Ok(())
    } else {
        Err(Error::InvalidCredentials)
    }
}

/// Switches 2FA off and deletes the secret and recovery codes.
///
/// # Errors
///
/// Database errors.
pub async fn disable(db: &SqlitePool, user_id: i64) -> Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM user_totp WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM totp_recovery_codes WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// `secret_enc, nonce, enabled, last_step`.
type TotpRow = (Vec<u8>, Vec<u8>, bool, Option<i64>);

async fn load(
    db: &SqlitePool,
    server_key: &[u8; 32],
    user_id: i64,
) -> Result<Option<(Vec<u8>, bool, Option<u64>)>> {
    let row: Option<TotpRow> = sqlx::query_as(
        "SELECT secret_enc, nonce, enabled, last_step FROM user_totp WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(enc, nonce, enabled, last)| {
        (
            crypt(server_key, &nonce, &enc),
            enabled,
            last.map(|l| l as u64),
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 6238 appendix B, SHA-1 secret, truncated to 6 digits.
    const RFC_SECRET: &[u8] = b"12345678901234567890";

    #[test]
    fn rfc6238_vectors() {
        let cases = [
            (59, "287082"),
            (1_111_111_109, "081804"),
            (1_111_111_111, "050471"),
            (1_234_567_890, "005924"),
            (2_000_000_000, "279037"),
            (20_000_000_000, "353130"),
        ];
        for (t, want) in cases {
            assert_eq!(code_at(RFC_SECRET, t), want, "t={t}");
        }
    }

    #[test]
    fn rfc4226_vectors() {
        let want = [755_224, 287_082, 359_152, 969_429, 338_314];
        for (i, w) in want.iter().enumerate() {
            assert_eq!(hotp(RFC_SECRET, i as u64), *w);
        }
    }

    #[test]
    fn check_accepts_skew_and_refuses_replay() {
        let t = 1_000_000_020;
        let step = t / STEP_SECS;
        let code = code_at(RFC_SECRET, t);
        assert_eq!(check(RFC_SECRET, &code, t, None), Some(step));
        assert_eq!(check(RFC_SECRET, &code, t + STEP_SECS, None), Some(step));
        assert_eq!(check(RFC_SECRET, &code, t - STEP_SECS, None), Some(step));
        assert_eq!(check(RFC_SECRET, &code, t + 2 * STEP_SECS, None), None);
        assert_eq!(check(RFC_SECRET, &code, t, Some(step)), None);
        assert_eq!(check(RFC_SECRET, &code, t, Some(step - 1)), Some(step));
        let spaced = format!("{} {}", &code[..3], &code[3..]);
        assert_eq!(check(RFC_SECRET, &spaced, t, None), Some(step));
        assert_eq!(check(RFC_SECRET, "12345", t, None), None);
        assert_eq!(check(RFC_SECRET, "abcdef", t, None), None);
    }

    #[test]
    fn crypt_round_trips_and_depends_on_nonce() {
        let key = [7u8; 32];
        let secret = [1u8; SECRET_LEN];
        let a = crypt(&key, b"nonce-a", &secret);
        let b = crypt(&key, b"nonce-b", &secret);
        assert_ne!(a, b);
        assert_ne!(a.as_slice(), secret.as_slice());
        assert_eq!(crypt(&key, b"nonce-a", &a), secret);
    }

    #[test]
    fn uri_and_recovery_format() {
        let uri = provisioning_uri("File brary", "bob@x", RFC_SECRET);
        assert!(uri.starts_with("otpauth://totp/File%20brary:bob%40x?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=File%20brary"));
        let codes = generate_recovery_codes(3);
        assert_eq!(codes.len(), 3);
        for c in &codes {
            assert_eq!(c.len(), 14);
            assert_eq!(normalise_recovery(&c.to_uppercase()).len(), 12);
        }
    }
}
