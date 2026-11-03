//! WebDAV at `/dav`, on the same users, permissions and sources as the web UI.
//!
//! The protocol lives in the `filebrary-webdav` crate; this module plugs it
//! into the server:
//!
//! * **Authentication** is HTTP Basic (what Windows Explorer, macOS Finder
//!   and rclone send) checked against the user table with argon2id. Failed
//!   attempts are rate limited per client address with the same
//!   `login_max_attempts` / `login_window_secs` settings as the web login.
//!   A verified password is remembered for [`CRED_CACHE_TTL`] under an HMAC
//!   of the credentials, so a client that sends Basic on every request (all
//!   of them do) does not pay for an argon2 hash each time. The user and
//!   their grants are still loaded from the database on every request, so a
//!   disabled user or changed permission applies at once.
//! * The web session cookie is never accepted here: WebDAV methods are not
//!   covered by the CSRF check, and browsers attach cookies cross-site.
//! * **Permissions** map one to one: see [`filebrary_webdav::DavPerm`].
//! * **Storage** comes from [`crate::api::files::scoped`]'s rule: the source
//!   backend confined to the user's scope with `Storage::sub`, used with
//!   user-visible paths.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axum::http::HeaderMap;
use filebrary_webdav::{
    AuthError, BasicCredentials, Change, DavAuth, DavConfig, DavPerm, DavServer, DavUser, Denied,
};

use crate::{
    activity::{Activity, action},
    auth::{Perms, User, load_user, password},
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
        _ => Denied::Forbidden,
    }
}

/// The URL prefix WebDAV is served under.
pub const PREFIX: &str = "/dav";

/// How long a verified Basic credential is remembered.
pub const CRED_CACHE_TTL: Duration = Duration::from_secs(300);

/// Most remembered credentials (oldest are dropped first).
const CRED_CACHE_MAX: usize = 1024;

/// A hash of a random password, verified against when the user name does not
/// exist so that the response time does not reveal which names exist.
static DUMMY_HASH: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();

async fn dummy_hash() -> String {
    DUMMY_HASH
        .get_or_init(|| async {
            password::hash(crypto::random_token::<16>())
                .await
                .unwrap_or_default()
        })
        .await
        .clone()
}

/// Builds the WebDAV router for the app.
pub fn router(state: &AppState) -> axum::Router<AppState> {
    let auth = Arc::new(FilebraryDavAuth::new(state.clone()));
    let config = DavConfig {
        prefix: PREFIX.into(),
        realm: "Filebrary".into(),
    };
    DavServer::new(auth, config).router()
}

/// [`DavAuth`] over the Filebrary user database.
pub struct FilebraryDavAuth {
    state: AppState,
    failures: Mutex<HashMap<IpAddr, (u32, Instant)>>,
    verified: Mutex<HashMap<String, (i64, Instant)>>,
}

impl FilebraryDavAuth {
    /// Creates the authenticator.
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            failures: Mutex::default(),
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

    fn window(&self) -> Duration {
        Duration::from_secs(self.state.config.login_window_secs)
    }

    /// Seconds to wait if this client has used up its attempts.
    fn limited(&self, ip: Option<IpAddr>) -> Option<u64> {
        let ip = ip?;
        let mut f = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        let (count, since) = *f.get(&ip)?;
        let elapsed = since.elapsed();
        if elapsed >= self.window() {
            f.remove(&ip);
            return None;
        }
        (count >= self.state.config.login_max_attempts)
            .then(|| (self.window() - elapsed).as_secs().max(1))
    }

    fn record_failure(&self, ip: Option<IpAddr>) {
        let Some(ip) = ip else { return };
        let window = self.window();
        let mut f = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        if f.len() > 10_000 {
            f.retain(|_, (_, since)| since.elapsed() < window);
        }
        let e = f.entry(ip).or_insert((0, Instant::now()));
        if e.1.elapsed() >= window {
            *e = (0, Instant::now());
        }
        e.0 += 1;
    }

    fn cache_key(&self, c: &BasicCredentials) -> String {
        let msg = format!("{}\0{}", c.username.to_lowercase(), c.password);
        crypto::hmac_b64(&self.state.secret, msg.as_bytes())
    }

    /// Checks a user name and password, returning the user id.
    async fn verify(&self, c: &BasicCredentials) -> Result<Option<i64>, AppError> {
        let key = self.cache_key(c);
        {
            let mut v = self.verified.lock().unwrap_or_else(|e| e.into_inner());
            match v.get(&key) {
                Some((id, at)) if at.elapsed() < CRED_CACHE_TTL => return Ok(Some(*id)),
                Some(_) => {
                    v.remove(&key);
                }
                None => {}
            }
        }
        let row: Option<(i64, String)> =
            sqlx::query_as("SELECT id, password_hash FROM users WHERE username = ?")
                .bind(&c.username)
                .fetch_optional(&self.state.db)
                .await?;
        let (id, hash) = match row {
            Some((id, hash)) => (Some(id), hash),
            None => (None, dummy_hash().await),
        };
        let ok = password::verify(c.password.clone(), hash).await;
        let Some(id) = id.filter(|_| ok) else {
            return Ok(None);
        };
        let mut v = self.verified.lock().unwrap_or_else(|e| e.into_inner());
        if v.len() >= CRED_CACHE_MAX {
            v.retain(|_, (_, at)| at.elapsed() < CRED_CACHE_TTL);
            if v.len() >= CRED_CACHE_MAX {
                v.clear();
            }
        }
        v.insert(key, (id, Instant::now()));
        Ok(Some(id))
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
        if let Some(retry_after) = self.limited(ip) {
            return Err(AuthError::RateLimited { retry_after });
        }
        let creds = BasicCredentials::from_headers(headers).ok_or(AuthError::Unauthorized)?;
        let id = match self.verify(&creds).await {
            Ok(Some(id)) => id,
            Ok(None) => {
                self.record_failure(ip);
                tracing::info!(user = %creds.username, ?ip, "webdav login failed");
                return Err(AuthError::Unauthorized);
            }
            Err(e) => {
                tracing::error!(error = %e, "webdav authentication");
                return Err(AuthError::Internal);
            }
        };
        match load_user(&self.state, id).await {
            Ok(Some(user)) => Ok(Arc::new(FilebraryDavUser {
                user,
                ip: ip.map(|i| i.to_string()),
            })),
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
    ) -> Result<(), Denied> {
        // Only our own users reach here; anything else is refused.
        let u = ours(user).ok_or(Denied::NotFound)?;
        // `confine` checks the permission, resolves symlinks inside the scope
        // and refuses the trash folder, exactly as the files API does.
        organize::confine(&self.state, &u.user, source, perm(p), path)
            .await
            .map(|_| ())
            .map_err(|e| denied(&e))
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

    #[tokio::test]
    async fn dummy_hash_is_a_real_hash() {
        assert!(dummy_hash().await.starts_with("$argon2id$"));
    }

    #[test]
    fn perms_map() {
        assert_eq!(perm(DavPerm::Edit), Perms::EDIT);
        assert_eq!(perm(DavPerm::Delete), Perms::DELETE);
    }
}
