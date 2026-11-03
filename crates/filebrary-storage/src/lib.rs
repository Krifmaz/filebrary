//! The storage abstraction every Filebrary backend implements.
//!
//! A *source* is a named storage location (a local folder today; S3, SFTP and
//! SMB later) exposed to users. The rest of the server only talks to sources
//! through the [`Storage`] trait, using validated [`VPath`]s, so that path
//! safety is enforced in one place per backend.

#![warn(missing_docs)]

pub mod local;
pub mod path;

use std::{io, ops::Range, pin::Pin, sync::Arc};

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use serde::Serialize;
use time::OffsetDateTime;
use tokio::io::AsyncRead;

pub use path::VPath;

/// A stream of file content chunks.
pub type ByteStream = Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send>>;

/// A reader for content being written to a backend.
pub type ByteReader = Pin<Box<dyn AsyncRead + Send>>;

/// Errors a backend can report.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// Nothing exists at the path.
    #[error("not found")]
    NotFound,
    /// The target already exists and overwriting was not requested.
    #[error("already exists")]
    AlreadyExists,
    /// The path failed validation.
    #[error("invalid path: {0}")]
    InvalidPath(String),
    /// The path (or a symlink on it) resolves outside the source root.
    #[error("path escapes the source root")]
    OutsideRoot,
    /// A directory was required.
    #[error("not a directory")]
    NotADirectory,
    /// A file was required.
    #[error("is a directory")]
    IsADirectory,
    /// The requested byte range does not fit the file.
    #[error("invalid range")]
    InvalidRange {
        /// The file size.
        size: u64,
    },
    /// The backend cannot perform this operation.
    #[error("{0}")]
    Unsupported(String),
    /// Any other I/O failure.
    #[error(transparent)]
    Io(io::Error),
}

impl From<io::Error> for StorageError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::AlreadyExists => Self::AlreadyExists,
            io::ErrorKind::NotADirectory => Self::NotADirectory,
            io::ErrorKind::IsADirectory => Self::IsADirectory,
            _ => Self::Io(e),
        }
    }
}

/// Result alias for storage operations.
pub type StorageResult<T> = Result<T, StorageError>;

/// What kind of object an [`Entry`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A directory.
    Dir,
}

/// Metadata about one file or directory.
#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    /// The final path component.
    pub name: String,
    /// Full path inside the source.
    pub path: VPath,
    /// File or directory.
    pub kind: EntryKind,
    /// Size in bytes (0 for directories).
    pub size: u64,
    /// Last modification time, if the backend knows it.
    #[serde(with = "time::serde::rfc3339::option")]
    pub modified: Option<OffsetDateTime>,
    /// Best-guess MIME type for files, from the extension.
    pub mime: Option<String>,
}

impl Entry {
    /// Returns `true` for directories.
    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Dir
    }
}

/// A storage backend. All paths are relative to the backend's root and have
/// already been validated by [`VPath::parse`]; implementations must still
/// make sure nothing (symlinks, junctions, provider quirks) leads outside.
#[async_trait]
pub trait Storage: Send + Sync + 'static {
    /// Lists the direct children of a directory, unsorted.
    async fn list(&self, path: &VPath) -> StorageResult<Vec<Entry>>;

    /// Returns metadata for one path.
    async fn stat(&self, path: &VPath) -> StorageResult<Entry>;

    /// Reads a file, optionally only `range` (half-open byte range).
    ///
    /// Fails with [`StorageError::InvalidRange`] if the range does not fit.
    async fn read(&self, path: &VPath, range: Option<Range<u64>>) -> StorageResult<ByteStream>;

    /// Writes a file from `data`, returning the number of bytes written.
    ///
    /// The write is atomic where the backend allows it: readers never see a
    /// half-written file. Fails with [`StorageError::AlreadyExists`] if the
    /// target exists and `overwrite` is false. The parent must exist.
    async fn write(&self, path: &VPath, data: ByteReader, overwrite: bool) -> StorageResult<u64>;

    /// Creates a directory (and any missing parents).
    async fn mkdir(&self, path: &VPath) -> StorageResult<()>;

    /// Renames or moves `from` to `to` within the same source.
    async fn rename(&self, from: &VPath, to: &VPath, overwrite: bool) -> StorageResult<()>;

    /// Copies a file or (recursively) a directory within the same source.
    async fn copy(&self, from: &VPath, to: &VPath, overwrite: bool) -> StorageResult<()>;

    /// Deletes a file or (recursively) a directory.
    async fn delete(&self, path: &VPath) -> StorageResult<()>;

    /// Returns a backend rooted at the existing folder `scope`.
    ///
    /// Every safety rule that applies to the source root applies to the
    /// scope of the returned backend: in particular a symlink inside the
    /// scope that points elsewhere in the source (outside the scope) is
    /// refused. Use it to confine a user to a folder.
    async fn sub(&self, scope: &VPath) -> StorageResult<Arc<dyn Storage>>;

    /// Returns the symlink-free form of `path` inside the root, so permission
    /// rules can be checked against where a request really lands.
    ///
    /// For an existing path this is its canonical form; for a missing one, the
    /// canonical nearest existing ancestor plus the remaining components.
    /// Fails with [`StorageError::OutsideRoot`] if it resolves outside.
    /// Backends without symlinks (object stores) keep the default, which
    /// returns the path unchanged.
    async fn resolve(&self, path: &VPath) -> StorageResult<VPath> {
        Ok(path.clone())
    }
}

/// A configured, named source with its backend.
#[derive(Clone)]
pub struct Source {
    /// Short identifier used in URLs (`[a-z0-9_-]`).
    pub name: String,
    /// The backend serving this source.
    pub storage: Arc<dyn Storage>,
}

/// Guesses a MIME type from a file name.
pub fn guess_mime(name: &str) -> String {
    mime_guess::from_path(name)
        .first_or_octet_stream()
        .essence_str()
        .to_owned()
}
