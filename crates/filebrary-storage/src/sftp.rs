//! The SFTP backend: a directory on any SSH server.
//!
//! The client is pure Rust (`russh` and `russh-sftp`); no `ssh` binary or
//! shell is involved. One SSH connection per source is opened on first use and
//! shared by all requests (SFTP pipelines them). If it drops, the next request
//! reconnects.
//!
//! # Server authentication
//!
//! The server's host key must be pinned: by its SHA-256 fingerprint
//! (`host_key_fingerprint`, as printed by `ssh-keygen -lf`), by a
//! `known_hosts` file, or, for tests only, by turning the check off. When the
//! check fails, the error names the fingerprint the server presented so the
//! admin can verify and pin it.
//!
//! # Path safety
//!
//! Every [`VPath`] has already been validated. On top of that this backend
//! mirrors the local backend's symlink rules, using the server's `realpath`:
//!
//! * the root is canonicalised once per connection;
//! * the parent of every path it touches is canonicalised and must lie inside
//!   the root, so a symlinked folder that points outside is refused;
//! * a final component that is a symlink must resolve inside the root;
//! * such symlinks are hidden from listings;
//! * copies and deletes never follow symlinks inside a tree.
//!
//! The server enforces its own permissions on top; give the SSH account access
//! to the shared directory only.
//!
//! # Atomicity
//!
//! Writes go to a hidden temporary file that is renamed into place, so readers
//! never see a half-written file. SFTP version 3 cannot rename over an existing
//! file, so an overwrite removes the old file just before the rename.

use std::{
    io,
    ops::Range,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use futures::StreamExt;
use russh::{
    client,
    keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate},
};
use russh_sftp::{
    client::{SftpSession, error::Error as SftpError, fs::Metadata},
    protocol::{FileType, OpenFlags, StatusCode},
};
use serde::Deserialize;
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;

use crate::{
    ByteReader, ByteStream, Entry, EntryKind, Storage, StorageError, StorageResult, VPath,
    guess_mime,
    util::{check_range, io_other, is_temp_name, refuse_into_self, refuse_root, temp_name},
};

/// Size of the chunks produced by [`SftpStorage::read`].
const READ_CHUNK: usize = 64 * 1024;

/// How long to wait for the TCP connection and SSH handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Seconds to wait for any single SFTP reply.
const REQUEST_TIMEOUT_SECS: u64 = 60;

/// Settings for an SFTP source.
///
/// ```toml
/// [[sources]]
/// name = "nas"
/// kind = "sftp"
///
/// [sources.sftp]
/// host = "nas.lan"
/// username = "filebrary"
/// private_key = "/run/secrets/nas_ed25519"
/// root = "/volume1/shared"
/// host_key_fingerprint = "SHA256:7cLm..."
/// ```
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SftpConfig {
    /// Server host name or address.
    pub host: String,
    /// SSH port (default 22).
    pub port: Option<u16>,
    /// Account name.
    pub username: String,
    /// Password (prefer `password_file` or a key).
    pub password: Option<String>,
    /// File holding the password (wins over `password`).
    pub password_file: Option<PathBuf>,
    /// Private key file in OpenSSH or PKCS#8 format.
    pub private_key: Option<PathBuf>,
    /// Passphrase for an encrypted private key.
    pub private_key_passphrase: Option<String>,
    /// Directory on the server that acts as the source root. Relative paths
    /// start at the account's home directory; empty means the home directory.
    #[serde(default)]
    pub root: String,
    /// Expected host key fingerprint, e.g. `SHA256:7cLm...`.
    pub host_key_fingerprint: Option<String>,
    /// A `known_hosts` file to check the host key against.
    pub known_hosts: Option<PathBuf>,
    /// Accept any host key. Only for tests: it allows man-in-the-middle attacks.
    #[serde(default)]
    pub insecure_accept_any_host_key: bool,
}

impl SftpConfig {
    fn port(&self) -> u16 {
        self.port.unwrap_or(22)
    }

    fn password(&self) -> io::Result<Option<String>> {
        if let Some(f) = &self.password_file {
            return Ok(Some(
                std::fs::read_to_string(f)?
                    .trim_end_matches(['\r', '\n'])
                    .to_owned(),
            ));
        }
        Ok(self.password.clone())
    }
}

/// How the server's host key is checked.
#[derive(Clone, Debug)]
enum HostKeyCheck {
    Fingerprint(String),
    KnownHosts(PathBuf),
    Insecure,
}

/// The russh client handler: checks the host key.
struct Client {
    host: String,
    port: u16,
    check: HostKeyCheck,
    /// The fingerprint the server presented, for error messages.
    seen: Arc<std::sync::Mutex<Option<String>>>,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let (data, plain) = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => (key.key_data().clone(), Some(key)),
            PublicKeyOrCertificate::Certificate(c) => (c.public_key().clone(), None),
        };
        let fp = data.fingerprint(HashAlg::Sha256).to_string();
        if let Ok(mut seen) = self.seen.lock() {
            *seen = Some(fp.clone());
        }
        Ok(match &self.check {
            HostKeyCheck::Insecure => true,
            HostKeyCheck::Fingerprint(want) => want.trim() == fp,
            HostKeyCheck::KnownHosts(path) => match plain {
                Some(k) => russh::keys::check_known_hosts_path(&self.host, self.port, k, path)
                    .unwrap_or(false),
                None => false,
            },
        })
    }
}

/// One live SSH connection with its SFTP session.
struct Conn {
    handle: client::Handle<Client>,
    sftp: SftpSession,
    /// The canonical root directory on the server.
    root: String,
    /// Set when a request failed in a way that suggests the link is gone.
    dead: AtomicBool,
}

impl Conn {
    fn usable(&self) -> bool {
        !self.dead.load(Ordering::Relaxed) && !self.handle.is_closed()
    }

    fn join(base: &str, name: &str) -> String {
        if base.ends_with('/') {
            format!("{base}{name}")
        } else {
            format!("{base}/{name}")
        }
    }
}

/// A connection together with the root directory one storage works in.
struct Ctx {
    conn: Arc<Conn>,
    /// The canonical root of this storage (the source root or a sub-scope).
    root: String,
}

impl std::ops::Deref for Ctx {
    type Target = Conn;
    fn deref(&self) -> &Conn {
        &self.conn
    }
}

impl Ctx {
    /// Returns `true` if the canonical path `p` is the root or lies inside it.
    fn inside(&self, p: &str) -> bool {
        self.root == "/" || p == self.root || p.starts_with(&format!("{}/", self.root))
    }
}

/// A source backed by a directory on an SFTP server.
pub struct SftpStorage {
    cfg: Arc<SftpConfig>,
    check: HostKeyCheck,
    /// Shared with the scoped storages made by [`Storage::sub`].
    conn: Arc<tokio::sync::Mutex<Option<Arc<Conn>>>>,
    /// Canonical root of a scoped storage; `None` for the source root.
    scope: Option<String>,
}

impl std::fmt::Debug for SftpStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SftpStorage")
            .field("host", &self.cfg.host)
            .field("port", &self.cfg.port())
            .field("username", &self.cfg.username)
            .field("root", &self.cfg.root)
            .finish_non_exhaustive()
    }
}

impl SftpStorage {
    /// Checks the settings. The connection is made on first use.
    ///
    /// # Errors
    ///
    /// Fails when the host, user, credentials or host-key check are missing.
    pub fn new(cfg: &SftpConfig) -> StorageResult<Self> {
        let bad = |m: &str| Err(StorageError::InvalidPath(format!("sftp: {m}")));
        if cfg.host.is_empty() {
            return bad("host is required");
        }
        if cfg.username.is_empty() {
            return bad("username is required");
        }
        if cfg.password.is_none() && cfg.password_file.is_none() && cfg.private_key.is_none() {
            return bad("set password, password_file or private_key");
        }
        if cfg.root.contains('\0') {
            return bad("invalid root");
        }
        let check = match (
            &cfg.host_key_fingerprint,
            &cfg.known_hosts,
            cfg.insecure_accept_any_host_key,
        ) {
            (Some(fp), _, _) => HostKeyCheck::Fingerprint(fp.clone()),
            (None, Some(kh), _) => HostKeyCheck::KnownHosts(kh.clone()),
            (None, None, true) => HostKeyCheck::Insecure,
            (None, None, false) => {
                return bad("pin the server: set host_key_fingerprint or known_hosts \
                     (the fingerprint is in the error of the first connection attempt)");
            }
        };
        if matches!(check, HostKeyCheck::Insecure) {
            tracing::warn!(host = %cfg.host, "SFTP host key check is off; do not use this in production");
        }
        Ok(Self {
            cfg: Arc::new(cfg.clone()),
            check,
            conn: Arc::new(tokio::sync::Mutex::new(None)),
            scope: None,
        })
    }

    /// Returns the live connection, connecting if needed.
    async fn conn(&self) -> StorageResult<Ctx> {
        let conn = self.raw_conn().await?;
        let root = self.scope.clone().unwrap_or_else(|| conn.root.clone());
        Ok(Ctx { conn, root })
    }

    async fn raw_conn(&self) -> StorageResult<Arc<Conn>> {
        let mut guard = self.conn.lock().await;
        if let Some(c) = guard.as_ref().filter(|c| c.usable()) {
            return Ok(Arc::clone(c));
        }
        *guard = None;
        let c = Arc::new(
            tokio::time::timeout(CONNECT_TIMEOUT, self.connect())
                .await
                .map_err(|_| {
                    io_other(format!("sftp: connecting to {} timed out", self.cfg.host))
                })??,
        );
        *guard = Some(Arc::clone(&c));
        Ok(c)
    }

    async fn connect(&self) -> StorageResult<Conn> {
        let cfg = &self.cfg;
        let seen = Arc::new(std::sync::Mutex::new(None));
        let handler = Client {
            host: cfg.host.clone(),
            port: cfg.port(),
            check: self.check.clone(),
            seen: Arc::clone(&seen),
        };
        let config = Arc::new(client::Config {
            inactivity_timeout: None,
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max: 3,
            ..Default::default()
        });
        let mut handle =
            match client::connect(config, (cfg.host.as_str(), cfg.port()), handler).await {
                Ok(h) => h,
                Err(russh::Error::UnknownKey) => {
                    let fp = seen.lock().ok().and_then(|s| s.clone()).unwrap_or_default();
                    return Err(io_other(format!(
                        "sftp: host key of {}:{} is not trusted (the server presented {fp})",
                        cfg.host,
                        cfg.port()
                    )));
                }
                Err(e) => return Err(io_other(format!("sftp: connecting to {}: {e}", cfg.host))),
            };
        let ssh = |e: russh::Error| io_other(format!("sftp: {e}"));
        let auth = if let Some(key_path) = &cfg.private_key {
            let key = russh::keys::load_secret_key(key_path, cfg.private_key_passphrase.as_deref())
                .map_err(|e| io_other(format!("sftp: loading {}: {e}", key_path.display())))?;
            let hash = handle
                .best_supported_rsa_hash()
                .await
                .map_err(ssh)?
                .flatten();
            handle
                .authenticate_publickey(
                    &cfg.username,
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                )
                .await
                .map_err(ssh)?
        } else {
            let password = cfg.password()?.unwrap_or_default();
            handle
                .authenticate_password(&cfg.username, password)
                .await
                .map_err(ssh)?
        };
        if !auth.success() {
            return Err(io_other(format!(
                "sftp: authentication as {} failed",
                cfg.username
            )));
        }
        let channel = handle.channel_open_session().await.map_err(ssh)?;
        channel.request_subsystem(true, "sftp").await.map_err(ssh)?;
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| io_other(format!("sftp: starting session: {e}")))?;
        sftp.set_timeout(REQUEST_TIMEOUT_SECS);
        let wanted = if cfg.root.is_empty() {
            "."
        } else {
            cfg.root.as_str()
        };
        let root = sftp.canonicalize(wanted).await.map_err(map_err)?;
        let meta = sftp.metadata(root.as_str()).await.map_err(map_err)?;
        if !meta.is_dir() {
            return Err(io_other(format!("sftp: root {root} is not a directory")));
        }
        Ok(Conn {
            handle,
            sftp,
            root,
            dead: AtomicBool::new(false),
        })
    }

    /// Converts an SFTP error, marking the connection dead on transport errors.
    fn fail(c: &Conn, e: SftpError) -> StorageError {
        if matches!(
            e,
            SftpError::IO(_) | SftpError::Timeout | SftpError::UnexpectedPacket
        ) || matches!(&e, SftpError::Status(s) if matches!(s.status_code, StatusCode::NoConnection | StatusCode::ConnectionLost))
        {
            c.dead.store(true, Ordering::Relaxed);
        }
        map_err(e)
    }

    /// Canonical path of the directory `vp` names, checked to lie inside the root.
    async fn real_dir(&self, c: &Ctx, vp: &VPath) -> StorageResult<String> {
        if vp.is_root() {
            return Ok(c.root.clone());
        }
        let raw = vp
            .components()
            .iter()
            .fold(c.root.clone(), |acc, part| Conn::join(&acc, part));
        let real = c
            .sftp
            .canonicalize(raw)
            .await
            .map_err(|e| Self::fail(c, e))?;
        if !c.inside(&real) {
            return Err(StorageError::OutsideRoot);
        }
        Ok(real)
    }

    /// The path of `vp` itself, with its parent resolved (a final symlink is
    /// not followed). The root maps to the root.
    async fn resolve_entry(&self, c: &Ctx, vp: &VPath) -> StorageResult<String> {
        match (vp.parent(), vp.name()) {
            (Some(parent), Some(name)) => {
                let dir = self.real_dir(c, &parent).await?;
                Ok(Conn::join(&dir, name))
            }
            _ => Ok(c.root.clone()),
        }
    }

    /// The path of `vp` with a final symlink followed and checked.
    async fn resolve_follow(&self, c: &Ctx, vp: &VPath) -> StorageResult<String> {
        let p = self.resolve_entry(c, vp).await?;
        if vp.is_root() {
            return Ok(p);
        }
        match c.sftp.symlink_metadata(p.as_str()).await {
            Ok(m) if m.is_symlink() => {
                let target = c.sftp.canonicalize(p).await.map_err(|e| Self::fail(c, e))?;
                if !c.inside(&target) {
                    return Err(StorageError::OutsideRoot);
                }
                Ok(target)
            }
            Ok(_) => Ok(p),
            Err(e) => Err(Self::fail(c, e)),
        }
    }

    /// `lstat`, with "missing" as `None`.
    async fn lstat(c: &Conn, p: &str) -> StorageResult<Option<Metadata>> {
        match c.sftp.symlink_metadata(p).await {
            Ok(m) => Ok(Some(m)),
            Err(e) => match Self::fail(c, e) {
                StorageError::NotFound => Ok(None),
                other => Err(other),
            },
        }
    }

    /// Refuses an existing target unless `overwrite` and it is not a folder.
    /// Returns whether a file (or link) is in the way.
    async fn check_target(c: &Conn, dst: &str, overwrite: bool) -> StorageResult<bool> {
        match Self::lstat(c, dst).await? {
            Some(m) if m.is_dir() || !overwrite => Err(StorageError::AlreadyExists),
            Some(_) => Ok(true),
            None => Ok(false),
        }
    }

    /// Moves `tmp` onto `dst`, removing an existing `dst` first if asked.
    async fn commit(c: &Conn, tmp: &str, dst: &str, replace: bool) -> StorageResult<()> {
        if replace {
            match c.sftp.remove_file(dst).await {
                Ok(()) => {}
                Err(e) => match Self::fail(c, e) {
                    StorageError::NotFound => {}
                    other => return Err(other),
                },
            }
        } else if Self::lstat(c, dst).await?.is_some() {
            return Err(StorageError::AlreadyExists);
        }
        c.sftp.rename(tmp, dst).await.map_err(|e| Self::fail(c, e))
    }

    /// Streams `data` into a new temporary file next to `dst` and returns its
    /// path and the byte count.
    async fn write_temp(
        c: &Conn,
        dst: &str,
        data: &mut (impl tokio::io::AsyncRead + Unpin + ?Sized),
    ) -> StorageResult<(String, u64)> {
        let (dir, name) = dst.rsplit_once('/').unwrap_or((".", dst));
        let tmp = Conn::join(if dir.is_empty() { "/" } else { dir }, &temp_name(name));
        let result: StorageResult<u64> = async {
            let mut f = c
                .sftp
                .open_with_flags(
                    tmp.as_str(),
                    OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
                )
                .await
                .map_err(|e| Self::fail(c, e))?;
            let n = tokio::io::copy(data, &mut f).await?;
            f.flush().await?;
            f.shutdown().await?;
            Ok(n)
        }
        .await;
        match result {
            Ok(n) => Ok((tmp, n)),
            Err(e) => {
                let _ = c.sftp.remove_file(tmp).await;
                Err(e)
            }
        }
    }

    /// Copies one file through this machine (SFTP has no portable server-side copy).
    async fn copy_file(c: &Conn, src: &str, dst: &str, replace: bool) -> StorageResult<()> {
        let mut from = c.sftp.open(src).await.map_err(|e| Self::fail(c, e))?;
        let (tmp, _) = Self::write_temp(c, dst, &mut from).await?;
        if let Err(e) = Self::commit(c, &tmp, dst, replace).await {
            let _ = c.sftp.remove_file(tmp).await;
            return Err(e);
        }
        Ok(())
    }

    /// Copies a directory tree, skipping symlinks.
    async fn copy_tree(c: &Conn, src: String, dst: String) -> StorageResult<()> {
        let mut stack = vec![(src, dst)];
        while let Some((s, d)) = stack.pop() {
            c.sftp
                .create_dir(d.as_str())
                .await
                .map_err(|e| Self::fail(c, e))?;
            let entries = c
                .sftp
                .read_dir(s.as_str())
                .await
                .map_err(|e| Self::fail(c, e))?;
            for ent in entries {
                let name = ent.file_name();
                if name == "." || name == ".." || is_temp_name(&name) {
                    continue;
                }
                let (from, to) = (Conn::join(&s, &name), Conn::join(&d, &name));
                match ent.file_type() {
                    FileType::Dir => stack.push((from, to)),
                    FileType::File => Self::copy_file(c, &from, &to, false).await?,
                    _ => tracing::debug!(path = %from, "skipping link or special file during copy"),
                }
            }
        }
        Ok(())
    }

    /// Deletes a tree without following symlinks.
    async fn remove_tree(c: &Conn, root: String) -> StorageResult<()> {
        // Depth-first: collect directories, delete files on the way down,
        // then remove the directories deepest first.
        let mut dirs = vec![root.clone()];
        let mut stack = vec![root];
        while let Some(d) = stack.pop() {
            let entries = c
                .sftp
                .read_dir(d.as_str())
                .await
                .map_err(|e| Self::fail(c, e))?;
            for ent in entries {
                let name = ent.file_name();
                if name == "." || name == ".." {
                    continue;
                }
                let p = Conn::join(&d, &name);
                if ent.file_type() == FileType::Dir {
                    dirs.push(p.clone());
                    stack.push(p);
                } else {
                    c.sftp.remove_file(p).await.map_err(|e| Self::fail(c, e))?;
                }
            }
        }
        for d in dirs.into_iter().rev() {
            c.sftp.remove_dir(d).await.map_err(|e| Self::fail(c, e))?;
        }
        Ok(())
    }

    fn entry(vp: VPath, m: &Metadata) -> Entry {
        let name = vp.name().unwrap_or_default().to_owned();
        let is_dir = m.is_dir();
        Entry {
            mime: (!is_dir).then(|| guess_mime(&name)),
            name,
            path: vp,
            kind: if is_dir {
                EntryKind::Dir
            } else {
                EntryKind::File
            },
            size: if is_dir { 0 } else { m.size.unwrap_or(0) },
            modified: m
                .mtime
                .and_then(|t| OffsetDateTime::from_unix_timestamp(i64::from(t)).ok()),
        }
    }

    /// The configured root, as written in the settings.
    pub fn configured_root(&self) -> &str {
        &self.cfg.root
    }

    /// Creates a symlink `name` in the source root pointing at `target`
    /// (an absolute server path). Test support only.
    #[cfg(feature = "conformance")]
    #[doc(hidden)]
    pub async fn create_test_symlink(&self, name: &str, target: &str) -> StorageResult<()> {
        let c = self.conn().await?;
        let link = Conn::join(&c.root, name);
        // OpenSSH's sftp-server swaps the two SSH_FXP_SYMLINK arguments
        // relative to the draft; russh-sftp sends them in draft order.
        c.sftp
            .symlink(target, link.as_str())
            .await
            .map_err(|e| Self::fail(&c, e))
    }
}

#[async_trait]
impl Storage for SftpStorage {
    async fn sub(&self, scope: &VPath) -> StorageResult<Arc<dyn Storage>> {
        let c = self.conn().await?;
        let real = self.resolve_follow(&c, scope).await?;
        let m = c
            .sftp
            .metadata(real.as_str())
            .await
            .map_err(|e| Self::fail(&c, e))?;
        if !m.is_dir() {
            return Err(StorageError::NotADirectory);
        }
        Ok(Arc::new(Self {
            cfg: Arc::clone(&self.cfg),
            check: self.check.clone(),
            conn: Arc::clone(&self.conn),
            scope: Some(real),
        }))
    }

    async fn resolve(&self, vp: &VPath) -> StorageResult<VPath> {
        let c = self.conn().await?;
        let mut existing = vp.clone();
        let mut missing: Vec<String> = Vec::new();
        loop {
            let raw = existing
                .components()
                .iter()
                .fold(c.root.clone(), |acc, part| Conn::join(&acc, part));
            match c.sftp.canonicalize(raw).await {
                Ok(real) => {
                    if !c.inside(&real) {
                        return Err(StorageError::OutsideRoot);
                    }
                    let rel = if c.root == "/" {
                        real.as_str()
                    } else {
                        &real[c.root.len()..]
                    };
                    let mut out = VPath::parse(rel)?;
                    for name in missing.iter().rev() {
                        out = out.join(name)?;
                    }
                    return Ok(out);
                }
                Err(e) => match Self::fail(&c, e) {
                    StorageError::NotFound => {
                        let (Some(parent), Some(name)) = (existing.parent(), existing.name())
                        else {
                            return Err(StorageError::NotFound);
                        };
                        missing.push(name.to_owned());
                        existing = parent;
                    }
                    other => return Err(other),
                },
            }
        }
    }

    async fn list(&self, vp: &VPath) -> StorageResult<Vec<Entry>> {
        let c = self.conn().await?;
        let dir = self.resolve_follow(&c, vp).await?;
        let meta = c
            .sftp
            .metadata(dir.as_str())
            .await
            .map_err(|e| Self::fail(&c, e))?;
        if !meta.is_dir() {
            return Err(StorageError::NotADirectory);
        }
        let entries = c
            .sftp
            .read_dir(dir.as_str())
            .await
            .map_err(|e| Self::fail(&c, e))?;
        let mut out = Vec::new();
        for ent in entries {
            let name = ent.file_name();
            if name == "." || name == ".." || is_temp_name(&name) {
                continue;
            }
            let Ok(child) = vp.join(&name) else {
                continue; // names our path rules refuse cannot be addressed
            };
            let mut meta = ent.metadata();
            if ent.file_type() == FileType::Symlink {
                let p = Conn::join(&dir, &name);
                match c.sftp.canonicalize(p.as_str()).await {
                    Ok(t) if c.inside(&t) => {}
                    _ => continue, // outside the root or dangling: hide it
                }
                match c.sftp.metadata(p).await {
                    Ok(m) => meta = m,
                    Err(_) => continue,
                }
            }
            if matches!(meta.file_type(), FileType::Dir | FileType::File) {
                out.push(Self::entry(child, &meta));
            }
        }
        Ok(out)
    }

    async fn stat(&self, vp: &VPath) -> StorageResult<Entry> {
        let c = self.conn().await?;
        let p = self.resolve_follow(&c, vp).await?;
        let m = c.sftp.metadata(p).await.map_err(|e| Self::fail(&c, e))?;
        Ok(Self::entry(vp.clone(), &m))
    }

    async fn read(&self, vp: &VPath, range: Option<Range<u64>>) -> StorageResult<ByteStream> {
        let c = self.conn().await?;
        let p = self.resolve_follow(&c, vp).await?;
        let m = c
            .sftp
            .metadata(p.as_str())
            .await
            .map_err(|e| Self::fail(&c, e))?;
        if m.is_dir() {
            return Err(StorageError::IsADirectory);
        }
        let size = m.size.unwrap_or(0);
        let mut file = c.sftp.open(p).await.map_err(|e| Self::fail(&c, e))?;
        let stream: ByteStream = match range {
            None => Box::pin(ReaderStream::with_capacity(file, READ_CHUNK)),
            Some(r) => {
                check_range(&r, size)?;
                file.seek(io::SeekFrom::Start(r.start)).await?;
                Box::pin(ReaderStream::with_capacity(
                    file.take(r.end - r.start),
                    READ_CHUNK,
                ))
            }
        };
        // Keep the connection alive while the stream is read.
        let keep = Arc::clone(&c.conn);
        Ok(stream
            .map(move |chunk| {
                let _ = &keep;
                chunk
            })
            .boxed())
    }

    async fn write(&self, vp: &VPath, mut data: ByteReader, overwrite: bool) -> StorageResult<u64> {
        if vp.is_root() {
            return Err(StorageError::IsADirectory);
        }
        let c = self.conn().await?;
        let dst = self.resolve_entry(&c, vp).await?;
        let replace = match Self::lstat(&c, &dst).await? {
            Some(m) if m.is_dir() => return Err(StorageError::IsADirectory),
            Some(_) if !overwrite => return Err(StorageError::AlreadyExists),
            Some(_) => true,
            None => false,
        };
        let (tmp, n) = Self::write_temp(&c, &dst, &mut data).await?;
        if let Err(e) = Self::commit(&c, &tmp, &dst, replace).await {
            let _ = c.sftp.remove_file(tmp).await;
            return Err(e);
        }
        Ok(n)
    }

    async fn mkdir(&self, vp: &VPath) -> StorageResult<()> {
        let c = self.conn().await?;
        let mut cur_vp = VPath::root();
        let mut cur = c.root.clone();
        for part in vp.components() {
            cur_vp = cur_vp.join(part)?;
            let next = Conn::join(&cur, part);
            match Self::lstat(&c, &next).await? {
                Some(_) => {
                    cur = self.resolve_follow(&c, &cur_vp).await?;
                    let m = c
                        .sftp
                        .metadata(cur.as_str())
                        .await
                        .map_err(|e| Self::fail(&c, e))?;
                    if !m.is_dir() {
                        return Err(StorageError::NotADirectory);
                    }
                }
                None => {
                    if let Err(e) = c.sftp.create_dir(next.as_str()).await {
                        // Lost a race with another creator: fine if it is a folder now.
                        let err = Self::fail(&c, e);
                        match Self::lstat(&c, &next).await? {
                            Some(m) if m.is_dir() => {}
                            _ => return Err(err),
                        }
                    }
                    cur = next;
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
        let c = self.conn().await?;
        let src = self.resolve_entry(&c, from).await?;
        if Self::lstat(&c, &src).await?.is_none() {
            return Err(StorageError::NotFound);
        }
        let dst = self.resolve_entry(&c, to).await?;
        let replace = Self::check_target(&c, &dst, overwrite).await?;
        if replace {
            c.sftp
                .remove_file(dst.as_str())
                .await
                .map_err(|e| Self::fail(&c, e))?;
        }
        c.sftp.rename(src, dst).await.map_err(|e| Self::fail(&c, e))
    }

    async fn copy(&self, from: &VPath, to: &VPath, overwrite: bool) -> StorageResult<()> {
        if to.is_root() || from == to {
            return Err(StorageError::AlreadyExists);
        }
        refuse_into_self(from, to)?;
        let c = self.conn().await?;
        let src = self.resolve_follow(&c, from).await?;
        let m = c
            .sftp
            .metadata(src.as_str())
            .await
            .map_err(|e| Self::fail(&c, e))?;
        let dst = self.resolve_entry(&c, to).await?;
        let replace = Self::check_target(&c, &dst, overwrite).await?;
        if m.is_dir() {
            if replace {
                return Err(StorageError::AlreadyExists);
            }
            Self::copy_tree(&c, src, dst).await
        } else {
            Self::copy_file(&c, &src, &dst, replace).await
        }
    }

    async fn delete(&self, vp: &VPath) -> StorageResult<()> {
        refuse_root(vp, "delete")?;
        let c = self.conn().await?;
        let p = self.resolve_entry(&c, vp).await?;
        let Some(m) = Self::lstat(&c, &p).await? else {
            return Err(StorageError::NotFound);
        };
        if m.is_dir() {
            Self::remove_tree(&c, p).await
        } else {
            // Files and symlinks: remove the entry itself, never a link target.
            c.sftp.remove_file(p).await.map_err(|e| Self::fail(&c, e))
        }
    }
}

fn map_err(e: SftpError) -> StorageError {
    match e {
        SftpError::Status(s) => match s.status_code {
            StatusCode::NoSuchFile => StorageError::NotFound,
            StatusCode::PermissionDenied => StorageError::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("sftp: {}", s.error_message),
            )),
            StatusCode::OpUnsupported => {
                StorageError::Unsupported(format!("sftp: {}", s.error_message))
            }
            code => io_other(format!("sftp: {code}: {}", s.error_message)),
        },
        other => io_other(format!("sftp: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SftpConfig {
        SftpConfig {
            host: "h".into(),
            username: "u".into(),
            password: Some("p".into()),
            host_key_fingerprint: Some("SHA256:x".into()),
            ..Default::default()
        }
    }

    #[test]
    fn requires_host_key_pinning_and_credentials() {
        assert!(SftpStorage::new(&cfg()).is_ok());
        let unpinned = SftpConfig {
            host_key_fingerprint: None,
            ..cfg()
        };
        let err = SftpStorage::new(&unpinned).unwrap_err().to_string();
        assert!(err.contains("host_key_fingerprint"), "{err}");
        let no_creds = SftpConfig {
            password: None,
            ..cfg()
        };
        assert!(SftpStorage::new(&no_creds).is_err());
        let no_host = SftpConfig {
            host: String::new(),
            ..cfg()
        };
        assert!(SftpStorage::new(&no_host).is_err());
    }

    #[test]
    fn debug_hides_secrets() {
        let s = SftpStorage::new(&cfg()).unwrap();
        assert!(!format!("{s:?}").contains("\"p\""));
    }

    #[test]
    fn errors_map_to_kinds() {
        let st = |code| {
            SftpError::Status(russh_sftp::protocol::Status {
                id: 0,
                status_code: code,
                error_message: "m".into(),
                language_tag: String::new(),
            })
        };
        assert!(matches!(
            map_err(st(StatusCode::NoSuchFile)),
            StorageError::NotFound
        ));
        assert!(matches!(
            map_err(st(StatusCode::OpUnsupported)),
            StorageError::Unsupported(_)
        ));
        assert!(matches!(map_err(SftpError::Timeout), StorageError::Io(_)));
    }

    #[test]
    fn join_handles_root_slash() {
        assert_eq!(Conn::join("/", "a"), "/a");
        assert_eq!(Conn::join("/x", "a"), "/x/a");
    }
}
