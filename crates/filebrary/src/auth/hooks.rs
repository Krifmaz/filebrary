//! Extension points for auth features built on top of this module: TOTP
//! second factors, external identities (OIDC, LDAP), API keys and quotas.
//!
//! Implement [`AuthHooks`] and install it once at start-up with
//! [`super::AuthRuntime::set_hooks`]. Every method has a default that keeps
//! plain password behaviour, so an implementation only overrides what it needs.

use std::{future::Future, pin::Pin};

use super::{Perms, User, users::UserRecord};
use crate::{error::AppResult, state::AppState, storage::VPath};

/// A boxed, sendable future, so [`AuthHooks`] stays object-safe.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a bearer token resolves to: the user id and the token's scopes.
pub type BearerIdentity = (i64, Vec<String>);

/// Hooks other auth features plug into. All methods have defaults.
pub trait AuthHooks: Send + Sync + 'static {
    /// Called after a correct password for an enabled local user, before the
    /// session is created. `totp` is the optional code from the login body.
    ///
    /// Return `Ok(())` to log in, [`crate::error::AppError::TotpRequired`]
    /// when a code is needed but missing (the web UI then asks for one), or
    /// another error such as `InvalidCredentials` for a wrong code (it
    /// counts as a failed login for rate limiting). Default: `Ok(())`.
    fn after_password<'a>(
        &'a self,
        _state: &'a AppState,
        _user: &'a UserRecord,
        _totp: Option<&'a str>,
    ) -> BoxFuture<'a, AppResult<()>> {
        Box::pin(async { Ok(()) })
    }

    /// Resolves an `Authorization: Bearer <token>` header to a user id and
    /// the token's scopes. `Ok(None)` means the token is unknown (401).
    /// Default: bearer tokens are not accepted.
    fn resolve_bearer<'a>(
        &'a self,
        _state: &'a AppState,
        _token: &'a str,
    ) -> BoxFuture<'a, AppResult<Option<BearerIdentity>>> {
        Box::pin(async { Ok(None) })
    }

    /// The most an API key with these scopes may do: every permission check
    /// for the request is capped by it (see [`super::User::ceiling`]).
    /// Default: [`Perms::ALL`] (rules alone decide).
    fn bearer_ceiling(&self, _scopes: &[String]) -> Perms {
        Perms::ALL
    }

    /// Called by [`super::authorize_write`] after the permission check
    /// passed and before any bytes are written. `path` is the real path in
    /// the source; `bytes` is the size about to be written when known.
    /// Return an error to refuse (a quota check, for example).
    /// Default: allow.
    fn before_write<'a>(
        &'a self,
        _state: &'a AppState,
        _user: &'a User,
        _source: &'a str,
        _path: &'a VPath,
        _bytes: Option<u64>,
    ) -> BoxFuture<'a, AppResult<()>> {
        Box::pin(async { Ok(()) })
    }
}

/// The hooks used until others are installed.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoHooks;

impl AuthHooks for NoHooks {}
