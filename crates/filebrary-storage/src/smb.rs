//! The SMB backend: a folder on a Windows, Samba or NAS share (SMB 2 and 3).
//!
//! The client is pure Rust (`smb2`); there is no `libsmbclient` and no mount.
//! One authenticated session per source is opened on first use and shared;
//! it reconnects by itself when the server goes away and comes back.
//! Requests on one source run one at a time over that session, except that
//! file content streams in the background once a transfer has started.
//!
//! Signing and encryption follow what the server negotiates (SMB 3 encrypts
//! when the share asks for it). Authentication is NTLMv2 with a user name,
//! password and optional domain.
//!
//! # Path safety
//!
//! Every [`VPath`] has already been validated. SMB servers follow junctions
//! and symbolic links themselves, which could lead outside the configured
//! folder or even the share, so this backend:
//!
//! * hides links (junctions and symlinks; any reparse point that names
//!   another place) from listings;
//! * refuses any path that goes *through* a link, checking every folder on
//!   the way (one round trip per level);
//! * refuses to read, list, write or copy a link itself;
//! * deletes and renames a link as the link, never its target;
//! * never follows links while copying or deleting trees.
//!
//! Cloud and deduplication placeholders are ordinary files and are served.
//!
//! # Atomicity
//!
//! Writes go to a hidden temporary file that is renamed over the target in
//! one step (SMB can replace on rename), so readers never see a partial file.

use std::{io, ops::Range, path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use smb2::{
    ClientConfig, ErrorKind, FileInfo, RenameOptions, SmbClient, Tree, client::tree::DirectoryEntry,
};
use time::OffsetDateTime;
use tokio::sync::Mutex;

use crate::{
    ByteReader, ByteStream, Entry, EntryKind, Storage, StorageError, StorageResult, VPath,
    guess_mime,
    util::{
        check_range, io_other, is_temp_name, read_chunk, refuse_into_self, refuse_root, temp_name,
    },
};

/// Bytes requested per read when streaming a file.
const READ_CHUNK: u64 = 1024 * 1024;

/// Bytes handed to the writer at a time.
const WRITE_CHUNK: usize = 1024 * 1024;

/// Settings for an SMB source.
///
/// ```toml
/// [[sources]]
/// name = "office"
/// kind = "smb"
///
/// [sources.smb]
/// host = "fileserver.lan"
/// share = "Projects"
/// root = "Shared/2024"        # optional folder inside the share
/// username = "filebrary"
/// password_file = "/run/secrets/smb"
/// domain = "CORP"             # optional
/// ```
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SmbConfig {
    /// Server host name or address.
    pub host: String,
    /// TCP port (default 445).
    pub port: Option<u16>,
    /// Share name.
    pub share: String,
    /// Folder inside the share that acts as the source root (optional).
    #[serde(default)]
    pub root: String,
    /// Account name; empty for guest access.
    #[serde(default)]
    pub username: String,
    /// Password.
    pub password: Option<String>,
    /// File holding the password (wins over `password`).
    pub password_file: Option<PathBuf>,
    /// Windows domain or workgroup (optional).
    #[serde(default)]
    pub domain: String,
}

impl SmbConfig {
    fn addr(&self) -> String {
        let port = self.port.unwrap_or(445);
        if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]:{port}", self.host) // bare IPv6 address
        } else {
            format!("{}:{port}", self.host)
        }
    }

    fn password(&self) -> io::Result<String> {
        if let Some(f) = &self.password_file {
            return Ok(std::fs::read_to_string(f)?
                .trim_end_matches(['\r', '\n'])
                .to_owned());
        }
        Ok(self.password.clone().unwrap_or_default())
    }
}

/// A connected client and the share it is attached to.
struct Session {
    client: SmbClient,
    tree: Tree,
}

/// A source backed by a folder on an SMB share.
pub struct SmbStorage {
    cfg: Arc<SmbConfig>,
    /// The root folder inside the share.
    root: VPath,
    /// Shared with the scoped storages made by [`Storage::sub`].
    session: Arc<Mutex<Option<Session>>>,
}

impl std::fmt::Debug for SmbStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmbStorage")
            .field("host", &self.cfg.host)
            .field("share", &self.cfg.share)
            .field("root", &self.root)
            .field("username", &self.cfg.username)
            .finish_non_exhaustive()
    }
}

impl SmbStorage {
    /// Checks the settings. The connection is made on first use.
    ///
    /// # Errors
    ///
    /// Fails when the host or share is missing or the root is not a valid path.
    pub fn new(cfg: &SmbConfig) -> StorageResult<Self> {
        let bad = |m: &str| Err(StorageError::InvalidPath(format!("smb: {m}")));
        if cfg.host.is_empty() {
            return bad("host is required");
        }
        if cfg.share.is_empty() || cfg.share.contains(['/', '\\']) {
            return bad("share must be a single share name");
        }
        let root = VPath::parse(&cfg.root.replace('\\', "/"))?;
        Ok(Self {
            cfg: Arc::new(cfg.clone()),
            root,
            session: Arc::new(Mutex::new(None)),
        })
    }

    /// Locks the session, connecting if needed.
    async fn session(&self) -> StorageResult<tokio::sync::MutexGuard<'_, Option<Session>>> {
        let mut guard = self.session.lock().await;
        if guard.as_ref().is_some_and(|s| s.client.is_disconnected()) {
            *guard = None;
        }
        if guard.is_none() {
            *guard = Some(self.connect().await?);
        }
        Ok(guard)
    }

    async fn connect(&self) -> StorageResult<Session> {
        let cfg = &self.cfg;
        let config = ClientConfig {
            addr: cfg.addr(),
            timeout: Duration::from_secs(20),
            username: cfg.username.clone(),
            password: cfg.password()?,
            domain: cfg.domain.clone(),
            auto_reconnect: true,
            ..Default::default()
        };
        let mut client = SmbClient::connect(config)
            .await
            .map_err(|e| io_other(format!("smb: connecting to {}: {e}", cfg.host)))?;
        let mut tree = client
            .connect_share(&cfg.share)
            .await
            .map_err(|e| io_other(format!("smb: opening share {}: {e}", cfg.share)))?;
        if !self.root.is_root() {
            let root = path_str(&self.root);
            let info = client.stat(&mut tree, &root).await.map_err(map_err)?;
            if !info.is_directory {
                return Err(io_other(format!("smb: root {root} is not a folder")));
            }
        }
        Ok(Session { client, tree })
    }

    /// The share-relative path of `vp`.
    fn full(&self, vp: &VPath) -> String {
        path_str(&self.root.join_path(vp))
    }

    /// Refuses `vp` if any folder on the way to it (below the root) is a link.
    async fn check_ancestors(&self, s: &mut Session, vp: &VPath) -> StorageResult<()> {
        let n = vp.components().len();
        if n < 2 {
            return Ok(());
        }
        let mut cur = VPath::root();
        let mut paths = Vec::with_capacity(n - 1);
        for part in &vp.components()[..n - 1] {
            cur = cur.join(part)?;
            paths.push(self.full(&cur));
        }
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        for r in s.client.stat_files(&s.tree, &refs).await {
            let info = r.map_err(map_err)?;
            if info.is_link() {
                return Err(StorageError::OutsideRoot);
            }
            if !info.is_directory {
                return Err(StorageError::NotADirectory);
            }
        }
        Ok(())
    }

    /// Stats `vp` itself (a link is described, not followed); `None` if missing.
    async fn lstat(&self, s: &mut Session, vp: &VPath) -> StorageResult<Option<Info>> {
        if vp.is_root() && self.root.is_root() {
            return Ok(Some(root_info()));
        }
        match s.client.stat(&mut s.tree, &self.full(vp)).await {
            Ok(i) => Ok(Some(i.into())),
            Err(e) => match map_err(e) {
                StorageError::NotFound => Ok(None),
                other => Err(other),
            },
        }
    }

    /// Checks the path and returns what is at `vp`, refusing links.
    async fn stat_checked(&self, s: &mut Session, vp: &VPath) -> StorageResult<Info> {
        self.check_ancestors(s, vp).await?;
        let info = self.lstat(s, vp).await?.ok_or(StorageError::NotFound)?;
        if info.is_link() && !vp.is_root() {
            return Err(StorageError::OutsideRoot);
        }
        Ok(info)
    }

    /// Refuses an existing target unless `overwrite` and it is not a folder.
    /// Returns whether something is in the way.
    async fn check_target(
        &self,
        s: &mut Session,
        to: &VPath,
        overwrite: bool,
    ) -> StorageResult<bool> {
        match self.lstat(s, to).await? {
            Some(i) if i.is_directory || !overwrite => Err(StorageError::AlreadyExists),
            Some(_) => Ok(true),
            None => Ok(false),
        }
    }

    /// Lists a folder, without `.`/`..` and temporary files.
    async fn read_dir(s: &mut Session, path: &str) -> StorageResult<Vec<DirectoryEntry>> {
        let entries = s
            .client
            .list_directory(&mut s.tree, path)
            .await
            .map_err(map_err)?;
        Ok(entries
            .into_iter()
            .filter(|e| e.name != "." && e.name != ".." && !is_temp_name(&e.name))
            .collect())
    }

    /// Streams `data` into a new temporary file next to `dst`, then renames it
    /// over `dst`. The session lock is released while the bytes flow.
    async fn write_via_temp(
        &self,
        dst: String,
        data: &mut ByteReader,
        replace: bool,
    ) -> StorageResult<u64> {
        let (dir, name) = dst
            .rsplit_once('/')
            .map_or(("", dst.as_str()), |(d, n)| (d, n));
        let tmp = if dir.is_empty() {
            temp_name(name)
        } else {
            format!("{dir}/{}", temp_name(name))
        };
        let mut writer = {
            let mut guard = self.session().await?;
            let s = guard.as_mut().ok_or_else(|| io_other("smb: no session"))?;
            s.client
                .create_file_writer_exclusive(&s.tree, &tmp)
                .await
                .map_err(map_err)?
        };
        let streamed: StorageResult<()> = async {
            loop {
                let chunk = read_chunk(data, WRITE_CHUNK).await?;
                if chunk.is_empty() {
                    return Ok(());
                }
                writer.write_chunk(&chunk).await.map_err(map_err)?;
            }
        }
        .await;
        let result = match streamed {
            Ok(()) => writer.finish().await.map_err(map_err),
            Err(e) => {
                let _ = writer.abort().await;
                Err(e)
            }
        };
        let mut guard = self.session().await?;
        let s = guard.as_mut().ok_or_else(|| io_other("smb: no session"))?;
        let committed = match result {
            Ok(n) => s
                .client
                .rename_with(
                    &s.tree,
                    &tmp,
                    &dst,
                    RenameOptions::new().replace_if_exists(replace),
                )
                .await
                .map_err(map_err)
                .map(|()| n),
            Err(e) => Err(e),
        };
        if committed.is_err() {
            let _ = s.client.delete_file(&s.tree, &tmp).await;
        }
        committed
    }

    /// Copies one file: server side when the server can, else through here.
    async fn copy_file(&self, src: String, dst: String, replace: bool) -> StorageResult<()> {
        let reader = {
            let mut guard = self.session().await?;
            let s = guard.as_mut().ok_or_else(|| io_other("smb: no session"))?;
            if !replace {
                let mut conn = s.client.connection().clone();
                match s.tree.server_side_copy_file(&mut conn, &src, &dst).await {
                    Ok(_) => return Ok(()),
                    Err(e) if e.kind() == ErrorKind::Unsupported => {}
                    Err(e) => return Err(map_err(e)),
                }
            }
            s.client
                .open_file_reader(&s.tree, &src)
                .await
                .map_err(map_err)?
        };
        let size = reader.size();
        let mut data: ByteReader = Box::pin(tokio_util::io::StreamReader::new(reader_stream(
            reader,
            0..size,
        )));
        self.write_via_temp(dst, &mut data, replace)
            .await
            .map(|_| ())
    }

    /// Copies a folder tree, skipping links.
    async fn copy_tree(&self, src: String, dst: String) -> StorageResult<()> {
        let mut stack = vec![(src, dst)];
        while let Some((from, to)) = stack.pop() {
            let entries = {
                let mut guard = self.session().await?;
                let s = guard.as_mut().ok_or_else(|| io_other("smb: no session"))?;
                s.client
                    .create_directory(&s.tree, &to)
                    .await
                    .map_err(map_err)?;
                Self::read_dir(s, &from).await?
            };
            for e in entries {
                let (f, t) = (join(&from, &e.name), join(&to, &e.name));
                if e.is_link() {
                    tracing::debug!(path = %f, "skipping link during copy");
                } else if e.is_directory {
                    stack.push((f, t));
                } else {
                    self.copy_file(f, t, false).await?;
                }
            }
        }
        Ok(())
    }

    /// Deletes a tree without following links.
    async fn remove_tree(s: &mut Session, root: String) -> StorageResult<()> {
        let mut dirs = vec![root.clone()];
        let mut stack = vec![root];
        while let Some(d) = stack.pop() {
            for e in Self::read_dir(s, &d).await? {
                let p = join(&d, &e.name);
                if e.is_link() {
                    s.client.delete_link(&s.tree, &p).await.map_err(map_err)?;
                } else if e.is_directory {
                    dirs.push(p.clone());
                    stack.push(p);
                } else {
                    s.client.delete_file(&s.tree, &p).await.map_err(map_err)?;
                }
            }
        }
        for d in dirs.into_iter().rev() {
            s.client
                .delete_directory(&s.tree, &d)
                .await
                .map_err(map_err)?;
        }
        Ok(())
    }
}

#[async_trait]
impl Storage for SmbStorage {
    async fn sub(&self, scope: &VPath) -> StorageResult<Arc<dyn Storage>> {
        {
            let mut guard = self.session().await?;
            let s = guard.as_mut().ok_or_else(|| io_other("smb: no session"))?;
            if !self.stat_checked(s, scope).await?.is_directory {
                return Err(StorageError::NotADirectory);
            }
        }
        Ok(Arc::new(Self {
            cfg: Arc::clone(&self.cfg),
            root: self.root.join_path(scope),
            session: Arc::clone(&self.session),
        }))
    }

    async fn list(&self, vp: &VPath) -> StorageResult<Vec<Entry>> {
        let mut guard = self.session().await?;
        let s = guard.as_mut().ok_or_else(|| io_other("smb: no session"))?;
        let info = self.stat_checked(s, vp).await?;
        if !info.is_directory {
            return Err(StorageError::NotADirectory);
        }
        let entries = Self::read_dir(s, &self.full(vp)).await?;
        Ok(entries
            .into_iter()
            .filter(|e| !e.is_link())
            .filter_map(|e| {
                let child = vp.join(&e.name).ok()?;
                Some(entry(
                    child,
                    e.is_directory,
                    e.size,
                    e.modified.to_system_time(),
                ))
            })
            .collect())
    }

    async fn stat(&self, vp: &VPath) -> StorageResult<Entry> {
        let mut guard = self.session().await?;
        let s = guard.as_mut().ok_or_else(|| io_other("smb: no session"))?;
        let info = self.stat_checked(s, vp).await?;
        Ok(entry(
            vp.clone(),
            info.is_directory,
            info.size,
            info.modified,
        ))
    }

    async fn read(&self, vp: &VPath, range: Option<Range<u64>>) -> StorageResult<ByteStream> {
        let reader = {
            let mut guard = self.session().await?;
            let s = guard.as_mut().ok_or_else(|| io_other("smb: no session"))?;
            let info = self.stat_checked(s, vp).await?;
            if info.is_directory {
                return Err(StorageError::IsADirectory);
            }
            if let Some(r) = &range {
                check_range(r, info.size)?;
            }
            s.client
                .open_file_reader(&s.tree, &self.full(vp))
                .await
                .map_err(map_err)?
        };
        let size = reader.size();
        let r = range.unwrap_or(0..size);
        check_range(&r, size)?;
        Ok(reader_stream(reader, r))
    }

    async fn write(&self, vp: &VPath, mut data: ByteReader, overwrite: bool) -> StorageResult<u64> {
        if vp.is_root() {
            return Err(StorageError::IsADirectory);
        }
        let replace = {
            let mut guard = self.session().await?;
            let s = guard.as_mut().ok_or_else(|| io_other("smb: no session"))?;
            self.check_ancestors(s, vp).await?;
            if let Some(parent) = vp.parent().filter(|p| !p.is_root()) {
                match self.lstat(s, &parent).await? {
                    Some(i) if i.is_directory => {}
                    Some(_) => return Err(StorageError::NotADirectory),
                    None => return Err(StorageError::NotFound),
                }
            }
            match self.lstat(s, vp).await? {
                Some(i) if i.is_directory => return Err(StorageError::IsADirectory),
                Some(_) if !overwrite => return Err(StorageError::AlreadyExists),
                Some(_) => true,
                None => false,
            }
        };
        self.write_via_temp(self.full(vp), &mut data, replace).await
    }

    async fn mkdir(&self, vp: &VPath) -> StorageResult<()> {
        let mut guard = self.session().await?;
        let s = guard.as_mut().ok_or_else(|| io_other("smb: no session"))?;
        let mut cur = VPath::root();
        for part in vp.components() {
            cur = cur.join(part)?;
            match self.lstat(s, &cur).await? {
                Some(i) if i.is_link() => return Err(StorageError::OutsideRoot),
                Some(i) if i.is_directory => {}
                Some(_) => return Err(StorageError::NotADirectory),
                None => {
                    if let Err(e) = s.client.create_directory(&s.tree, &self.full(&cur)).await {
                        let err = map_err(e);
                        // Lost a race with another creator: fine if it is a folder now.
                        match self.lstat(s, &cur).await? {
                            Some(i) if i.is_directory && !i.is_link() => {}
                            _ => return Err(err),
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn rename(&self, from: &VPath, to: &VPath, overwrite: bool) -> StorageResult<()> {
        refuse_root(from, "move")?;
        refuse_root(to, "move onto")?;
        if from == to {
            return Ok(());
        }
        refuse_into_self(from, to)?;
        let mut guard = self.session().await?;
        let s = guard.as_mut().ok_or_else(|| io_other("smb: no session"))?;
        self.check_ancestors(s, from).await?;
        self.check_ancestors(s, to).await?;
        if self.lstat(s, from).await?.is_none() {
            return Err(StorageError::NotFound);
        }
        let replace = self.check_target(s, to, overwrite).await?;
        s.client
            .rename_with(
                &s.tree,
                &self.full(from),
                &self.full(to),
                RenameOptions::new().replace_if_exists(replace),
            )
            .await
            .map_err(map_err)
    }

    async fn copy(&self, from: &VPath, to: &VPath, overwrite: bool) -> StorageResult<()> {
        if to.is_root() || from == to {
            return Err(StorageError::AlreadyExists);
        }
        refuse_into_self(from, to)?;
        let (is_dir, replace) = {
            let mut guard = self.session().await?;
            let s = guard.as_mut().ok_or_else(|| io_other("smb: no session"))?;
            let info = self.stat_checked(s, from).await?;
            self.check_ancestors(s, to).await?;
            let replace = self.check_target(s, to, overwrite).await?;
            (info.is_directory, replace)
        };
        if is_dir {
            if replace {
                return Err(StorageError::AlreadyExists);
            }
            self.copy_tree(self.full(from), self.full(to)).await
        } else {
            self.copy_file(self.full(from), self.full(to), replace)
                .await
        }
    }

    async fn delete(&self, vp: &VPath) -> StorageResult<()> {
        refuse_root(vp, "delete")?;
        let mut guard = self.session().await?;
        let s = guard.as_mut().ok_or_else(|| io_other("smb: no session"))?;
        self.check_ancestors(s, vp).await?;
        let info = self.lstat(s, vp).await?.ok_or(StorageError::NotFound)?;
        let p = self.full(vp);
        if info.is_link() {
            s.client.delete_link(&s.tree, &p).await.map_err(map_err)
        } else if info.is_directory {
            Self::remove_tree(s, p).await
        } else {
            s.client.delete_file(&s.tree, &p).await.map_err(map_err)
        }
    }
}

/// Streams `range` of an open file in [`READ_CHUNK`] pieces, closing it at the end.
fn reader_stream(reader: smb2::FileReader, range: Range<u64>) -> ByteStream {
    futures::stream::unfold(Some((reader, range.start)), move |state| {
        let end = range.end;
        async move {
            let (reader, pos) = state?;
            if pos >= end {
                if let Err(e) = reader.close().await {
                    tracing::debug!(error = %e, "closing SMB file failed");
                }
                return None;
            }
            let len = (end - pos).min(READ_CHUNK);
            match reader.read_at(pos, len).await {
                Ok(buf) if buf.is_empty() => Some((
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "smb: file shrank",
                    )),
                    None,
                )),
                Ok(buf) => {
                    let next = pos + buf.len() as u64;
                    Some((Ok(bytes::Bytes::from(buf)), Some((reader, next))))
                }
                Err(e) => Some((Err(io::Error::other(format!("smb: {e}"))), None)),
            }
        }
    })
    .boxed()
}

fn path_str(vp: &VPath) -> String {
    vp.components().join("/")
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// What a stat says about one path.
#[derive(Clone, Copy, Debug, Default)]
struct Info {
    is_directory: bool,
    link: bool,
    size: u64,
    modified: Option<std::time::SystemTime>,
}

impl Info {
    fn is_link(&self) -> bool {
        self.link
    }
}

impl From<FileInfo> for Info {
    fn from(i: FileInfo) -> Self {
        Self {
            is_directory: i.is_directory,
            link: i.is_link(),
            size: i.size,
            modified: i.modified.to_system_time(),
        }
    }
}

fn root_info() -> Info {
    Info {
        is_directory: true,
        ..Default::default()
    }
}

fn entry(vp: VPath, is_dir: bool, size: u64, modified: Option<std::time::SystemTime>) -> Entry {
    let name = vp.name().unwrap_or_default().to_owned();
    Entry {
        mime: (!is_dir).then(|| guess_mime(&name)),
        name,
        path: vp,
        kind: if is_dir {
            EntryKind::Dir
        } else {
            EntryKind::File
        },
        size: if is_dir { 0 } else { size },
        modified: modified.map(OffsetDateTime::from),
    }
}

fn map_err(e: smb2::Error) -> StorageError {
    let msg = format!("smb: {e}");
    match e.kind() {
        ErrorKind::NotFound => StorageError::NotFound,
        ErrorKind::AlreadyExists => StorageError::AlreadyExists,
        ErrorKind::IsADirectory => StorageError::IsADirectory,
        ErrorKind::NotADirectory => StorageError::NotADirectory,
        ErrorKind::AccessDenied => {
            StorageError::Io(io::Error::new(io::ErrorKind::PermissionDenied, msg))
        }
        ErrorKind::Unsupported => StorageError::Unsupported(msg),
        _ => io_other(msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SmbConfig {
        SmbConfig {
            host: "nas".into(),
            share: "data".into(),
            root: "a\\b".into(),
            ..Default::default()
        }
    }

    #[test]
    fn validates_settings() {
        let s = SmbStorage::new(&cfg()).unwrap();
        assert_eq!(s.full(&VPath::parse("x/y").unwrap()), "a/b/x/y");
        assert_eq!(s.full(&VPath::root()), "a/b");
        assert!(
            SmbStorage::new(&SmbConfig {
                share: "a/b".into(),
                ..cfg()
            })
            .is_err()
        );
        assert!(
            SmbStorage::new(&SmbConfig {
                host: String::new(),
                ..cfg()
            })
            .is_err()
        );
        assert!(
            SmbStorage::new(&SmbConfig {
                root: "../up".into(),
                ..cfg()
            })
            .is_err()
        );
    }

    #[test]
    fn addresses() {
        assert_eq!(cfg().addr(), "nas:445");
        let v6 = SmbConfig {
            host: "fe80::1".into(),
            port: Some(1445),
            ..cfg()
        };
        assert_eq!(v6.addr(), "[fe80::1]:1445");
    }

    #[test]
    fn joins() {
        assert_eq!(join("", "a"), "a");
        assert_eq!(join("x", "a"), "x/a");
    }
}
