//! The contract between the FTP server and the rest of Filebrary.

use std::{fmt::Debug, net::IpAddr, sync::Arc};

use async_trait::async_trait;
use filebrary_storage::{Storage, StorageError, VPath};

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
    /// uploads, or replacing the target of `RNFR`/`RNTO`).
    Edit,
    /// Move or rename an entry away from its path (the `RNFR` side of a
    /// rename). Needs the same permission as [`Op::Edit`], but acts on the
    /// entry itself: a final symlink is not followed.
    Move,
    /// Remove a file or folder (`DELE`, `RMD`). Acts on the entry itself:
    /// a final symlink is not followed.
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

/// A change made over FTP, reported to [`FtpAccount::after_change`] once it
/// succeeded. Paths are the ones the user sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change<'a> {
    /// A file was written (`STOR`); `size` is the final file size.
    Upload {
        /// The file.
        path: &'a VPath,
        /// Its size in bytes.
        size: u64,
        /// Whether an existing file was replaced or resumed.
        replaced: bool,
    },
    /// A folder was created (`MKD`).
    Mkdir {
        /// The folder.
        path: &'a VPath,
    },
    /// A file or folder was renamed or moved (`RNFR`/`RNTO`).
    Rename {
        /// The old path.
        from: &'a VPath,
        /// The new path.
        to: &'a VPath,
        /// Whether an existing item at `to` was replaced.
        replaced: bool,
    },
}

/// Why [`FtpAccount::remove`] failed.
#[derive(Debug, thiserror::Error)]
pub enum RemoveError {
    /// Refused by a permission or visibility rule.
    #[error(transparent)]
    Denied(#[from] Denied),
    /// The storage operation failed.
    #[error(transparent)]
    Storage(#[from] StorageError),
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

    /// Called after [`resolve`](Self::resolve) allowed a write and before
    /// any bytes are written, so quotas can be enforced. `op` is the
    /// operation that was allowed ([`Op::Upload`] or [`Op::Edit`]), `path`
    /// the user-visible file, and `bytes` the size about to be written when
    /// the client announced it (FTP usually does not). The default allows
    /// every write.
    ///
    /// # Errors
    ///
    /// [`Denied::QuotaExceeded`] to refuse the upload.
    async fn before_write(
        &self,
        _source: &str,
        _op: Op,
        _path: &VPath,
        _bytes: Option<u64>,
    ) -> Result<(), Denied> {
        Ok(())
    }

    /// Removes a file or folder (after the permission and type checks).
    /// Override it to send deletes to a trash bin; `storage` is the
    /// confined backend from [`resolve`](Self::resolve). The default deletes
    /// permanently.
    ///
    /// # Errors
    ///
    /// [`RemoveError`] when the item cannot be removed.
    async fn remove(
        &self,
        _source: &str,
        path: &VPath,
        storage: &Arc<dyn Storage>,
    ) -> Result<(), RemoveError> {
        Ok(storage.delete(path).await?)
    }

    /// Whether to leave the entry `name` out of the listing of folder `dir`
    /// (a user-visible path) in `source`, for example a reserved trash
    /// folder. The default shows everything.
    fn hide(&self, _source: &str, _dir: &VPath, _name: &str) -> bool {
        false
    }

    /// Called after a change succeeded. Use it to publish events, record
    /// activity, keep tags in step and update quota usage. The default does
    /// nothing.
    async fn after_change(&self, _source: &str, _change: Change<'_>) {}
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
