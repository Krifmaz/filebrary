//! Connects the optional FTP server to Filebrary's users and sources.
//!
//! * Login checks the same argon2id password hash as the web login. Unknown
//!   users are checked against a dummy hash so timing does not reveal which
//!   names exist. Disabled accounts cannot log in.
//! * Each command maps to a permission (`view`, `download`, `upload`,
//!   `edit`, `delete`) and gets its backend from [`api::files::scoped`], so
//!   the user is confined to their scope by [`Storage::sub`] exactly as in
//!   the web API.
//! * Finished uploads publish an `upload` event.
//!
//! The account is loaded at login: permission changes apply from the next
//! login.

use std::{net::IpAddr, sync::Arc};

use async_trait::async_trait;
use filebrary_ftp::{Denied, FtpAccess, FtpAccount, Op};
use tokio::sync::OnceCell;

use crate::{
    api,
    auth::{self, Perms, User, password},
    error::AppError,
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

/// A valid argon2id hash nobody knows the password of, for unknown users.
static DUMMY_HASH: OnceCell<String> = OnceCell::const_new();

#[async_trait]
impl FtpAccess for Access {
    async fn login(
        &self,
        username: &str,
        password_text: &str,
        _ip: IpAddr,
    ) -> Option<Arc<dyn FtpAccount>> {
        let row: Option<(i64, String)> = sqlx::query_as(
            "SELECT id, password_hash FROM users WHERE username = ? AND disabled = 0",
        )
        .bind(username)
        .fetch_optional(&self.state.db)
        .await
        .inspect_err(|e| tracing::warn!(error = %e, "ftp login lookup failed"))
        .ok()?;
        let (id, hash) = match row {
            Some(r) => r,
            None => {
                let dummy = DUMMY_HASH
                    .get_or_try_init(|| password::hash(crate::crypto::random_token::<16>()))
                    .await
                    .ok()?;
                (-1, dummy.clone())
            }
        };
        if !password::verify(password_text.to_owned(), hash).await || id < 0 {
            return None;
        }
        let user = auth::load_user(&self.state, id).await.ok()??;
        Some(Arc::new(Account {
            state: self.state.clone(),
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
        Op::Edit => Perms::EDIT,
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
        api::files::scoped(&self.state, &self.user, source, perm_for(op), path)
            .await
            .map_err(|e| match e {
                AppError::Forbidden(_) => Denied::Forbidden,
                AppError::NotFound => Denied::NotFound,
                other => {
                    tracing::warn!(error = %other, "ftp scope resolution failed");
                    Denied::NotFound
                }
            })
    }

    async fn after_write(&self, source: &str, path: &VPath, size: u64) {
        let real = self
            .user
            .grant(source)
            .map_or_else(|| path.clone(), |g| g.scope.join_path(path));
        self.state.events.publish(filebrary_events::Event::Upload {
            user: self.user.username.clone(),
            source: source.to_owned(),
            path: real.to_string(),
            size,
        });
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
