//! Password hashing with Argon2id (OWASP parameters: 19 MiB, 2 passes, 1 lane).
//!
//! Hashing is CPU- and memory-heavy, so it runs on the blocking pool.

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

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn hash_and_verify() {
        let h = super::hash("correct horse".into()).await.unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(super::verify("correct horse".into(), h.clone()).await);
        assert!(!super::verify("wrong".into(), h).await);
        assert!(!super::verify("x".into(), "garbage".into()).await);
    }
}
