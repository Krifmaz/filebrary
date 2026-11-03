//! Users, groups, permissions, sessions and the request extractors that
//! enforce them.
//!
//! # Contract for handlers
//!
//! * Take [`AuthUser`] as an argument to require a logged-in user. The
//!   extractor accepts the session cookie and, when a hook is installed, an
//!   `Authorization: Bearer` API key. For cookie sessions and unsafe methods
//!   (anything but GET/HEAD/OPTIONS/TRACE) it also checks the
//!   `X-CSRF-Token` header. `Option<AuthUser>` makes login optional.
//! * Take [`AdminUser`] to require an administrator.
//! * Call [`User::require`] with the source, the permission needed and the
//!   *user-visible* path. It returns the real path inside the source (the
//!   user's scope prefixed), `NotFound` without view access (so hidden
//!   folders are not revealed) or `Forbidden` without the permission.
//! * Before writing bytes, call [`authorize_write`] instead: it does the
//!   same check and then runs the [`hooks::AuthHooks::before_write`] hook
//!   (quotas).
//!
//! Extractors are generic over any router state `S` with
//! `AppState: FromRef<S>`.
//!
//! # Permissions
//!
//! See [`permissions`] for the rule model (per-source, per-folder, users
//! and groups, most specific folder wins). A user may also be confined to a
//! *scope* (home folder) per source; paths they send are relative to it and
//! rules apply to real paths.

pub mod groups;
pub mod hooks;
pub mod password;
pub mod permissions;
pub mod ratelimit;
pub mod routes;
pub mod session;
pub mod users;

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{Arc, OnceLock},
    time::Duration,
};

use axum::{
    extract::{ConnectInfo, FromRef, FromRequestParts, OptionalFromRequestParts},
    http::{header, request::Parts},
};
use axum_extra::extract::CookieJar;
use serde::Serialize;

pub use permissions::{Perms, Rule, Subject};

use crate::{
    config::Config,
    crypto,
    error::{AppError, AppResult},
    state::AppState,
    storage::VPath,
};

/// Name of the CSRF header clients send on unsafe requests.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// Auth state kept in [`AppState`]: the login rate limiter and the
/// installed hooks.
pub struct AuthRuntime {
    /// Failed-login limiter.
    pub limiter: ratelimit::LoginRateLimiter,
    hooks: OnceLock<Arc<dyn hooks::AuthHooks>>,
}

impl AuthRuntime {
    /// Builds the runtime from the configuration (rate limit settings).
    pub fn new(config: &Config) -> Self {
        let max = config.login_max_attempts;
        Self {
            limiter: ratelimit::LoginRateLimiter::new(
                Duration::from_secs(config.login_window_secs.max(1)),
                max,
                max,
            ),
            hooks: OnceLock::new(),
        }
    }

    /// Installs the hooks. Call once at start-up, before serving.
    ///
    /// # Errors
    ///
    /// Returns the hooks back if some were already installed.
    pub fn set_hooks(
        &self,
        hooks: Arc<dyn hooks::AuthHooks>,
    ) -> Result<(), Arc<dyn hooks::AuthHooks>> {
        self.hooks.set(hooks)
    }

    /// The installed hooks, or [`hooks::NoHooks`].
    pub fn hooks(&self) -> &dyn hooks::AuthHooks {
        match self.hooks.get() {
            Some(h) => h.as_ref(),
            None => &hooks::NoHooks,
        }
    }
}

impl std::fmt::Debug for AuthRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthRuntime")
            .field("hooks_installed", &self.hooks.get().is_some())
            .finish_non_exhaustive()
    }
}

/// A user's access to one source, as summarised for the UI.
#[derive(Clone, Debug, Serialize)]
pub struct Grant {
    /// What the user may do at the top of their scope.
    pub perms: Perms,
    /// The folder the user is confined to; paths they send are relative to it.
    pub scope: VPath,
}

/// A loaded, enabled user with everything needed to check permissions
/// without further database queries.
#[derive(Clone, Debug, Serialize)]
pub struct User {
    /// Database id.
    pub id: i64,
    /// Login name.
    pub username: String,
    /// Where the account authenticates (`local`, `oidc`, `ldap`...).
    pub auth_source: String,
    /// Administrators have every permission on every source.
    pub is_admin: bool,
    /// Sources the user can reach (any rule granting something, or every
    /// source for administrators), with their scope.
    pub grants: BTreeMap<String, Grant>,
    /// Every permission rule that applies to the user (own and via groups).
    #[serde(skip)]
    pub rules: Vec<Rule>,
}

impl User {
    /// The user's grant on `source`, if they can reach it at all.
    pub fn grant(&self, source: &str) -> Option<&Grant> {
        self.grants.get(source)
    }

    /// The user's scope on `source` (the root when unconfined).
    pub fn scope(&self, source: &str) -> VPath {
        self.grant(source)
            .map(|g| g.scope.clone())
            .unwrap_or_default()
    }

    /// Effective permissions at a *real* path in `source`.
    pub fn perms_at(&self, source: &str, real: &VPath) -> Perms {
        if self.is_admin {
            return Perms::ALL;
        }
        permissions::resolve(self.rules.iter().filter(|r| r.source == source), real)
    }

    /// Effective permissions at a *user-visible* path in `source`.
    /// Nothing outside sources the user can reach.
    pub fn effective(&self, source: &str, path: &VPath) -> Perms {
        match self.grant(source) {
            Some(g) => self.perms_at(source, &g.scope.join_path(path)),
            None => Perms::NONE,
        }
    }

    /// Whether the user can see anything at or below a *user-visible* path,
    /// for showing the folders that lead to a deeper grant.
    pub fn can_traverse(&self, source: &str, path: &VPath) -> bool {
        let Some(g) = self.grant(source) else {
            return false;
        };
        let real = g.scope.join_path(path);
        if self.perms_at(source, &real).contains(Perms::VIEW) {
            return true;
        }
        self.rules.iter().any(|r| {
            r.source == source
                && r.path.starts_with(&real)
                && !self.perms_at(source, &r.path).is_empty()
        })
    }

    /// Checks that the user may do `perm` on `source` and maps the
    /// user-visible `path` to the real path inside the source.
    ///
    /// # Errors
    ///
    /// `NotFound` without view access (unknown sources included), so hidden
    /// files are not revealed; `Forbidden` without `perm`.
    pub fn require(&self, source: &str, perm: Perms, path: &VPath) -> AppResult<VPath> {
        let g = self.grant(source).ok_or(AppError::NotFound)?;
        let real = g.scope.join_path(path);
        let have = self.perms_at(source, &real);
        if !have.contains(Perms::VIEW) {
            return Err(AppError::NotFound);
        }
        if !have.contains(perm) {
            return Err(AppError::forbidden(format!(
                "you do not have the {} permission here",
                perm.names().join("+")
            )));
        }
        Ok(real)
    }

    /// Maps a real source path back to what this user sees, if it is in scope.
    pub fn visible_path(&self, source: &str, real: &VPath) -> Option<VPath> {
        real.strip_prefix(&self.grant(source)?.scope)
    }
}

/// Loads an enabled user with their rules and scopes. `None` if the user is
/// missing or disabled. Use it to re-check a share creator's rights.
///
/// # Errors
///
/// Database failures.
pub async fn load_user(state: &AppState, id: i64) -> AppResult<Option<User>> {
    let rec = match users::get(&state.db, id).await {
        Ok(r) => r,
        Err(AppError::NotFound) => return Ok(None),
        Err(e) => return Err(e),
    };
    if rec.disabled {
        return Ok(None);
    }
    let rules = if rec.is_admin {
        Vec::new()
    } else {
        permissions::rules_for_user(&state.db, id).await?
    };
    let scopes = users::scopes(&state.db, id).await?;
    let mut user = User {
        id: rec.id,
        username: rec.username,
        auth_source: rec.auth_source,
        is_admin: rec.is_admin,
        grants: BTreeMap::new(),
        rules,
    };
    for name in state.sources.keys() {
        let scope = scopes.get(name).cloned().unwrap_or_default();
        let reachable = user.is_admin
            || user
                .rules
                .iter()
                .any(|r| &r.source == name && !r.perms.is_empty());
        if reachable {
            let perms = user.perms_at(name, &scope);
            user.grants.insert(name.clone(), Grant { perms, scope });
        }
    }
    Ok(Some(user))
}

/// [`User::require`] plus a symlink check: the path is resolved through
/// the source's backend ([`crate::storage::Storage::resolve`]) and the
/// resolved path must still be inside the user's scope and carry `perm`
/// under the permission rules. So a link in a readable folder cannot lead
/// into a hidden one. Returns the real (unresolved) path in the source.
///
/// Every handler that touches storage should go through this (directly or
/// via `api::files::scoped`) rather than [`User::require`] alone.
///
/// # Errors
///
/// As [`User::require`]; `NotFound` for an unknown source; `Forbidden` when
/// a link leads outside the scope or the root, or into a folder without
/// `perm`.
pub async fn authorize(
    state: &AppState,
    user: &User,
    source: &str,
    perm: Perms,
    path: &VPath,
) -> AppResult<VPath> {
    let real = user.require(source, perm, path)?;
    let src = state.source(source).ok_or(AppError::NotFound)?;
    let resolved = match src.storage.resolve(&real).await {
        Ok(r) => r,
        Err(crate::storage::StorageError::OutsideRoot) => {
            return Err(AppError::forbidden("path is outside the source"));
        }
        Err(e) => return Err(e.into()),
    };
    if resolved != real {
        check_resolved(user, source, perm, &resolved)?;
    }
    Ok(real)
}

/// The check [`authorize`] applies to a symlink-resolved path.
fn check_resolved(user: &User, source: &str, perm: Perms, resolved: &VPath) -> AppResult<()> {
    if !resolved.starts_with(&user.scope(source)) {
        return Err(AppError::forbidden("a link here leads outside your folder"));
    }
    if !user.perms_at(source, resolved).contains(perm | Perms::VIEW) {
        return Err(AppError::forbidden(
            "a link here leads to a folder you cannot use",
        ));
    }
    Ok(())
}

/// [`authorize`] for writes, followed by the `before_write` hook (quotas).
/// Every upload, overwrite and copy should go through here. `bytes` is the
/// size about to be written, when known.
///
/// # Errors
///
/// As [`authorize`], plus whatever the hook returns.
pub async fn authorize_write(
    state: &AppState,
    user: &User,
    source: &str,
    perm: Perms,
    path: &VPath,
    bytes: Option<u64>,
) -> AppResult<VPath> {
    let real = authorize(state, user, source, perm, path).await?;
    state
        .auth
        .hooks()
        .before_write(state, user, source, &real, bytes)
        .await?;
    Ok(real)
}

/// Creates the first administrator when the database has no users.
///
/// The name comes from `admin_username` (default `admin`) and the password
/// from `admin_password` / `FILEBRARY_ADMIN_PASSWORD`. Without one, a random
/// password is generated and logged once at WARN level. Returns the
/// password when one was generated.
///
/// # Errors
///
/// Fails if the configured password is invalid or the insert fails.
pub async fn bootstrap_admin(state: &AppState) -> AppResult<Option<String>> {
    if users::count(&state.db).await? > 0 {
        return Ok(None);
    }
    let name = state.config.admin_username.as_str();
    let (password, generated) = match &state.config.admin_password {
        Some(p) => (p.clone(), false),
        None => (crypto::random_token::<18>(), true),
    };
    users::create(&state.db, name, &password, true).await?;
    if generated {
        tracing::warn!(
            username = name,
            password = %password,
            "created the first administrator with a generated password; \
             log in and change it (it is not shown again)"
        );
        Ok(Some(password))
    } else {
        tracing::info!(username = name, "created the first administrator");
        Ok(None)
    }
}

/// The client's address: the first `X-Forwarded-For` entry when
/// `trust_proxy_headers` is on, otherwise the TCP peer address.
pub fn client_ip(parts: &Parts, config: &Config) -> Option<String> {
    if config.trust_proxy_headers {
        let fwd = parts
            .headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            .filter(|v| !v.is_empty());
        if let Some(ip) = fwd {
            return Some(ip.chars().take(64).collect());
        }
    }
    parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip().to_string())
}

/// Extractor for a logged-in user (see the module docs for the CSRF rule).
#[derive(Clone, Debug)]
pub struct AuthUser {
    /// The user.
    pub user: User,
    /// SHA-256 of the session token (to log out this session). Empty for
    /// API-key requests.
    pub session_hash: Vec<u8>,
    /// The session's CSRF token. Empty for API-key requests.
    pub csrf_token: String,
    /// The API key's scopes, or `None` for a browser session (unrestricted).
    pub scopes: Option<Vec<String>>,
}

impl AuthUser {
    /// Whether the request may use `scope`: always for sessions; for API
    /// keys when the key lists it or `*`.
    pub fn has_scope(&self, scope: &str) -> bool {
        match &self.scopes {
            None => true,
            Some(s) => s.iter().any(|x| x == scope || x == "*"),
        }
    }

    /// `Forbidden` unless [`AuthUser::has_scope`].
    ///
    /// # Errors
    ///
    /// `Forbidden` naming the missing scope.
    pub fn require_scope(&self, scope: &str) -> AppResult<()> {
        if self.has_scope(scope) {
            Ok(())
        } else {
            Err(AppError::forbidden(format!(
                "this API key lacks the {scope} scope"
            )))
        }
    }

    async fn authenticate(parts: &mut Parts, state: &AppState) -> AppResult<Self> {
        if let Some(cached) = parts.extensions.get::<AuthUser>() {
            return Ok(cached.clone());
        }
        let bearer = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                v.strip_prefix("Bearer ")
                    .or_else(|| v.strip_prefix("bearer "))
            })
            .map(|t| t.trim().to_owned());
        let me = if let Some(token) = bearer {
            let (uid, scopes) = state
                .auth
                .hooks()
                .resolve_bearer(state, &token)
                .await?
                .ok_or(AppError::Unauthorized)?;
            let user = load_user(state, uid).await?.ok_or(AppError::Unauthorized)?;
            AuthUser {
                user,
                session_hash: Vec::new(),
                csrf_token: String::new(),
                scopes: Some(scopes),
            }
        } else {
            let jar = CookieJar::from_headers(&parts.headers);
            let token = jar
                .get(session::SESSION_COOKIE)
                .map(|c| c.value().to_owned())
                .ok_or(AppError::Unauthorized)?;
            let sess = session::lookup(state, &token)
                .await?
                .ok_or(AppError::Unauthorized)?;
            if !parts.method.is_safe() {
                let sent = parts
                    .headers
                    .get(CSRF_HEADER)
                    .map(|v| v.as_bytes())
                    .unwrap_or_default();
                if !crypto::ct_eq(sent, sess.csrf_token.as_bytes()) {
                    return Err(AppError::forbidden("missing or invalid CSRF token"));
                }
            }
            let user = load_user(state, sess.user_id)
                .await?
                .ok_or(AppError::Unauthorized)?;
            AuthUser {
                user,
                session_hash: sess.token_hash,
                csrf_token: sess.csrf_token,
                scopes: None,
            }
        };
        parts.extensions.insert(me.clone());
        Ok(me)
    }
}

impl<S> FromRequestParts<S> for AuthUser
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> AppResult<Self> {
        Self::authenticate(parts, &AppState::from_ref(state)).await
    }
}

impl<S> OptionalFromRequestParts<S> for AuthUser
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AppError;

    /// `None` when there are no (valid) credentials; a CSRF failure or a
    /// database error is still an error.
    async fn from_request_parts(parts: &mut Parts, state: &S) -> AppResult<Option<Self>> {
        match Self::authenticate(parts, &AppState::from_ref(state)).await {
            Ok(u) => Ok(Some(u)),
            Err(AppError::Unauthorized) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Extractor for a logged-in administrator (`Forbidden` otherwise).
#[derive(Clone, Debug)]
pub struct AdminUser(pub AuthUser);

impl<S> FromRequestParts<S> for AdminUser
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> AppResult<Self> {
        let u = AuthUser::authenticate(parts, &AppState::from_ref(state)).await?;
        if !u.user.is_admin {
            return Err(AppError::forbidden("administrators only"));
        }
        u.require_scope("admin")?;
        Ok(Self(u))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(rules: Vec<Rule>, scope: &str) -> User {
        let mut grants = BTreeMap::new();
        grants.insert(
            "s".to_owned(),
            Grant {
                perms: Perms::NONE,
                scope: VPath::parse(scope).unwrap(),
            },
        );
        User {
            id: 1,
            username: "bob".into(),
            auth_source: "local".into(),
            is_admin: false,
            grants,
            rules,
        }
    }

    fn rule(path: &str, perms: Perms) -> Rule {
        Rule {
            id: 0,
            source: "s".into(),
            path: VPath::parse(path).unwrap(),
            subject: Subject::Group(1),
            perms: perms.with_implied(),
        }
    }

    #[test]
    fn require_maps_scope_and_checks_rules() {
        let u = user(
            vec![
                rule("/home/bob", Perms::DOWNLOAD),
                rule("/home/bob/secret", Perms::NONE),
            ],
            "home/bob",
        );
        let real = u
            .require("s", Perms::DOWNLOAD, &VPath::parse("a.txt").unwrap())
            .unwrap();
        assert_eq!(real.to_string(), "/home/bob/a.txt");
        assert!(matches!(
            u.require("s", Perms::DELETE, &VPath::root()),
            Err(AppError::Forbidden(_))
        ));
        assert!(matches!(
            u.require("s", Perms::VIEW, &VPath::parse("secret/x").unwrap()),
            Err(AppError::NotFound)
        ));
        assert!(matches!(
            u.require("x", Perms::VIEW, &VPath::root()),
            Err(AppError::NotFound)
        ));
        assert_eq!(u.visible_path("s", &real).unwrap().to_string(), "/a.txt");
        assert!(
            u.visible_path("s", &VPath::parse("home/eve").unwrap())
                .is_none()
        );
    }

    #[test]
    fn resolved_links_must_stay_in_scope_and_rules() {
        let u = user(
            vec![
                rule("/home/bob", Perms::DOWNLOAD),
                rule("/home/bob/hidden", Perms::NONE),
                rule("/home/eve", Perms::DOWNLOAD),
            ],
            "home/bob",
        );
        let vp = |s: &str| VPath::parse(s).unwrap();
        assert!(check_resolved(&u, "s", Perms::DOWNLOAD, &vp("home/bob/real.txt")).is_ok());
        // A link to another user's folder: outside the scope even with rights there.
        assert!(check_resolved(&u, "s", Perms::DOWNLOAD, &vp("home/eve/x")).is_err());
        // A link into a hidden subfolder.
        assert!(check_resolved(&u, "s", Perms::DOWNLOAD, &vp("home/bob/hidden/x")).is_err());
        assert!(check_resolved(&u, "s", Perms::EDIT, &vp("home/bob/real.txt")).is_err());
    }

    #[test]
    fn traverse_reaches_deeper_grants() {
        let u = user(vec![rule("/a/b/c", Perms::VIEW)], "/");
        assert!(u.can_traverse("s", &VPath::root()));
        assert!(u.can_traverse("s", &VPath::parse("a/b").unwrap()));
        assert!(!u.can_traverse("s", &VPath::parse("x").unwrap()));
        assert!(
            u.require("s", Perms::VIEW, &VPath::parse("a").unwrap())
                .is_err()
        );
    }

    #[test]
    fn scopes_on_api_keys() {
        let mut a = AuthUser {
            user: user(vec![], "/"),
            session_hash: vec![],
            csrf_token: String::new(),
            scopes: None,
        };
        assert!(a.has_scope("files:write"));
        a.scopes = Some(vec!["files:read".into()]);
        assert!(a.has_scope("files:read") && !a.has_scope("files:write"));
        a.scopes = Some(vec!["*".into()]);
        assert!(a.has_scope("admin"));
    }
}
