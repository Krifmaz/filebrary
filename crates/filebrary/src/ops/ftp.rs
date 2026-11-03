//! Connects the optional FTP server to Filebrary's users and sources.
//!
//! * Login checks the same argon2id password hash as the web login and
//!   shares its rate limiter. Unknown users are checked against a dummy hash
//!   so timing does not reveal which names exist. Disabled accounts and
//!   accounts from external sign-in (OIDC, LDAP) cannot log in, and neither
//!   can accounts that need a second factor, since FTP cannot ask for one.
//! * Writes go through [`auth::authorize_write`], so quotas apply.
//! * Each command maps to a permission (`view`, `download`, `upload`,
//!   `edit`, `delete`) and gets its backend from [`api::files::scoped`], so
//!   the user is confined to their scope by [`Storage::sub`] exactly as in
//!   the web API.
//! * Deletes go to the trash bin like web deletes, the trash folder is
//!   hidden, renames carry tags and stars along, and every change is
//!   recorded in the activity log (with `"via": "ftp"`).
//! * Finished uploads publish an `upload` event.
//!
//! The account is loaded at login: permission changes apply from the next
//! login.

use std::{net::IpAddr, sync::Arc};

use async_trait::async_trait;
use filebrary_ftp::{Change, Denied, FtpAccess, FtpAccount, Op, RemoveError};

use crate::{
    activity::{Activity, action},
    api,
    auth::{self, Perms, User, password, users},
    error::AppError,
    organize::{
        self, meta,
        trash::{self, Removed},
    },
    state::AppState,
    storage::{Storage, VPath},
};

/// [`FtpAccess`] over the user table.
#[derive(Clone)]
pub struct Access {
    state: AppState,
}

impl std::fmt::Debug for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FtpAccess")
    }
}

impl Access {
    /// Creates the access checker.
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl FtpAccess for Access {
    async fn login(
        &self,
        username: &str,
        password_text: &str,
        ip: IpAddr,
    ) -> Option<Arc<dyn FtpAccount>> {
        // Same checks as the web login, sharing its rate limiter.
        let state = &self.state;
        let ip = ip.to_string();
        let limiter = &state.auth.limiter;
        limiter.check(username, &ip).ok()?;
        let fail = || {
            limiter.record_failure(username, &ip);
            tracing::info!(username, ip, "failed ftp login");
        };
        if username.len() > users::MAX_USERNAME_LEN || password_text.len() > password::MAX_LEN {
            fail();
            return None;
        }
        let found = users::get_with_hash(&state.db, username)
            .await
            .inspect_err(|e| tracing::warn!(error = %e, "ftp login lookup failed"))
            .ok()?;
        let (rec, hash) = match found {
            Some((rec, Some(hash))) if rec.auth_source == "local" => (Some(rec), hash),
            _ => (None, password::dummy_hash().await),
        };
        let ok = password::verify(password_text.to_owned(), hash).await;
        let Some(rec) = rec.filter(|r| ok && !r.disabled) else {
            fail();
            return None;
        };
        // FTP has no way to ask for a second factor: accounts the hooks do
        // not let in without one (two-factor login) cannot use FTP.
        if state
            .auth
            .hooks()
            .after_password(state, &rec, None)
            .await
            .is_err()
        {
            fail();
            return None;
        }
        limiter.record_success(username);
        let user = auth::load_user(state, rec.id).await.ok()??;
        tracing::info!(user = rec.id, ip, "ftp login");
        Some(Arc::new(Account {
            state: state.clone(),
            user,
        }))
    }
}

/// A logged-in FTP user.
pub struct Account {
    state: AppState,
    user: User,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FtpAccount")
            .field("user", &self.user.username)
            .finish()
    }
}

/// The permission an FTP operation needs.
fn perm_for(op: Op) -> Perms {
    match op {
        Op::View => Perms::VIEW,
        Op::Download => Perms::DOWNLOAD,
        Op::Upload => Perms::UPLOAD,
        Op::Edit | Op::Move => Perms::EDIT,
        Op::Delete => Perms::DELETE,
    }
}

#[async_trait]
impl FtpAccount for Account {
    fn username(&self) -> &str {
        &self.user.username
    }

    fn sources(&self) -> Vec<String> {
        self.user
            .grants
            .iter()
            .filter(|(_, g)| g.perms.contains(Perms::VIEW))
            .map(|(name, _)| name.clone())
            .collect()
    }

    async fn resolve(
        &self,
        source: &str,
        op: Op,
        path: &VPath,
    ) -> Result<Arc<dyn Storage>, Denied> {
        let perm = perm_for(op);
        let scoped = if matches!(op, Op::Move | Op::Delete) {
            // Operations on the entry itself do not follow a final symlink.
            api::files::scoped_entry(&self.state, &self.user, source, perm, path).await
        } else {
            api::files::scoped(&self.state, &self.user, source, perm, path).await
        };
        scoped.map_err(denied)
    }

    async fn before_write(
        &self,
        source: &str,
        op: Op,
        path: &VPath,
        bytes: Option<u64>,
    ) -> Result<(), Denied> {
        // The permission check again plus the write hook (quotas), as for
        // every web upload.
        auth::authorize_write(&self.state, &self.user, source, perm_for(op), path, bytes)
            .await
            .map(drop)
            .map_err(denied)
    }

    fn hide(&self, source: &str, dir: &VPath, name: &str) -> bool {
        // The trash folder at the top of the source is never listed.
        name == trash::TRASH_DIR && self.real(source, dir).is_root()
    }

    async fn remove(
        &self,
        source: &str,
        path: &VPath,
        _storage: &Arc<dyn Storage>,
    ) -> Result<(), RemoveError> {
        // Same as the web API: confine, then move to the trash (or delete
        // when the trash is off) and record the activity.
        let outcome = async {
            let real =
                organize::confine(&self.state, &self.user, source, Perms::DELETE, path).await?;
            let removed = trash::remove(&self.state, Some(self.user.id), source, &real).await?;
            crate::error::AppResult::Ok((real, removed))
        }
        .await;
        match outcome {
            Ok((real, removed)) => {
                let ev = match &removed {
                    Removed::Trashed(item) => Activity::new(action::TRASH)
                        .details(serde_json::json!({ "trash_id": item.id, "via": "ftp" })),
                    Removed::Deleted => {
                        Activity::new(action::DELETE).details(serde_json::json!({ "via": "ftp" }))
                    }
                };
                ev.user(&self.user)
                    .at(source, &real)
                    .record(&self.state)
                    .await;
                Ok(())
            }
            Err(e) => Err(RemoveError::Denied(denied(e))),
        }
    }

    async fn after_change(&self, source: &str, change: Change<'_>) {
        let via = serde_json::json!({ "via": "ftp" });
        match change {
            Change::Upload {
                path,
                size,
                replaced,
            } => {
                let real = self.real(source, path);
                Activity::new(action::UPLOAD)
                    .user(&self.user)
                    .at(source, &real)
                    .details(
                        serde_json::json!({ "size": size, "overwrite": replaced, "via": "ftp" }),
                    )
                    .record(&self.state)
                    .await;
                self.state.events.publish(filebrary_events::Event::Upload {
                    user: self.user.username.clone(),
                    source: source.to_owned(),
                    path: real.to_string(),
                    size,
                });
            }
            Change::Mkdir { path } => {
                Activity::new(action::MKDIR)
                    .user(&self.user)
                    .at(source, &self.real(source, path))
                    .details(via)
                    .record(&self.state)
                    .await;
            }
            Change::Rename { from, to, .. } => {
                let (from, to) = (self.real(source, from), self.real(source, to));
                // Tags and stars follow the item, as in the web API.
                if let Err(e) = meta::on_move(&self.state.db, source, &from, source, &to).await {
                    tracing::warn!(error = %e, "ftp rename: moving tags failed");
                }
                Activity::new(action::MOVE)
                    .user(&self.user)
                    .at(source, &from)
                    .target(&to)
                    .details(via)
                    .record(&self.state)
                    .await;
            }
        }
    }
}

impl Account {
    /// The real path in the source for a user-visible path.
    fn real(&self, source: &str, path: &VPath) -> VPath {
        self.user
            .grant(source)
            .map_or_else(|| path.clone(), |g| g.scope.join_path(path))
    }
}

/// Maps a server error to an FTP refusal.
fn denied(e: AppError) -> Denied {
    match e {
        AppError::Forbidden(_) | AppError::BadRequest(_) => Denied::Forbidden,
        AppError::NotFound => Denied::NotFound,
        AppError::PayloadTooLarge => Denied::QuotaExceeded,
        other => {
            tracing::warn!(error = %other, "ftp operation failed");
            Denied::NotFound
        }
    }
}

/// Starts the FTP server in the background if `ftp.enabled` is set.
///
/// # Errors
///
/// Fails if the FTP configuration is invalid.
pub fn spawn(
    state: &AppState,
    shutdown: impl std::future::Future<Output = ()> + Send + Sync + 'static,
) -> anyhow::Result<Option<tokio::task::JoinHandle<()>>> {
    if !state.config.ftp.enabled {
        return Ok(None);
    }
    let server = filebrary_ftp::FtpServer::new(
        state.config.ftp.clone(),
        Arc::new(Access::new(state.clone())),
    )?;
    Ok(Some(tokio::spawn(async move {
        if let Err(e) = server.run(shutdown).await {
            tracing::error!(error = %e, "ftp server stopped");
        }
    })))
}
