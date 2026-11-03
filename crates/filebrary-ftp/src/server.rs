//! Building and running the FTP server.

use std::{
    collections::HashMap,
    fmt,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use libunftp::{
    ServerBuilder,
    options::{FailedLoginsBlock, FailedLoginsPolicy, FtpsRequired, Shutdown, SiteMd5},
};
use unftp_core::auth::{
    AuthenticationError, Authenticator, Credentials, Principal, UserDetailError, UserDetailProvider,
};

use crate::{
    FtpConfig, FtpsMode,
    access::{FtpAccess, FtpAccount},
    backend::{Backend, FtpUser},
};

/// Errors starting the FTP server.
#[derive(Debug, thiserror::Error)]
pub enum FtpError {
    /// The configuration is invalid.
    #[error("invalid FTP configuration: {0}")]
    Config(String),
    /// libunftp refused to build or bind the server.
    #[error("FTP server: {0}")]
    Server(String),
}

/// Accounts that passed the password check, waiting for libunftp to ask for
/// their details. libunftp authenticates and loads the user in two steps
/// that only share the user name.
type Pending = Arc<Mutex<HashMap<String, Arc<dyn FtpAccount>>>>;

struct Auth {
    access: Arc<dyn FtpAccess>,
    pending: Pending,
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FilebraryFtpAuth")
    }
}

#[async_trait]
impl Authenticator for Auth {
    async fn authenticate(
        &self,
        username: &str,
        creds: &Credentials,
    ) -> Result<Principal, AuthenticationError> {
        let Some(password) = creds.password.as_deref() else {
            return Err(AuthenticationError::BadPassword);
        };
        match self.access.login(username, password, creds.source_ip).await {
            Some(account) => {
                metrics::counter!("filebrary_ftp_logins_total", "result" => "success").increment(1);
                tracing::info!(user = username, ip = %creds.source_ip, "ftp login");
                self.pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(username.to_owned(), account);
                Ok(Principal {
                    username: username.to_owned(),
                })
            }
            None => {
                metrics::counter!("filebrary_ftp_logins_total", "result" => "failure").increment(1);
                tracing::warn!(user = username, ip = %creds.source_ip, "ftp login failed");
                Err(AuthenticationError::BadPassword)
            }
        }
    }
}

struct Details {
    pending: Pending,
}

impl fmt::Debug for Details {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FilebraryFtpDetails")
    }
}

#[async_trait]
impl UserDetailProvider for Details {
    type User = FtpUser;

    async fn provide_user_detail(&self, p: &Principal) -> Result<FtpUser, UserDetailError> {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&p.username)
            .map(|account| FtpUser { account })
            .ok_or_else(|| UserDetailError::UserNotFound {
                username: p.username.clone(),
            })
    }
}

/// A configured FTP server, ready to [`run`](FtpServer::run).
pub struct FtpServer {
    config: FtpConfig,
    access: Arc<dyn FtpAccess>,
}

impl FtpServer {
    /// Prepares a server. Call [`run`](Self::run) to listen.
    ///
    /// # Errors
    ///
    /// Fails if the configuration is invalid.
    pub fn new(config: FtpConfig, access: Arc<dyn FtpAccess>) -> Result<Self, FtpError> {
        config.validate().map_err(FtpError::Config)?;
        Ok(Self { config, access })
    }

    /// Listens on the configured address until `shutdown` resolves, then
    /// gives open sessions five seconds to finish.
    ///
    /// # Errors
    ///
    /// Fails if the server cannot be built (bad certificate) or bound.
    pub async fn run<F>(self, shutdown: F) -> Result<(), FtpError>
    where
        F: Future<Output = ()> + Send + Sync + 'static,
    {
        let cfg = self.config;
        let pending: Pending = Arc::default();
        let auth = Arc::new(Auth {
            access: self.access,
            pending: pending.clone(),
        });
        // libunftp keeps the greeting as `&'static str`. This runs once per
        // process, so leaking the short string is fine.
        let greeting: &'static str = Box::leak(cfg.greeting.clone().into_boxed_str());
        let [lo, hi] = cfg.passive_ports;
        let mut builder = ServerBuilder::with_user_detail_provider(
            Box::new(|| Backend),
            Arc::new(Details { pending }),
        )
        .authenticator(auth)
        // SITE MD5 hashes whole files on request: an easy way to burn CPU.
        .sitemd5(SiteMd5::None)
        .greeting(greeting)
        .passive_ports(lo..=hi)
        .idle_session_timeout(cfg.idle_timeout_secs)
        .failed_logins_policy(FailedLoginsPolicy::new(
            cfg.max_login_attempts,
            Duration::from_secs(cfg.login_block_secs),
            FailedLoginsBlock::IP,
        ))
        .shutdown_indicator(async move {
            shutdown.await;
            Shutdown::new().grace_period(Duration::from_secs(5))
        });
        if let Some(host) = &cfg.passive_host {
            builder = builder.passive_host(host.as_str());
        }
        match &cfg.tls {
            Some(tls) => {
                let required = match tls.mode {
                    FtpsMode::Required => FtpsRequired::All,
                    FtpsMode::Optional => FtpsRequired::None,
                };
                builder = builder
                    .ftps(tls.cert.clone(), tls.key.clone())
                    .ftps_required(required, required);
            }
            None => {
                tracing::warn!(
                    "FTP is running without TLS: passwords and files travel in clear text. \
                     Set [ftp.tls] to enable FTPS."
                );
            }
        }
        let server = builder
            .build()
            .map_err(|e| FtpError::Server(e.to_string()))?;
        tracing::info!(listen = %cfg.listen, tls = cfg.tls.is_some(), "ftp server listening");
        server
            .listen(cfg.listen.to_string())
            .await
            .map_err(|e| FtpError::Server(e.to_string()))
    }
}
