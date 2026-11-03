//! Password hashing with Argon2id (OWASP parameters: 19 MiB, 2 passes, 1 lane).
//!
//! Hashing is CPU- and memory-heavy, so it runs on the blocking pool.
//! Hashes are PHC strings, so parameters travel with each hash and can be
//! raised later without breaking stored passwords. Other modules (share-link
//! passwords, for example) reuse these helpers.

use std::sync::OnceLock;

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};

use crate::error::{AppError, AppResult};

/// Hashes a password into a PHC string (`$argon2id$v=19$...`).
///
/// # Errors
///
/// Fails only if the hasher fails, which should not happen.
pub async fn hash(password: String) -> AppResult<String> {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|h| h.to_string())
            .map_err(|e| AppError::Internal(anyhow::anyhow!("argon2: {e}")))
    })
    .await
    .map_err(|e| AppError::Internal(e.into()))?
}

/// Verifies a password against a stored PHC string. Malformed hashes never verify.
pub async fn verify(password: String, phc: String) -> bool {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&phc)
            .map(|h| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &h)
                    .is_ok()
            })
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

/// Shortest password accepted by [`validate`], in characters.
pub const MIN_LEN: usize = 8;
/// Longest password accepted by [`validate`], in bytes (bounds hashing cost).
pub const MAX_LEN: usize = 1024;

/// Checks a new password against the length policy.
///
/// # Errors
///
/// `BadRequest` when shorter than [`MIN_LEN`] characters or longer than
/// [`MAX_LEN`] bytes.
pub fn validate(password: &str) -> AppResult<()> {
    if password.chars().count() < MIN_LEN {
        return Err(AppError::bad_request(format!(
            "password must be at least {MIN_LEN} characters"
        )));
    }
    if password.len() > MAX_LEN {
        return Err(AppError::bad_request(format!(
            "password must be at most {MAX_LEN} bytes"
        )));
    }
    Ok(())
}

/// A real hash of a random password. Logins naming an unknown user are
/// verified against it so they take as long as logins for real users.
pub(crate) async fn dummy_hash() -> String {
    static DUMMY: OnceLock<String> = OnceLock::new();
    if let Some(h) = DUMMY.get() {
        return h.clone();
    }
    let h = hash(crate::crypto::random_token::<16>())
        .await
        .unwrap_or_default();
    DUMMY.get_or_init(|| h).clone()
}

#[cfg(test)]
mod tests {
    #[test]
    fn validate_length() {
        assert!(super::validate("short").is_err());
        assert!(super::validate("long enough").is_ok());
        assert!(super::validate(&"a".repeat(super::MAX_LEN + 1)).is_err());
    }

    #[tokio::test]
    async fn dummy_hash_is_a_real_hash() {
        let h = super::dummy_hash().await;
        assert!(h.starts_with("$argon2id$"));
        assert_eq!(h, super::dummy_hash().await);
    }

    #[tokio::test]
    async fn hash_and_verify() {
        let h = super::hash("correct horse".into()).await.unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(super::verify("correct horse".into(), h.clone()).await);
        assert!(!super::verify("wrong".into(), h).await);
        assert!(!super::verify("x".into(), "garbage".into()).await);
    }
}
