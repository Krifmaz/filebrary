//! The libunftp storage backend: maps the FTP namespace onto sources.
//!
//! The FTP namespace has a virtual root that lists one folder per visible
//! source. A client path is turned into a [`Target`] by [`parse_path`]:
//!
//! 1. Split on `/` and `\` (libunftp joins paths with the platform
//!    separator; no valid file name contains either character).
//! 2. Drop empty and `.` components and apply `..` lexically, never above
//!    the virtual root. This happens on the *virtual* path, before any
//!    source is chosen, so `..` cannot step from one source into another
//!    or out of a user's scope.
//! 3. The first component names the source; the rest goes through
//!    [`VPath::parse`], which rejects NUL bytes, reserved names and anything
//!    else that is not a plain file name.
//!
//! The account then checks permissions and hands back the source's backend
//! confined to the user's scope (see [`FtpAccount::resolve`]); that backend
//! receives the user-visible path and itself refuses symlinks that lead
//! outside the scope.

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use async_trait::async_trait;
use filebrary_storage::{ByteReader, Entry, Storage, StorageError, VPath};
use futures::TryStreamExt;
use sync_wrapper::SyncStream;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::io::StreamReader;
use unftp_core::{
    auth::UserDetail,
    storage::{Error, ErrorKind, FEATURE_RESTART, Fileinfo, Metadata, StorageBackend},
};

use crate::access::{Change, Denied, FtpAccount, Op, RemoveError};

/// The libunftp user type: a logged-in account.
#[derive(Clone)]
pub struct FtpUser {
    /// The account, as returned by [`crate::FtpAccess::login`].
    pub account: Arc<dyn FtpAccount>,
}

impl fmt::Debug for FtpUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FtpUser")
            .field("username", &self.account.username())
            .finish()
    }
}

impl fmt::Display for FtpUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.account.username())
    }
}

impl UserDetail for FtpUser {}

/// Where a client path points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The virtual root that lists the sources.
    Root,
    /// A path inside a source, as the user sees it (the account adds the
    /// scope). `VPath::root()` is the top of the source.
    Source(String, VPath),
}

/// Normalises a client path. See the module documentation for the rules.
///
/// # Errors
///
/// Fails on non-UTF-8 paths and on components [`VPath::parse`] rejects.
pub fn parse_path(path: &Path) -> Result<Target, Error> {
    let raw = path
        .to_str()
        .ok_or_else(|| Error::from(ErrorKind::PermanentFileNotAvailable))?;
    let mut parts: Vec<&str> = Vec::new();
    for comp in raw.split(['/', '\\']) {
        match comp {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    let Some((source, rest)) = parts.split_first() else {
        return Ok(Target::Root);
    };
    let inner = VPath::parse(&rest.join("/")).map_err(storage_error)?;
    Ok(Target::Source((*source).to_owned(), inner))
}

/// File metadata as libunftp wants it.
#[derive(Clone, Debug)]
pub struct Meta {
    len: u64,
    dir: bool,
    modified: Option<SystemTime>,
}

impl Meta {
    fn dir() -> Self {
        Self {
            len: 0,
            dir: true,
            modified: None,
        }
    }
}

impl From<&Entry> for Meta {
    fn from(e: &Entry) -> Self {
        Self {
            len: e.size,
            dir: e.is_dir(),
            modified: e.modified.map(SystemTime::from),
        }
    }
}

impl Metadata for Meta {
    fn len(&self) -> u64 {
        self.len
    }
    fn is_dir(&self) -> bool {
        self.dir
    }
    fn is_file(&self) -> bool {
        !self.dir
    }
    fn is_symlink(&self) -> bool {
        false
    }
    fn modified(&self) -> unftp_core::storage::Result<SystemTime> {
        self.modified
            .ok_or_else(|| Error::from(ErrorKind::PermanentFileNotAvailable))
    }
    fn gid(&self) -> u32 {
        0
    }
    fn uid(&self) -> u32 {
        0
    }
}

/// Maps a storage error to an FTP reply.
fn storage_error(e: StorageError) -> Error {
    let kind = match &e {
        StorageError::NotFound
        | StorageError::AlreadyExists
        | StorageError::InvalidPath(_)
        | StorageError::IsADirectory
        | StorageError::InvalidRange { .. } => ErrorKind::PermanentFileNotAvailable,
        StorageError::NotADirectory => ErrorKind::PermanentDirectoryNotAvailable,
        StorageError::OutsideRoot | StorageError::Unsupported(_) => ErrorKind::PermissionDenied,
        StorageError::Io(io) => {
            tracing::warn!(error = %io, "ftp storage error");
            ErrorKind::LocalError
        }
    };
    Error::new(kind, e)
}

fn denied(d: Denied) -> Error {
    let kind = match d {
        Denied::NotFound => ErrorKind::PermanentFileNotAvailable,
        Denied::Forbidden => ErrorKind::PermissionDenied,
        Denied::QuotaExceeded => ErrorKind::InsufficientStorageSpaceError,
    };
    Error::new(kind, d)
}

fn remove_error(e: RemoveError) -> Error {
    match e {
        RemoveError::Denied(d) => denied(d),
        RemoveError::Storage(s) => storage_error(s),
    }
}

fn perm() -> Error {
    Error::from(ErrorKind::PermissionDenied)
}

/// Resolves a client path inside a source for `op`: the source name, the
/// user-visible path and the backend confined to the user's scope.
async fn resolve(
    user: &FtpUser,
    path: &Path,
    op: Op,
) -> Result<(String, VPath, Arc<dyn Storage>), Error> {
    match parse_path(path)? {
        Target::Root => Err(perm()),
        Target::Source(source, visible) => {
            let storage = user
                .account
                .resolve(&source, op, &visible)
                .await
                .map_err(denied)?;
            Ok((source, visible, storage))
        }
    }
}

/// Like [`resolve`], but refuses the top of a source (it cannot be
/// created, removed or renamed over FTP).
async fn resolve_inner(
    user: &FtpUser,
    path: &Path,
    op: Op,
) -> Result<(String, VPath, Arc<dyn Storage>), Error> {
    let r = resolve(user, path, op).await?;
    if r.1.is_root() {
        return Err(perm());
    }
    Ok(r)
}

async fn stat_opt(storage: &dyn Storage, path: &VPath) -> Result<Option<Entry>, Error> {
    match storage.stat(path).await {
        Ok(e) => Ok(Some(e)),
        Err(StorageError::NotFound) => Ok(None),
        Err(e) => Err(storage_error(e)),
    }
}

fn reader(stream: filebrary_storage::ByteStream) -> impl AsyncRead + Send + Sync + Unpin {
    // libunftp wants a `Sync` reader; the storage stream is only `Send`.
    // `SyncStream` is sound because the reader is only used through `&mut`.
    StreamReader::new(SyncStream::new(stream.map_err(std::io::Error::other)))
}

/// The libunftp storage backend. Stateless: everything it needs comes from
/// the [`FtpUser`] passed with each call.
#[derive(Clone, Copy, Debug, Default)]
pub struct Backend;

#[async_trait]
impl StorageBackend<FtpUser> for Backend {
    type Metadata = Meta;

    fn name(&self) -> &str {
        "filebrary"
    }

    fn supported_features(&self) -> u32 {
        FEATURE_RESTART
    }

    async fn metadata<P: AsRef<Path> + Send + fmt::Debug>(
        &self,
        user: &FtpUser,
        path: P,
    ) -> unftp_core::storage::Result<Meta> {
        if parse_path(path.as_ref())? == Target::Root {
            return Ok(Meta::dir());
        }
        let (_, vp, storage) = resolve(user, path.as_ref(), Op::View).await?;
        let e = storage.stat(&vp).await.map_err(storage_error)?;
        Ok(Meta::from(&e))
    }

    async fn list<P: AsRef<Path> + Send + fmt::Debug>(
        &self,
        user: &FtpUser,
        path: P,
    ) -> unftp_core::storage::Result<Vec<Fileinfo<PathBuf, Meta>>> {
        let mut out = if parse_path(path.as_ref())? == Target::Root {
            user.account
                .sources()
                .into_iter()
                .map(|name| Fileinfo {
                    path: PathBuf::from(name),
                    metadata: Meta::dir(),
                })
                .collect::<Vec<_>>()
        } else {
            let (source, vp, storage) = resolve(user, path.as_ref(), Op::View).await?;
            storage
                .list(&vp)
                .await
                .map_err(storage_error)?
                .iter()
                .filter(|e| !user.account.hide(&source, &vp, &e.name))
                .map(|e| Fileinfo {
                    path: PathBuf::from(&e.name),
                    metadata: Meta::from(e),
                })
                .collect()
        };
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    async fn get<P: AsRef<Path> + Send + fmt::Debug>(
        &self,
        user: &FtpUser,
        path: P,
        start_pos: u64,
    ) -> unftp_core::storage::Result<Box<dyn AsyncRead + Send + Sync + Unpin>> {
        let (_, vp, storage) = resolve(user, path.as_ref(), Op::Download).await?;
        let range = if start_pos > 0 {
            let size = storage.stat(&vp).await.map_err(storage_error)?.size;
            if start_pos >= size {
                return Ok(Box::new(tokio::io::empty()));
            }
            Some(start_pos..size)
        } else {
            None
        };
        let stream = storage.read(&vp, range).await.map_err(storage_error)?;
        metrics::counter!("filebrary_ftp_transfers_total", "direction" => "download").increment(1);
        Ok(Box::new(reader(stream)))
    }

    async fn put<
        P: AsRef<Path> + Send + fmt::Debug,
        R: AsyncRead + Send + Sync + Unpin + 'static,
    >(
        &self,
        user: &FtpUser,
        input: R,
        path: P,
        start_pos: u64,
    ) -> unftp_core::storage::Result<u64> {
        // Check view access first so the existence probe below cannot reveal
        // anything the user may not see.
        let (source, vp, storage) = resolve_inner(user, path.as_ref(), Op::View).await?;
        let existing = stat_opt(storage.as_ref(), &vp).await?;
        if existing.as_ref().is_some_and(Entry::is_dir) {
            return Err(Error::from(ErrorKind::PermanentFileNotAvailable));
        }
        let op = if existing.is_some() || start_pos > 0 {
            Op::Edit
        } else {
            Op::Upload
        };
        user.account
            .resolve(&source, op, &vp)
            .await
            .map_err(denied)?;
        user.account
            .before_write(&source, op, &vp, None)
            .await
            .map_err(denied)?;

        let data: ByteReader = if start_pos > 0 {
            // Resumed upload (REST + STOR): keep the first `start_pos` bytes
            // of the existing file and append the new data after them.
            let size = existing.as_ref().map_or(0, |e| e.size);
            if start_pos > size {
                return Err(Error::from(ErrorKind::PermanentFileNotAvailable));
            }
            let head = storage
                .read(&vp, Some(0..start_pos))
                .await
                .map_err(storage_error)?;
            Box::pin(reader(head).chain(input))
        } else {
            Box::pin(input)
        };
        let written = storage
            .write(&vp, data, existing.is_some())
            .await
            .map_err(storage_error)?;
        metrics::counter!("filebrary_ftp_transfers_total", "direction" => "upload").increment(1);
        user.account
            .after_change(
                &source,
                Change::Upload {
                    path: &vp,
                    size: written,
                    replaced: existing.is_some(),
                },
            )
            .await;
        Ok(written.saturating_sub(start_pos))
    }

    async fn del<P: AsRef<Path> + Send + fmt::Debug>(
        &self,
        user: &FtpUser,
        path: P,
    ) -> unftp_core::storage::Result<()> {
        let (source, vp, storage) = resolve_inner(user, path.as_ref(), Op::Delete).await?;
        let e = storage.stat(&vp).await.map_err(storage_error)?;
        if e.is_dir() {
            return Err(Error::from(ErrorKind::PermanentFileNotAvailable));
        }
        user.account
            .remove(&source, &vp, &storage)
            .await
            .map_err(remove_error)
    }

    async fn mkd<P: AsRef<Path> + Send + fmt::Debug>(
        &self,
        user: &FtpUser,
        path: P,
    ) -> unftp_core::storage::Result<()> {
        let (source, vp, storage) = resolve_inner(user, path.as_ref(), Op::Upload).await?;
        if stat_opt(storage.as_ref(), &vp).await?.is_some() {
            return Err(Error::from(ErrorKind::PermanentFileNotAvailable));
        }
        storage.mkdir(&vp).await.map_err(storage_error)?;
        user.account
            .after_change(&source, Change::Mkdir { path: &vp })
            .await;
        Ok(())
    }

    async fn rename<P: AsRef<Path> + Send + fmt::Debug>(
        &self,
        user: &FtpUser,
        from: P,
        to: P,
    ) -> unftp_core::storage::Result<()> {
        let (from_source, from_vp, storage) = resolve_inner(user, from.as_ref(), Op::Move).await?;
        let (to_source, to_vp, _) = resolve_inner(user, to.as_ref(), Op::View).await?;
        if from_source != to_source {
            // Moving between sources would need a copy across backends.
            return Err(perm());
        }
        let exists = stat_opt(storage.as_ref(), &to_vp).await?.is_some();
        let op = if exists { Op::Edit } else { Op::Upload };
        user.account
            .resolve(&to_source, op, &to_vp)
            .await
            .map_err(denied)?;
        storage
            .rename(&from_vp, &to_vp, exists)
            .await
            .map_err(storage_error)?;
        user.account
            .after_change(
                &to_source,
                Change::Rename {
                    from: &from_vp,
                    to: &to_vp,
                    replaced: exists,
                },
            )
            .await;
        Ok(())
    }

    async fn rmd<P: AsRef<Path> + Send + fmt::Debug>(
        &self,
        user: &FtpUser,
        path: P,
    ) -> unftp_core::storage::Result<()> {
        let (source, vp, storage) = resolve_inner(user, path.as_ref(), Op::Delete).await?;
        let e = storage.stat(&vp).await.map_err(storage_error)?;
        if !e.is_dir() {
            return Err(Error::from(ErrorKind::PermanentDirectoryNotAvailable));
        }
        if !storage.list(&vp).await.map_err(storage_error)?.is_empty() {
            return Err(Error::from(ErrorKind::PermanentDirectoryNotEmpty));
        }
        user.account
            .remove(&source, &vp, &storage)
            .await
            .map_err(remove_error)
    }

    async fn cwd<P: AsRef<Path> + Send + fmt::Debug>(
        &self,
        user: &FtpUser,
        path: P,
    ) -> unftp_core::storage::Result<()> {
        let m = self.metadata(user, path).await?;
        if m.is_dir() {
            Ok(())
        } else {
            Err(Error::from(ErrorKind::PermanentDirectoryNotAvailable))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(s: &str, p: &str) -> Target {
        Target::Source(s.into(), VPath::parse(p).unwrap())
    }

    #[test]
    fn root_forms() {
        for p in ["", "/", ".", "/./", "//", "/..", "/../..", "a/.."] {
            assert_eq!(parse_path(Path::new(p)).unwrap(), Target::Root, "{p:?}");
        }
    }

    #[test]
    fn source_paths() {
        assert_eq!(parse_path(Path::new("/files")).unwrap(), src("files", ""));
        assert_eq!(
            parse_path(Path::new("/files/a/b.txt")).unwrap(),
            src("files", "a/b.txt")
        );
        assert_eq!(
            parse_path(Path::new("files\\a\\b.txt")).unwrap(),
            src("files", "a/b.txt")
        );
    }

    #[test]
    fn dot_dot_stays_inside_the_virtual_root() {
        // `..` is applied to the virtual path, so it can at most reach the
        // list of sources: never a parent of a source root or of a scope.
        assert_eq!(
            parse_path(Path::new("/files/a/../../media/x")).unwrap(),
            src("media", "x")
        );
        assert_eq!(
            parse_path(Path::new("/files/../../../../etc/passwd")).unwrap(),
            src("etc", "passwd")
        );
        assert_eq!(
            parse_path(Path::new("/files/a/b/../../../files/c")).unwrap(),
            src("files", "c")
        );
    }

    #[test]
    fn rejects_bad_components() {
        assert!(parse_path(Path::new("/files/a\0b")).is_err());
    }
}
