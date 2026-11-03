//! The contract between the FTP server and the rest of Filebrary.

use std::{fmt::Debug, net::IpAddr, sync::Arc};

use async_trait::async_trait;
use filebrary_storage::{Storage, VPath};

/// What a client wants to do with a path. The server crate maps each one to
/// a permission (`view`, `download`, `upload`, `edit`, `delete`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// List a folder or read metadata (`LIST`, `NLST`, `MLSD`, `SIZE`,
    /// `MDTM`, `CWD`).
    View,
    /// Read file contents (`RETR`).
    Download,
    /// Create a new file or folder (`STOR` of a new name, `MKD`).
    Upload,
    /// Change an existing file (`STOR` over an existing file, resumed
    /// uploads, `RNFR`/`RNTO`).
    Edit,
    /// Remove a file or folder (`DELE`, `RMD`).
    Delete,
}

/// Why an operation was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Denied {
    /// The source does not exist or the user may not see it. Reported to the
    /// client as "file not available" so hidden sources stay hidden.
    #[error("not found")]
    NotFound,
    /// The user may see the path but lacks the permission for the operation.
    #[error("permission denied")]
    Forbidden,
    /// The user is over their storage quota.
    #[error("quota exceeded")]
    QuotaExceeded,
}

/// A logged-in FTP user.
///
/// Implementations are expected to be cheap to call: they are consulted on
/// every command.
#[async_trait]
pub trait FtpAccount: Send + Sync + Debug + 'static {
    /// The login name, for logs.
    fn username(&self) -> &str;

    /// Names of the sources this user may view, shown as top-level folders.
    fn sources(&self) -> Vec<String>;

    /// Checks that the user may do `op` on `path` (as the user sees it) in
    /// `source`, and returns the source's backend *confined to the user's
    /// scope* (for example with [`Storage::sub`]). The FTP server passes the
    /// same user-visible `path` to the returned backend, so the backend, not
    /// string prefixing, keeps the user inside their folder, including
    /// against symlinks that point elsewhere in the source.
    ///
    /// # Errors
    ///
    /// [`Denied`] when the source is hidden or the permission is missing.
    async fn resolve(&self, source: &str, op: Op, path: &VPath)
    -> Result<Arc<dyn Storage>, Denied>;

    /// Called before a file is written, so quotas can be enforced.
    /// The default allows every write.
    ///
    /// # Errors
    ///
    /// [`Denied::QuotaExceeded`] to refuse the upload.
    async fn before_write(&self, _source: &str) -> Result<(), Denied> {
        Ok(())
    }

    /// Called after a file was written successfully. `path` is the path the
    /// user sees, `size` the final file size. Use it to publish an upload
    /// event and update quota usage. The default does nothing.
    async fn after_write(&self, _source: &str, _path: &VPath, _size: u64) {}
}

/// Logs users in. Implemented by the server crate on top of its user table.
#[async_trait]
pub trait FtpAccess: Send + Sync + Debug + 'static {
    /// Checks a user name and password from a client at `ip`. Return `None`
    /// for an unknown user, a wrong password or a disabled account; the
    /// client is told the same thing in every case.
    ///
    /// Repeated failures are rate limited by the FTP server itself (see
    /// [`crate::FtpConfig::max_login_attempts`]).
    async fn login(
        &self,
        username: &str,
        password: &str,
        ip: IpAddr,
    ) -> Option<Arc<dyn FtpAccount>>;
}
