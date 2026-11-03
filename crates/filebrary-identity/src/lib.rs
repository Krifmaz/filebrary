//! Identity features layered on top of Filebrary's core authentication.
//!
//! This crate holds the logic and storage for:
//!
//! * [`totp`]: time-based one-time passwords (RFC 6238) as a second factor,
//!   with single-use recovery codes and replay protection.
//! * [`apikey`]: long-lived API keys with scopes, stored only as hashes.
//! * [`quota`]: per-user storage quotas with atomic reservations.
//! * [`ldap`]: password checks against an LDAP directory.
//! * [`oidc`]: OpenID Connect single sign-on (authorization code flow with
//!   PKCE and nonce).
//! * [`identities`]: links between external accounts (an OIDC subject or an
//!   LDAP DN) and local users.
//!
//! The crate knows nothing about HTTP or the server state: every function
//! takes a [`sqlx::SqlitePool`] and plain values. The server crate wires these
//! into its login flow, request extractors and write checks.
//!
//! The tables live in `migrations/0015_identity.sql` of the workspace.

pub mod apikey;
pub mod base32;
mod error;
pub mod identities;
pub mod ldap;
pub mod oidc;
pub mod quota;
mod secret;
pub mod totp;

pub use error::{Error, Result};
pub use secret::SecretString;

/// The current time as Unix seconds.
pub(crate) fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// SHA-256 of `data`.
pub(crate) fn sha256(data: &[u8]) -> Vec<u8> {
    use sha2::Digest;
    sha2::Sha256::digest(data).to_vec()
}

/// Fills a fresh array with cryptographically secure random bytes.
pub(crate) fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    rand::fill(&mut b);
    b
}
