//! Small cryptographic helpers: random tokens, hashing and MACs.

use std::path::Path;

use anyhow::Context;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Returns `n` random bytes from a cryptographically secure generator.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    rand::fill(&mut buf[..]);
    buf
}

/// Returns a URL-safe random token carrying `N` bytes of entropy.
pub fn random_token<const N: usize>() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<N>())
}

/// SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> Vec<u8> {
    Sha256::digest(data).to_vec()
}

/// HMAC-SHA-256 of `msg` under `key`, URL-safe base64 encoded.
pub fn hmac_b64(key: &[u8], msg: &[u8]) -> String {
    // HMAC accepts keys of any length, so this cannot fail.
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(msg);
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

/// Compares two byte strings in constant time (for equal lengths).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// Loads the server's secret key from `<data_dir>/secret.key`, creating it
/// (32 random bytes, owner-only permissions) on first start.
///
/// # Errors
///
/// Fails if the file cannot be read or created, or has the wrong length.
pub fn load_or_create_secret(data_dir: &Path) -> anyhow::Result<[u8; 32]> {
    let path = data_dir.join("secret.key");
    match std::fs::read(&path) {
        Ok(bytes) => bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("{} must hold exactly 32 bytes", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = random_bytes::<32>();
            write_private(&path, &key).with_context(|| format!("writing {}", path.display()))?;
            Ok(key)
        }
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(unix)]
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

#[cfg(not(unix))]
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_and_url_safe() {
        let a = random_token::<32>();
        let b = random_token::<32>();
        assert_ne!(a, b);
        assert_eq!(a.len(), 43);
        assert!(
            a.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
    }

    #[test]
    fn hmac_and_eq() {
        assert_eq!(hmac_b64(b"k", b"m"), hmac_b64(b"k", b"m"));
        assert_ne!(hmac_b64(b"k", b"m"), hmac_b64(b"k2", b"m"));
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }

    #[test]
    fn secret_is_persistent() {
        let d = tempfile::tempdir().unwrap();
        let a = load_or_create_secret(d.path()).unwrap();
        let b = load_or_create_secret(d.path()).unwrap();
        assert_eq!(a, b);
    }
}
