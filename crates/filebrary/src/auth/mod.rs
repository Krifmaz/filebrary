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

/// Permissions a read-only account is capped at.
pub const READ_ONLY_CEILING: Perms = Perms::READ_ONLY;

/// Auth state kept in [`AppState`]: the login rate limiter, the installed
/// hooks and the first-run setup token.
pub struct AuthRuntime {
    /// Failed-login limiter.
    pub limiter: ratelimit::LoginRateLimiter,
    hooks: OnceLock<Arc<dyn hooks::AuthHooks>>,
    /// While the server has no users and no bootstrap password was given:
    /// the one-time token that `POST /api/setup` needs (see
    /// [`bootstrap_admin`]). Taken when the first administrator is created.
    pub(crate) setup_token: tokio::sync::Mutex<Option<String>>,
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
            setup_token: tokio::sync::Mutex::new(None),
        }
    }

    /// Whether first-run setup is waiting for its administrator.
    pub async fn setup_pending(&self) -> bool {
        self.setup_token.lock().await.is_some()
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
    /// Read-only accounts are capped at [`READ_ONLY_CEILING`].
    pub read_only: bool,
    /// Sources the user can reach (any rule granting something, or every
    /// source for administrators), with their scope.
    pub grants: BTreeMap<String, Grant>,
    /// Every permission rule that applies to the user (own and via groups).
    #[serde(skip)]
    pub rules: Vec<Rule>,
    /// Upper bound on every permission check: [`Perms::ALL`] for browser
    /// sessions, narrower for API keys (from
    /// [`hooks::AuthHooks::bearer_ceiling`]).
    #[serde(skip)]
    pub ceiling: Perms,
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
    /// Capped by [`User::ceiling`].
    pub fn perms_at(&self, source: &str, real: &VPath) -> Perms {
        let p = if self.is_admin {
            Perms::ALL
        } else {
            permissions::resolve(self.rules.iter().filter(|r| r.source == source), real)
        };
        p & self.ceiling
    }

    /// The user's role.
    pub fn role(&self) -> users::Role {
        users::Role::from_flags(self.is_admin, self.read_only)
    }

    /// Lowers the ceiling (for an API key) and recomputes the grants. The
    /// new ceiling is intersected with the current one, so an API key never
    /// widens what the account itself may do (a read-only user's key stays
    /// read-only).
    pub fn apply_ceiling(&mut self, ceiling: Perms) {
        self.ceiling = self.ceiling & ceiling;
        let names: Vec<String> = self.grants.keys().cloned().collect();
        for name in names {
            let scope = self.scope(&name);
            let perms = self.perms_at(&name, &scope);
            if let Some(g) = self.grants.get_mut(&name) {
                g.perms = perms;
            }
        }
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

    /// Whether `perm` holds everywhere strictly below the *real* folder
    /// `real`, that is no deeper rule takes it away. Needed before acting on
    /// a whole tree (copy, move, delete) so hidden subfolders are not
    /// copied out, moved away from their rule, or deleted.
    pub fn uniform_below(&self, source: &str, real: &VPath, perm: Perms) -> bool {
        (self.is_admin && self.ceiling.contains(perm))
            || self
                .rules
                .iter()
                .filter(|r| r.source == source && r.path.starts_with(real) && r.path != *real)
                .all(|r| self.perms_at(source, &r.path).contains(perm | Perms::VIEW))
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
    let read_only = rec.read_only && !rec.is_admin;
    let mut user = User {
        id: rec.id,
        username: rec.username,
        auth_source: rec.auth_source,
        is_admin: rec.is_admin,
        read_only,
        grants: BTreeMap::new(),
        rules,
        ceiling: if read_only {
            READ_ONLY_CEILING
        } else {
            Perms::ALL
        },
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
    authorize_inner(state, user, source, perm, path, true).await
}

/// Like [`authorize`] for operations on an entry itself (delete, rename,
/// move away): symlinks in the parent folders are resolved, but a final
/// symlink is not followed, since the link is what gets acted on.
///
/// # Errors
///
/// As [`authorize`].
pub async fn authorize_entry(
    state: &AppState,
    user: &User,
    source: &str,
    perm: Perms,
    path: &VPath,
) -> AppResult<VPath> {
    authorize_inner(state, user, source, perm, path, false).await
}

async fn authorize_inner(
    state: &AppState,
    user: &User,
    source: &str,
    perm: Perms,
    path: &VPath,
    follow_final: bool,
) -> AppResult<VPath> {
    let real = user.require(source, perm, path)?;
    let src = state.source(source).ok_or(AppError::NotFound)?;
    let resolved = if follow_final {
        src.storage.resolve(&real).await
    } else {
        match (real.parent(), real.name()) {
            (Some(parent), Some(name)) => match src.storage.resolve(&parent).await {
                Ok(p) => p.join(name),
                Err(e) => Err(e),
            },
            _ => Ok(real.clone()),
        }
    };
    let resolved = match resolved {
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

/// [`authorize`] for an operation on a whole tree (copy, move, delete):
/// also refuses when a deeper rule takes `perm` away somewhere below.
/// With `follow_final` off it checks like [`authorize_entry`] (for delete
/// and move, which act on a final symlink itself).
///
/// # Errors
///
/// As [`authorize`]; `Forbidden` when the folder holds items the user may
/// not act on.
pub async fn authorize_tree(
    state: &AppState,
    user: &User,
    source: &str,
    perm: Perms,
    path: &VPath,
    follow_final: bool,
) -> AppResult<VPath> {
    let real = authorize_inner(state, user, source, perm, path, follow_final).await?;
    if !user.uniform_below(source, &real, perm) {
        return Err(AppError::forbidden(format!(
            "this folder contains items you do not have the {} permission on",
            perm.names().join("+")
        )));
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

/// Prepares the first administrator when the database has no users.
///
/// * With `admin_password` / `FILEBRARY_ADMIN_PASSWORD` set, the
///   administrator `admin_username` (default `admin`) is created with that
///   password straight away (an optional bootstrap for unattended installs).
/// * Without one, nobody is created and there is no default password: the
///   server enters *first-run setup*. A random one-time setup token is
///   logged once at WARN level, and the web UI's `/setup` page (or
///   `POST /api/setup`) creates the administrator with a name and password
///   of the person's choosing, given that token. The token proves that
///   whoever claims the server can read its logs.
///
/// Returns the setup token when setup mode was entered.
///
/// # Errors
///
/// Fails if the configured password is invalid or the insert fails.
pub async fn bootstrap_admin(state: &AppState) -> AppResult<Option<String>> {
    if users::count(&state.db).await? > 0 {
        return Ok(None);
    }
    let name = state.config.admin_username.as_str();
    if let Some(password) = &state.config.admin_password {
        users::create(&state.db, name, password, true).await?;
        tracing::info!(username = name, "created the first administrator");
        return Ok(None);
    }
    let token = crypto::random_token::<18>();
    *state.auth.setup_token.lock().await = Some(token.clone());
    tracing::warn!(
        setup_token = %token,
        "no users yet: open the web UI, choose an administrator name and \
         password on the setup page and enter this setup token there \
         (it is shown once per start)"
    );
    Ok(Some(token))
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
            let mut user = load_user(state, uid).await?.ok_or(AppError::Unauthorized)?;
            user.apply_ceiling(state.auth.hooks().bearer_ceiling(&scopes));
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
            read_only: false,
            grants,
            rules,
            ceiling: Perms::ALL,
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
    fn ceiling_caps_every_check() {
        let mut u = user(vec![rule("/", Perms::ALL)], "/");
        assert!(u.require("s", Perms::DELETE, &VPath::root()).is_ok());
        u.apply_ceiling(Perms::VIEW | Perms::DOWNLOAD);
        assert!(u.require("s", Perms::DOWNLOAD, &VPath::root()).is_ok());
        assert!(u.require("s", Perms::DELETE, &VPath::root()).is_err());
        assert_eq!(u.grant("s").unwrap().perms, Perms::VIEW | Perms::DOWNLOAD);
        u.is_admin = true;
        assert!(!u.perms_at("s", &VPath::root()).contains(Perms::DELETE));
    }

    #[test]
    fn uniform_below_sees_narrower_rules() {
        let u = user(
            vec![rule("/team", Perms::ALL), rule("/team/hr", Perms::NONE)],
            "/",
        );
        let vp = |s: &str| VPath::parse(s).unwrap();
        assert!(!u.uniform_below("s", &vp("team"), Perms::DOWNLOAD));
        assert!(u.uniform_below("s", &vp("team/hr"), Perms::DOWNLOAD));
        assert!(u.uniform_below("s", &vp("team/dev"), Perms::DELETE));
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
