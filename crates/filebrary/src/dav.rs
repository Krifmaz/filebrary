//! WebDAV at `/dav`, on the same users, permissions and sources as the web UI.
//!
//! The protocol lives in the `filebrary-webdav` crate; this module plugs it
//! into the server:
//!
//! * **Authentication** is HTTP Basic (what Windows Explorer, macOS Finder
//!   and rclone send). The password may be the account password of a local
//!   user, or an API key, which works as an app password: it is the way in
//!   for accounts with two-factor authentication or single sign-on, and its
//!   scopes cap what the WebDAV session may do. Failures go through the same
//!   login rate limiter as the web login (per user name and per address).
//!   A verified credential is remembered for [`CRED_CACHE_TTL`] under an
//!   HMAC of it, so a client that sends Basic on every request (all of them
//!   do) does not pay for an argon2 hash each time. The user, their rules
//!   and the key's ceiling are still loaded on every request, so disabling a
//!   user or changing a permission applies at once.
//! * The web session cookie is never accepted here: WebDAV methods are not
//!   covered by the CSRF check, and browsers attach cookies cross-site.
//! * **Permissions** go through [`auth::authorize`] and its tree, entry and
//!   write variants, exactly like the files API: per-folder rules apply to
//!   the symlink-resolved path, deletes and moves check rules deeper in the
//!   tree, and writes pass the quota hook. See [`filebrary_webdav::DavPerm`]
//!   for how methods map to permissions.
//! * **Storage** is the source backend confined to the user's scope with
//!   `Storage::sub`, used with user-visible paths. The trash folder is
//!   hidden and refused; `DELETE` moves items to the trash.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axum::http::HeaderMap;
use filebrary_webdav::{
    Access, AuthError, BasicCredentials, Change, DavAuth, DavConfig, DavPerm, DavServer, DavUser,
    Denied,
};

use crate::{
    activity::{Activity, action},
    auth::{self, Perms, User, load_user, password, users},
    crypto,
    error::AppError,
    organize::{self, meta, trash},
    state::AppState,
    storage::{Storage, StorageError, VPath},
};

fn ours(user: &dyn DavUser) -> Option<&FilebraryDavUser> {
    (user as &dyn std::any::Any).downcast_ref::<FilebraryDavUser>()
}

fn denied(e: &AppError) -> Denied {
    match e {
        AppError::NotFound => Denied::NotFound,
        AppError::PayloadTooLarge => Denied::InsufficientStorage,
        _ => Denied::Forbidden,
    }
}

/// The URL prefix WebDAV is served under.
pub const PREFIX: &str = "/dav";

/// How long a verified Basic credential is remembered.
pub const CRED_CACHE_TTL: Duration = Duration::from_secs(300);

/// Most remembered credentials (all are dropped when it fills up).
const CRED_CACHE_MAX: usize = 1024;

/// Builds the WebDAV router for the app.
pub fn router(state: &AppState) -> axum::Router<AppState> {
    let auth = Arc::new(FilebraryDavAuth::new(state.clone()));
    let config = DavConfig {
        prefix: PREFIX.into(),
        realm: "Filebrary".into(),
    };
    DavServer::new(auth, config).router()
}

/// What a verified credential stands for.
#[derive(Clone, Debug)]
struct Verified {
    user_id: i64,
    /// The API key's scopes, or `None` for a password.
    scopes: Option<Vec<String>>,
    at: Instant,
}

/// [`DavAuth`] over the Filebrary user database.
pub struct FilebraryDavAuth {
    state: AppState,
    verified: Mutex<HashMap<String, Verified>>,
}

impl FilebraryDavAuth {
    /// Creates the authenticator.
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            verified: Mutex::default(),
        }
    }

    /// The client address, honouring `X-Forwarded-For` only when
    /// `trust_proxy_headers` is set.
    fn client_ip(&self, headers: &HeaderMap, peer: Option<SocketAddr>) -> Option<IpAddr> {
        if self.state.config.trust_proxy_headers
            && let Some(ip) = headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
                .and_then(|v| v.trim().parse().ok())
        {
            return Some(ip);
        }
        peer.map(|p| p.ip())
    }

    fn cache_key(&self, c: &BasicCredentials) -> String {
        let msg = format!("{}\0{}", c.username.to_lowercase(), c.password);
        crypto::hmac_b64(&self.state.secret, msg.as_bytes())
    }

    fn cached(&self, key: &str) -> Option<Verified> {
        let mut v = self.verified.lock().unwrap_or_else(|e| e.into_inner());
        match v.get(key) {
            Some(hit) if hit.at.elapsed() < CRED_CACHE_TTL => Some(hit.clone()),
            Some(_) => {
                v.remove(key);
                None
            }
            None => None,
        }
    }

    fn remember(&self, key: String, verified: Verified) {
        let mut v = self.verified.lock().unwrap_or_else(|e| e.into_inner());
        if v.len() >= CRED_CACHE_MAX {
            v.retain(|_, hit| hit.at.elapsed() < CRED_CACHE_TTL);
            if v.len() >= CRED_CACHE_MAX {
                v.clear();
            }
        }
        v.insert(key, verified);
    }

    /// Checks the credentials: first as an API key of that user, then as the
    /// local password. `Ok(None)` means wrong credentials.
    async fn verify(&self, c: &BasicCredentials) -> Result<Option<Verified>, AppError> {
        let hooks = self.state.auth.hooks();
        if let Some((user_id, scopes)) = hooks.resolve_bearer(&self.state, &c.password).await? {
            // The key must belong to the user named in the Basic header.
            let owner = users::get_with_hash(&self.state.db, &c.username).await?;
            return Ok(match owner {
                Some((rec, _)) if rec.id == user_id && !rec.disabled => Some(Verified {
                    user_id,
                    scopes: Some(scopes),
                    at: Instant::now(),
                }),
                _ => None,
            });
        }
        let found = users::get_with_hash(&self.state.db, &c.username).await?;
        let (rec, hash) = match found {
            Some((rec, Some(hash))) if rec.auth_source == "local" => (Some(rec), hash),
            // Unknown, external (OIDC/LDAP) or password-less accounts still
            // pay for a hash, so timing does not reveal which names exist.
            _ => (None, password::dummy_hash().await),
        };
        let ok = password::verify(c.password.clone(), hash).await;
        let rec = match rec {
            Some(r) if ok && !r.disabled => r,
            _ => return Ok(None),
        };
        // Two-factor accounts cannot answer a code over Basic auth: they
        // must use an API key instead.
        match hooks.after_password(&self.state, &rec, None).await {
            Ok(()) => {}
            Err(AppError::TotpRequired) => {
                tracing::info!(
                    user = %rec.username,
                    "webdav: password login refused for a two-factor account; use an API key"
                );
                return Ok(None);
            }
            Err(AppError::InvalidCredentials | AppError::Unauthorized) => return Ok(None),
            Err(e) => return Err(e),
        }
        Ok(Some(Verified {
            user_id: rec.id,
            scopes: None,
            at: Instant::now(),
        }))
    }
}

#[async_trait]
impl DavAuth for FilebraryDavAuth {
    async fn authenticate(
        &self,
        headers: &HeaderMap,
        client: Option<SocketAddr>,
    ) -> Result<Arc<dyn DavUser>, AuthError> {
        let ip = self.client_ip(headers, client);
        let ip_str = ip.map_or_else(|| "unknown".to_owned(), |i| i.to_string());
        let creds = BasicCredentials::from_headers(headers).ok_or(AuthError::Unauthorized)?;
        let limiter = &self.state.auth.limiter;
        let key = self.cache_key(&creds);
        let verified = match self.cached(&key) {
            Some(v) => v,
            None => {
                if let Err(AppError::TooManyRequests { retry_after }) =
                    limiter.check(&creds.username, &ip_str)
                {
                    return Err(AuthError::RateLimited { retry_after });
                }
                let too_long = creds.username.len() > users::MAX_USERNAME_LEN
                    || creds.password.len() > password::MAX_LEN;
                let result = if too_long {
                    Ok(None)
                } else {
                    self.verify(&creds).await
                };
                match result {
                    Ok(Some(v)) => {
                        limiter.record_success(&creds.username);
                        self.remember(key, v.clone());
                        v
                    }
                    Ok(None) => {
                        limiter.record_failure(&creds.username, &ip_str);
                        tracing::info!(user = %creds.username, ip = %ip_str, "webdav login failed");
                        return Err(AuthError::Unauthorized);
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "webdav authentication");
                        return Err(AuthError::Internal);
                    }
                }
            }
        };
        match load_user(&self.state, verified.user_id).await {
            Ok(Some(mut user)) => {
                if let Some(scopes) = &verified.scopes {
                    user.apply_ceiling(self.state.auth.hooks().bearer_ceiling(scopes));
                }
                Ok(Arc::new(FilebraryDavUser {
                    user,
                    ip: ip.map(|i| i.to_string()),
                }))
            }
            // Disabled or deleted since the credentials were cached.
            Ok(None) => Err(AuthError::Unauthorized),
            Err(e) => {
                tracing::error!(error = %e, "webdav loading user");
                Err(AuthError::Internal)
            }
        }
    }

    async fn authorize(
        &self,
        user: &dyn DavUser,
        source: &str,
        p: DavPerm,
        path: &VPath,
        access: Access,
    ) -> Result<(), Denied> {
        // Only our own users reach here; anything else is refused.
        let u = ours(user).ok_or(Denied::NotFound)?;
        let (state, user, perm) = (&self.state, &u.user, perm(p));
        let real = match access {
            Access::Read => auth::authorize(state, user, source, perm, path).await,
            Access::Tree { follow } => {
                auth::authorize_tree(state, user, source, perm, path, follow).await
            }
            Access::Write { bytes } => {
                auth::authorize_write(state, user, source, perm, path, bytes).await
            }
        }
        .map_err(|e| denied(&e))?;
        if trash::is_reserved(&real) {
            return Err(Denied::NotFound);
        }
        Ok(())
    }

    fn hidden(&self, _source: &str, real: &VPath) -> bool {
        trash::is_reserved(real)
    }

    async fn remove(
        &self,
        user: &dyn DavUser,
        source: &str,
        path: &VPath,
        _storage: &Arc<dyn Storage>,
    ) -> Result<Option<String>, StorageError> {
        // Deletes go to the trash, like the web UI. The trash lives at the
        // source root, outside the user's scope, so confine the path first.
        let u = ours(user).ok_or(StorageError::NotFound)?;
        let to_storage = |e: AppError| match e {
            AppError::NotFound => StorageError::NotFound,
            AppError::Forbidden(m) | AppError::BadRequest(m) => StorageError::Unsupported(m),
            other => StorageError::Io(std::io::Error::other(other.to_string())),
        };
        let real = organize::confine(&self.state, &u.user, source, Perms::DELETE, path)
            .await
            .map_err(to_storage)?;
        match trash::remove(&self.state, Some(u.user.id), source, &real)
            .await
            .map_err(to_storage)?
        {
            trash::Removed::Trashed(item) => Ok(Some(item.id)),
            trash::Removed::Deleted => Ok(None),
        }
    }

    async fn storage(&self, user: &dyn DavUser, source: &str) -> Option<Arc<dyn Storage>> {
        let src = self.state.source(source)?;
        let scope = user.scope(source)?;
        if scope.is_root() {
            Some(src.storage.clone())
        } else {
            match src.storage.sub(&scope).await {
                Ok(s) => Some(s),
                Err(e) => {
                    tracing::warn!(%source, %scope, error = %e, "webdav: cannot open user scope");
                    None
                }
            }
        }
    }

    async fn on_change(&self, user: &dyn DavUser, change: &Change) {
        let Some(u) = ours(user) else { return };
        let db = &self.state.db;
        // Tags and stars follow files, as they do through the web UI.
        let meta_result = match change {
            Change::Moved {
                source,
                from,
                to_source,
                to,
            } => meta::on_move(db, source, from, to_source, to).await,
            Change::Copied {
                source,
                from,
                to_source,
                to,
            } => meta::on_copy(db, source, from, to_source, to).await,
            Change::Deleted {
                source,
                path,
                replaced: true,
                ..
            } => meta::on_delete(db, source, path).await,
            _ => Ok(()),
        };
        if let Err(e) = meta_result {
            tracing::error!(error = %e, "webdav: updating tags and stars");
        }
        let ev = match change {
            Change::Written { source, path, .. } => Activity::new(action::UPLOAD).at(source, path),
            Change::CreatedCollection { source, path } => {
                Activity::new(action::MKDIR).at(source, path)
            }
            Change::Deleted {
                source,
                path,
                trash_id: Some(id),
                ..
            } => Activity::new(action::TRASH)
                .at(source, path)
                .details(serde_json::json!({ "trash_id": id, "via": "webdav" })),
            Change::Deleted { source, path, .. } => Activity::new(action::DELETE).at(source, path),
            Change::Moved {
                source, from, to, ..
            } => Activity::new(action::MOVE).at(source, from).target(to),
            Change::Copied {
                source, from, to, ..
            } => Activity::new(action::COPY).at(source, from).target(to),
            // Dead properties (Explorer and Finder metadata) are not worth an event.
            _ => return,
        };
        ev.user(&u.user)
            .ip(u.ip.as_deref())
            .record(&self.state)
            .await;
    }
}

/// A Filebrary user as the WebDAV server sees them.
pub struct FilebraryDavUser {
    user: User,
    /// The client address, for the activity log.
    ip: Option<String>,
}

fn perm(p: DavPerm) -> Perms {
    match p {
        DavPerm::View => Perms::VIEW,
        DavPerm::Download => Perms::DOWNLOAD,
        DavPerm::Upload => Perms::UPLOAD,
        DavPerm::Edit => Perms::EDIT,
        DavPerm::Delete => Perms::DELETE,
    }
}

impl DavUser for FilebraryDavUser {
    fn id(&self) -> String {
        self.user.id.to_string()
    }

    fn sources(&self) -> Vec<String> {
        self.user
            .grants
            .iter()
            .filter(|(_, g)| g.perms.contains(Perms::VIEW))
            .map(|(name, _)| name.clone())
            .collect()
    }

    fn scope(&self, source: &str) -> Option<VPath> {
        self.user
            .grant(source)
            .filter(|g| g.perms.contains(Perms::VIEW))
            .map(|g| g.scope.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perms_map() {
        assert_eq!(perm(DavPerm::Edit), Perms::EDIT);
        assert_eq!(perm(DavPerm::Delete), Perms::DELETE);
    }
}
