//! The error type for this crate.

/// Convenience alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong in the identity features.
///
/// The server maps these onto HTTP statuses: `InvalidInput` to 400,
/// `InvalidCredentials` to 401, `QuotaExceeded` to 507, `NotFound` to 404,
/// `Conflict` to 409 and the rest to 500 or 502.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The caller sent something malformed.
    #[error("{0}")]
    InvalidInput(String),
    /// A password, code, key or token did not check out.
    #[error("invalid credentials")]
    InvalidCredentials,
    /// The write would take the user over their storage quota.
    #[error("storage quota exceeded: {used} of {limit} bytes used, {requested} more requested")]
    QuotaExceeded {
        /// The user's limit in bytes.
        limit: i64,
        /// Bytes already in use.
        used: i64,
        /// Bytes the write needs.
        requested: i64,
    },
    /// The record does not exist (or belongs to someone else).
    #[error("not found")]
    NotFound,
    /// The request conflicts with the current state.
    #[error("{0}")]
    Conflict(String),
    /// The LDAP server failed or is misconfigured.
    #[error("LDAP: {0}")]
    Ldap(String),
    /// The OpenID provider failed, is misconfigured, or sent a bad token.
    #[error("OpenID Connect: {0}")]
    Oidc(String),
    /// A database error.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl Error {
    pub(crate) fn invalid(msg: impl Into<String>) -> Self {
        Self::InvalidInput(msg.into())
    }
}
