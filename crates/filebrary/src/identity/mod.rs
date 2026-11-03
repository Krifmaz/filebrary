//! Server side of the identity features: OpenID Connect and LDAP sign-in,
//! TOTP two-factor login, scoped API keys and storage quotas.
//!
//! The logic and storage live in the `filebrary_identity` crate. This module
//! plugs them into the core auth module through [`crate::auth::hooks::AuthHooks`]
//! (installed by [`AppState::new`]) and adds the endpoints in [`routes`].
//!
//! # How the pieces meet the core
//!
//! | Hook | What happens here |
//! |---|---|
//! | `authenticate_external` | LDAP search-then-bind for users who are not local password accounts |
//! | `after_password` | TOTP code check; refuses local password logins when `oidc.disable_password_login` is on (administrators excepted) |
//! | `resolve_bearer` | `Authorization: Bearer fbk_...` API keys |
//! | `bearer_ceiling` | maps a key's scopes to the most it may do |
//! | `before_write` | reserves quota for the bytes about to be written |

pub mod routes;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use filebrary_identity::{
    self as idn, apikey,
    identities::{self, Provider},
    ldap, oidc, quota, totp,
};

use crate::{
    auth::{
        Perms, User,
        hooks::{AuthHooks, BearerIdentity, BoxFuture},
        users::{self, UserRecord},
    },
    config::Config,
    error::{AppError, AppResult},
    state::AppState,
    storage::VPath,
};

/// How long a discovered OpenID provider (and its signing keys) is reused
/// before discovery runs again.
const OIDC_REFRESH: Duration = Duration::from_secs(3600);

impl From<idn::Error> for AppError {
    fn from(e: idn::Error) -> Self {
        match e {
            idn::Error::InvalidInput(m) => AppError::BadRequest(m),
            idn::Error::InvalidCredentials => AppError::InvalidCredentials,
            e @ idn::Error::QuotaExceeded { .. } => AppError::QuotaExceeded(e.to_string()),
            idn::Error::NotFound => AppError::NotFound,
            idn::Error::Conflict(m) => AppError::Conflict(m),
            idn::Error::Db(e) => e.into(),
            e @ (idn::Error::Ldap(_) | idn::Error::Oidc(_)) => AppError::Internal(e.into()),
        }
    }
}

/// Identity settings and caches kept in [`AppState`].
pub struct Identity {
    oidc: Option<OidcSlot>,
    default_quota: Option<i64>,
}

struct OidcSlot {
    cfg: oidc::OidcConfig,
    redirect_url: String,
    cached: tokio::sync::Mutex<Option<(Arc<oidc::OidcProvider>, Instant)>>,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("oidc", &self.oidc.is_some())
            .field("default_quota", &self.default_quota)
            .finish()
    }
}

impl Identity {
    /// Builds the runtime from the (validated) configuration. Nothing is
    /// contacted here: the OpenID provider is discovered on first use.
    pub fn new(config: &Config) -> Self {
        let oidc = config.oidc.clone().map(|cfg| OidcSlot {
            cfg,
            redirect_url: format!(
                "{}/api/auth/oidc/callback",
                config
                    .public_url
                    .as_deref()
                    .unwrap_or("http://localhost")
                    .trim_end_matches('/')
            ),
            cached: tokio::sync::Mutex::new(None),
        });
        Self {
            oidc,
            default_quota: config.default_quota_bytes(),
        }
    }

    /// The OpenID settings, when single sign-on is configured.
    pub fn oidc_config(&self) -> Option<&oidc::OidcConfig> {
        self.oidc.as_ref().map(|s| &s.cfg)
    }

    /// The server-wide default quota in bytes.
    pub fn default_quota(&self) -> Option<i64> {
        self.default_quota
    }

    /// The discovered OpenID provider (`None` when SSO is not configured).
    /// Discovery is cached for an hour, so rotated signing keys are picked up.
    ///
    /// # Errors
    ///
    /// `Internal` when discovery fails (the provider is down or misconfigured).
    pub async fn oidc_provider(&self) -> AppResult<Option<Arc<oidc::OidcProvider>>> {
        let Some(slot) = &self.oidc else {
            return Ok(None);
        };
        let mut cached = slot.cached.lock().await;
        if let Some((p, at)) = cached.as_ref()
            && at.elapsed() < OIDC_REFRESH
        {
            return Ok(Some(p.clone()));
        }
        let p = Arc::new(oidc::OidcProvider::discover(slot.cfg.clone(), &slot.redirect_url).await?);
        *cached = Some((p.clone(), Instant::now()));
        Ok(Some(p))
    }
}

/// An account vouched for by an external provider, to map to a local user.
#[derive(Clone, Debug)]
pub struct ExternalAccount {
    /// OIDC or LDAP.
    pub provider: Provider,
    /// OIDC issuer URL or LDAP server URL.
    pub issuer: String,
    /// OIDC `sub` or LDAP DN.
    pub subject: String,
    /// Suggested local username.
    pub username: String,
    /// Administrator status from the provider's admin group, when one is configured.
    pub admin: Option<bool>,
    /// Group names from the provider (synchronised with local groups of the same name).
    pub groups: Vec<String>,
    /// Create the local user if none is linked or pre-created.
    pub auto_create: bool,
}

/// Finds or creates the local user for an external account.
///
/// Matching order: an existing link (issuer + subject); then a not yet linked
/// user an administrator created with the same name and this provider as its
/// `auth_source`; then, with `auto_create`, a new user. A local password
/// account with the same name is never taken over.
///
/// On every login, administrator status follows the provider's admin group
/// (when configured) and membership of local groups follows the provider's
/// groups (when it sends any): the user is put in each local group whose name
/// matches a provider group, and taken out of local groups it no longer lists.
///
/// # Errors
///
/// `InvalidCredentials` when no user may be used, `Conflict` when the name is
/// taken by another kind of account.
pub async fn provision(state: &AppState, ext: &ExternalAccount) -> AppResult<UserRecord> {
    let db = &state.db;
    let src = ext.provider.as_str();
    let mut rec = match identities::find(db, ext.provider, &ext.issuer, &ext.subject).await? {
        Some(uid) => users::get(db, uid).await?,
        None => {
            let existing = users::get_with_hash(db, &ext.username)
                .await?
                .map(|(r, _)| r);
            match existing {
                Some(r)
                    if r.auth_source == src && !is_linked(state, ext.provider, r.id).await? =>
                {
                    identities::link(db, ext.provider, &ext.issuer, &ext.subject, r.id).await?;
                    r
                }
                Some(_) => {
                    tracing::warn!(
                        username = %ext.username, provider = src,
                        "external login refused: the name belongs to another account"
                    );
                    return Err(AppError::conflict(
                        "an account with this name already exists; ask an administrator to link it",
                    ));
                }
                None if ext.auto_create => {
                    let r =
                        users::create_external(db, &ext.username, src, ext.admin.unwrap_or(false))
                            .await?;
                    identities::link(db, ext.provider, &ext.issuer, &ext.subject, r.id).await?;
                    tracing::info!(user = r.id, username = %r.username, provider = src, "created user from external login");
                    r
                }
                None => return Err(AppError::InvalidCredentials),
            }
        }
    };
    if rec.auth_source != src {
        return Err(AppError::InvalidCredentials);
    }
    if let Some(admin) = ext.admin
        && admin != rec.is_admin
    {
        let upd = users::UserUpdate {
            is_admin: Some(admin),
            ..Default::default()
        };
        match users::update(db, rec.id, &upd).await {
            Ok(r) => rec = r,
            Err(e) => tracing::warn!(user = rec.id, error = %e, "could not sync admin status"),
        }
    }
    if !ext.groups.is_empty() {
        sync_groups(state, rec.id, &ext.groups).await?;
    }
    Ok(rec)
}

async fn is_linked(state: &AppState, provider: Provider, user_id: i64) -> AppResult<bool> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM external_identities WHERE provider = ? AND user_id = ?",
    )
    .bind(provider.as_str())
    .bind(user_id)
    .fetch_one(&state.db)
    .await?;
    Ok(n > 0)
}

async fn sync_groups(state: &AppState, user_id: i64, wanted: &[String]) -> AppResult<()> {
    use crate::auth::groups;
    let wanted: Vec<String> = wanted.iter().map(|g| g.to_lowercase()).collect();
    let current = groups::for_user(&state.db, user_id).await?;
    for g in groups::list(&state.db).await? {
        let want = wanted.contains(&g.name.to_lowercase());
        let has = current.iter().any(|c| c.id == g.id);
        if want && !has {
            groups::add_member(&state.db, g.id, user_id).await?;
        } else if !want && has {
            groups::remove_member(&state.db, g.id, user_id).await?;
        }
    }
    Ok(())
}

/// The first `cn=` value of a DN, which is how LDAP groups are matched to
/// local group names (`cn=editors,ou=groups,...` matches `editors`).
fn cn_of(dn: &str) -> Option<String> {
    dn.split(',')
        .next()
        .and_then(|rdn| rdn.split_once('='))
        .filter(|(k, _)| k.trim().eq_ignore_ascii_case("cn"))
        .map(|(_, v)| v.trim().to_owned())
}

/// The [`AuthHooks`] implementation installed by [`AppState::new`].
#[derive(Clone, Copy, Debug, Default)]
pub struct IdentityHooks;

impl AuthHooks for IdentityHooks {
    fn authenticate_external<'a>(
        &'a self,
        state: &'a AppState,
        username: &'a str,
        password: &'a str,
        existing: Option<&'a UserRecord>,
    ) -> BoxFuture<'a, AppResult<Option<UserRecord>>> {
        Box::pin(async move {
            let Some(cfg) = &state.config.ldap else {
                return Ok(None);
            };
            if existing.is_some_and(|r| r.auth_source != Provider::Ldap.as_str()) {
                return Ok(None);
            }
            let Some(lu) = ldap::authenticate(cfg, username, password).await? else {
                return Ok(None);
            };
            let groups = lu
                .groups
                .iter()
                .filter_map(|dn| cn_of(dn))
                .collect::<Vec<_>>();
            let ext = ExternalAccount {
                provider: Provider::Ldap,
                issuer: cfg.url.clone(),
                subject: lu.dn.clone(),
                username: lu.username.clone(),
                admin: cfg.admin_group.as_ref().map(|_| lu.is_admin),
                groups,
                auto_create: true,
            };
            match provision(state, &ext).await {
                Ok(r) => Ok(Some(r)),
                Err(AppError::InvalidCredentials | AppError::Conflict(_)) => Ok(None),
                Err(e) => Err(e),
            }
        })
    }

    fn after_password<'a>(
        &'a self,
        state: &'a AppState,
        user: &'a UserRecord,
        code: Option<&'a str>,
    ) -> BoxFuture<'a, AppResult<()>> {
        Box::pin(async move {
            if let Some(o) = state.identity.oidc_config()
                && o.disable_password_login
                && user.auth_source == "local"
                && !user.is_admin
            {
                return Err(AppError::forbidden(
                    "password login is turned off; sign in with single sign-on",
                ));
            }
            if !totp::status(&state.db, user.id).await?.enabled {
                return Ok(());
            }
            let Some(code) = code.filter(|c| !c.trim().is_empty()) else {
                return Err(AppError::TotpRequired);
            };
            totp::verify_login(&state.db, &state.secret, user.id, code)
                .await
                .map_err(|e| match e {
                    idn::Error::Db(e) => AppError::from(e),
                    _ => AppError::InvalidCredentials,
                })
        })
    }

    fn resolve_bearer<'a>(
        &'a self,
        state: &'a AppState,
        token: &'a str,
    ) -> BoxFuture<'a, AppResult<Option<BearerIdentity>>> {
        Box::pin(async move {
            match apikey::resolve(&state.db, token).await {
                Ok(Some((uid, scopes))) => Ok(Some((
                    uid,
                    scopes.iter().map(|s| s.as_str().to_owned()).collect(),
                ))),
                Ok(None) | Err(idn::Error::InvalidCredentials) => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
    }

    fn bearer_ceiling(&self, scopes: &[String]) -> Perms {
        scope_ceiling(scopes)
    }

    fn before_write<'a>(
        &'a self,
        state: &'a AppState,
        user: &'a User,
        _source: &'a str,
        _path: &'a VPath,
        bytes: Option<u64>,
    ) -> BoxFuture<'a, AppResult<()>> {
        Box::pin(async move {
            let Some(b) = bytes else {
                return Ok(());
            };
            let b = i64::try_from(b).map_err(|_| AppError::bad_request("size too large"))?;
            quota::reserve(&state.db, user.id, b, state.identity.default_quota()).await?;
            Ok(())
        })
    }
}

/// The file permissions an API key's scopes allow.
pub fn scope_ceiling(scopes: &[String]) -> Perms {
    let parsed: Vec<apikey::Scope> = scopes
        .iter()
        .filter_map(|s| apikey::Scope::parse(s))
        .collect();
    Perms::from_names(&apikey::allowed_permissions(&parsed)).unwrap_or(Perms::NONE)
}

/// Measures how many bytes a user stores: the total size of everything in
/// their home folder on each source where they are confined to one. Returns
/// `None` for users without any home folder (their usage stays the running
/// total kept on writes).
///
/// # Errors
///
/// Database or storage errors.
pub async fn measure_usage(state: &AppState, user_id: i64) -> AppResult<Option<i64>> {
    let scopes = users::scopes(&state.db, user_id).await?;
    let mut total: u64 = 0;
    let mut any = false;
    for (source, scope) in scopes {
        if scope == VPath::root() {
            continue;
        }
        let Some(src) = state.source(&source) else {
            continue;
        };
        any = true;
        let mut stack = vec![scope];
        while let Some(dir) = stack.pop() {
            let entries = match src.storage.list(&dir).await {
                Ok(e) => e,
                Err(crate::storage::StorageError::NotFound) => continue,
                Err(e) => return Err(e.into()),
            };
            for e in entries {
                if e.is_dir() {
                    stack.push(e.path);
                } else {
                    total = total.saturating_add(e.size);
                }
            }
        }
    }
    Ok(any.then(|| i64::try_from(total).unwrap_or(i64::MAX)))
}

/// Re-measures every user with a quota. Returns how many were updated.
///
/// # Errors
///
/// Database errors (storage errors for one user are logged and skipped).
pub async fn rescan_quotas(state: &AppState) -> AppResult<usize> {
    let default = state.identity.default_quota();
    let mut n = 0;
    for uid in quota::limited_users(&state.db, default).await? {
        match measure_usage(state, uid).await {
            Ok(Some(bytes)) => {
                quota::set_used(&state.db, uid, bytes, default).await?;
                n += 1;
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(user = uid, error = %e, "measuring quota usage failed"),
        }
    }
    Ok(n)
}

/// Starts the identity housekeeping: hourly purge of long-expired API keys
/// and the quota rescan every `quota_rescan_hours`.
pub fn spawn_maintenance(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3600));
        let mut hours: u64 = 0;
        loop {
            tick.tick().await;
            match apikey::purge_expired(&state.db, 30 * 86_400).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(count = n, "removed expired API keys"),
                Err(e) => tracing::warn!(error = %e, "purging API keys failed"),
            }
            let every = state.config.quota_rescan_hours;
            if every > 0 && hours.is_multiple_of(every) {
                match rescan_quotas(&state).await {
                    Ok(n) => tracing::debug!(users = n, "quota usage measured"),
                    Err(e) => tracing::warn!(error = %e, "quota rescan failed"),
                }
            }
            hours += 1;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cn_extraction() {
        assert_eq!(
            cn_of("cn=Editors,ou=groups,dc=x").as_deref(),
            Some("Editors")
        );
        assert_eq!(cn_of("CN = admins , OU=x").as_deref(), Some("admins"));
        assert_eq!(cn_of("uid=bob,dc=x"), None);
    }

    #[test]
    fn ceilings() {
        let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        let read = scope_ceiling(&s(&["files:read"]));
        assert!(read.contains(Perms::VIEW | Perms::DOWNLOAD));
        assert!(!read.contains(Perms::DELETE));
        assert!(!read.contains(Perms::UPLOAD));
        assert_eq!(scope_ceiling(&s(&["admin"])), Perms::NONE);
        assert_eq!(scope_ceiling(&s(&["bogus"])), Perms::NONE);
    }
}
