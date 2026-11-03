//! The local file-system backend.
//!
//! Serves a directory on the host (which can be any mount: ext4, ZFS, a FUSE
//! or mergerfs pool, a bind mount in Docker, ...).
//!
//! # Path safety
//!
//! Every [`VPath`] has already been validated (no `..`, no backslashes, no NUL).
//! On top of that this backend:
//!
//! * canonicalises the root once at start-up;
//! * canonicalises the *parent* of every path it touches and checks that the
//!   result lies under the root, so a symlinked directory anywhere on the path
//!   that points outside is refused;
//! * when the final component is itself a symlink, canonicalises its target and
//!   refuses it if it points outside the root or is dangling;
//! * hides such symlinks from directory listings;
//! * never follows symlinks while copying or deleting directory trees.
//!
//! Operations that act on an entry itself (rename, delete) act on a symlink and
//! not on its target. Nothing here goes through a shell.
//!
//! A local user who can change the tree *while* a request is in flight could
//! race these checks (swap a directory for a symlink between the check and the
//! use). The server is meant to own its sources; do not share a source with
//! untrusted local accounts.

use std::{
    io,
    ops::Range,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use futures::StreamExt;
use time::OffsetDateTime;
use tokio::{
    fs,
    io::{AsyncSeekExt, AsyncWriteExt},
};
use tokio_util::io::ReaderStream;

use crate::{
    ByteReader, ByteStream, Entry, EntryKind, Storage, StorageError, StorageResult, VPath,
    guess_mime,
};

/// Size of the chunks produced by [`LocalStorage::read`].
const READ_CHUNK: usize = 64 * 1024;

/// A source backed by a local directory.
#[derive(Debug, Clone)]
pub struct LocalStorage {
    /// The canonical root directory.
    root: PathBuf,
}

impl LocalStorage {
    /// Opens a local source rooted at `root`, which must be an existing directory.
    ///
    /// # Errors
    ///
    /// Fails if the root does not exist or is not a directory.
    pub fn new(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = std::fs::canonicalize(root.as_ref())?;
        if !std::fs::metadata(&root)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} is not a directory", root.display()),
            ));
        }
        Ok(Self { root })
    }

    /// The canonical root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn join(&self, vp: &VPath) -> PathBuf {
        let mut p = self.root.clone();
        for c in vp.components() {
            p.push(c);
        }
        p
    }

    /// Resolves an existing path, following symlinks, and checks it stays
    /// inside the root. Use for reading.
    async fn resolve_follow(&self, vp: &VPath) -> StorageResult<PathBuf> {
        let canon = match fs::canonicalize(self.join(vp)).await {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // Either missing, or a dangling symlink somewhere on the path.
                return Err(StorageError::NotFound);
            }
            Err(e) => return Err(e.into()),
        };
        if !canon.starts_with(&self.root) {
            return Err(StorageError::OutsideRoot);
        }
        Ok(canon)
    }

    /// Resolves the parent (which must exist and be inside the root) and joins
    /// the final name *without* following it. If the final component exists and
    /// is a symlink, its target must be inside the root. Use for writes,
    /// renames and deletes.
    async fn resolve_entry(&self, vp: &VPath) -> StorageResult<PathBuf> {
        let (Some(parent), Some(name)) = (vp.parent(), vp.name()) else {
            return Ok(self.root.clone());
        };
        let parent_canon = self.resolve_follow(&parent).await?;
        if !fs::metadata(&parent_canon).await?.is_dir() {
            return Err(StorageError::NotADirectory);
        }
        let p = parent_canon.join(name);
        self.check_symlink(&p).await?;
        Ok(p)
    }

    /// If `p` is a symlink, refuses it unless its target is inside the root.
    async fn check_symlink(&self, p: &Path) -> StorageResult<()> {
        match fs::symlink_metadata(p).await {
            Ok(m) if m.file_type().is_symlink() => match fs::canonicalize(p).await {
                Ok(t) if t.starts_with(&self.root) => Ok(()),
                // Outside, or dangling (we cannot know where it would lead).
                _ => Err(StorageError::OutsideRoot),
            },
            _ => Ok(()),
        }
    }

    fn entry_from_meta(vp: VPath, meta: &std::fs::Metadata) -> Entry {
        let name = vp.name().unwrap_or("").to_owned();
        let kind = if meta.is_dir() {
            EntryKind::Dir
        } else {
            EntryKind::File
        };
        Entry {
            mime: (kind == EntryKind::File).then(|| guess_mime(&name)),
            size: if kind == EntryKind::File {
                meta.len()
            } else {
                0
            },
            modified: meta.modified().ok().map(OffsetDateTime::from),
            name,
            path: vp,
            kind,
        }
    }

    fn temp_name(target: &Path) -> PathBuf {
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Keep the temp name short enough for 255-byte limits.
        let short: String = name.chars().take(64).collect();
        target.with_file_name(format!(".{short}.{}.fbtmp", uuid::Uuid::new_v4().simple()))
    }

    fn refuse_into_self(from: &VPath, to: &VPath) -> StorageResult<()> {
        if to.starts_with(from) && to != from {
            return Err(StorageError::InvalidPath(
                "cannot move or copy a folder into itself".into(),
            ));
        }
        Ok(())
    }

    /// Prepares `dst` for a rename/copy: refuses an existing target unless
    /// `overwrite` and the target is a file.
    async fn prepare_target(dst: &Path, overwrite: bool) -> StorageResult<()> {
        match fs::symlink_metadata(dst).await {
            Ok(m) => {
                if !overwrite {
                    return Err(StorageError::AlreadyExists);
                }
                if m.is_dir() {
                    return Err(StorageError::AlreadyExists);
                }
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    async fn copy_file_atomic(src: &Path, dst: &Path) -> StorageResult<()> {
        let tmp = Self::temp_name(dst);
        if let Err(e) = fs::copy(src, &tmp).await {
            let _ = fs::remove_file(&tmp).await;
            return Err(e.into());
        }
        if let Err(e) = fs::rename(&tmp, dst).await {
            let _ = fs::remove_file(&tmp).await;
            return Err(e.into());
        }
        Ok(())
    }

    /// Copies a directory tree without following symlinks (they are skipped).
    async fn copy_tree(src: PathBuf, dst: PathBuf) -> StorageResult<()> {
        let mut stack = vec![(src, dst)];
        while let Some((s, d)) = stack.pop() {
            fs::create_dir(&d).await?;
            let mut rd = fs::read_dir(&s).await?;
            while let Some(ent) = rd.next_entry().await? {
                let ft = ent.file_type().await?;
                let target = d.join(ent.file_name());
                if ft.is_symlink() {
                    tracing::debug!(path = %ent.path().display(), "skipping symlink during copy");
                } else if ft.is_dir() {
                    stack.push((ent.path(), target));
                } else if ft.is_file() {
                    fs::copy(ent.path(), &target).await?;
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Storage for LocalStorage {
    async fn list(&self, vp: &VPath) -> StorageResult<Vec<Entry>> {
        let dir = self.resolve_follow(vp).await?;
        if !fs::metadata(&dir).await?.is_dir() {
            return Err(StorageError::NotADirectory);
        }
        let mut out = Vec::new();
        let mut rd = fs::read_dir(&dir).await?;
        while let Some(ent) = rd.next_entry().await? {
            let Some(name) = ent.file_name().to_str().map(str::to_owned) else {
                tracing::warn!(path = %ent.path().display(), "skipping non-UTF-8 file name");
                continue;
            };
            let Ok(child) = vp.join(&name) else {
                // Names our path rules refuse (e.g. containing '\') cannot be addressed.
                continue;
            };
            let path = ent.path();
            let ft = match ent.file_type().await {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_symlink() {
                match fs::canonicalize(&path).await {
                    Ok(t) if t.starts_with(&self.root) => {}
                    _ => continue, // outside the root or dangling: hide it
                }
            }
            if name.ends_with(".fbtmp") && name.starts_with('.') {
                continue; // in-flight atomic write
            }
            match fs::metadata(&path).await {
                Ok(meta) => out.push(Self::entry_from_meta(child, &meta)),
                Err(e) => tracing::debug!(error = %e, "stat failed during listing"),
            }
        }
        Ok(out)
    }

    async fn stat(&self, vp: &VPath) -> StorageResult<Entry> {
        let p = self.resolve_follow(vp).await?;
        let meta = fs::metadata(&p).await?;
        Ok(Self::entry_from_meta(vp.clone(), &meta))
    }

    async fn read(&self, vp: &VPath, range: Option<Range<u64>>) -> StorageResult<ByteStream> {
        let p = self.resolve_follow(vp).await?;
        let meta = fs::metadata(&p).await?;
        if meta.is_dir() {
            return Err(StorageError::IsADirectory);
        }
        let size = meta.len();
        let mut file = fs::File::open(&p).await?;
        let stream: ByteStream = match range {
            None => Box::pin(ReaderStream::with_capacity(file, READ_CHUNK)),
            Some(r) => {
                if r.start > r.end || r.end > size {
                    return Err(StorageError::InvalidRange { size });
                }
                file.seek(io::SeekFrom::Start(r.start)).await?;
                use tokio::io::AsyncReadExt;
                let limited = file.take(r.end - r.start);
                Box::pin(ReaderStream::with_capacity(limited, READ_CHUNK))
            }
        };
        Ok(stream.boxed())
    }

    async fn write(&self, vp: &VPath, mut data: ByteReader, overwrite: bool) -> StorageResult<u64> {
        if vp.is_root() {
            return Err(StorageError::IsADirectory);
        }
        let target = self.resolve_entry(vp).await?;
        match fs::symlink_metadata(&target).await {
            Ok(m) if m.is_dir() => return Err(StorageError::IsADirectory),
            Ok(_) if !overwrite => return Err(StorageError::AlreadyExists),
            _ => {}
        }
        let tmp = Self::temp_name(&target);
        let result: StorageResult<u64> = async {
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
                .await?;
            let n = tokio::io::copy(&mut data, &mut f).await?;
            f.flush().await?;
            f.sync_all().await?;
            drop(f);
            if !overwrite && fs::symlink_metadata(&target).await.is_ok() {
                return Err(StorageError::AlreadyExists);
            }
            fs::rename(&tmp, &target).await?;
            Ok(n)
        }
        .await;
        if result.is_err() {
            let _ = fs::remove_file(&tmp).await;
        }
        result
    }

    async fn mkdir(&self, vp: &VPath) -> StorageResult<()> {
        let mut cur_vp = VPath::root();
        let mut cur = self.root.clone();
        for c in vp.components() {
            cur_vp = cur_vp.join(c)?;
            let next = cur.join(c);
            match fs::symlink_metadata(&next).await {
                Ok(_) => {
                    cur = self.resolve_follow(&cur_vp).await?;
                    if !fs::metadata(&cur).await?.is_dir() {
                        return Err(StorageError::NotADirectory);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    match fs::create_dir(&next).await {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(e) => return Err(e.into()),
                    }
                    cur = next;
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    async fn rename(&self, from: &VPath, to: &VPath, overwrite: bool) -> StorageResult<()> {
        if from.is_root() || to.is_root() {
            return Err(StorageError::InvalidPath(
                "cannot move the source root".into(),
            ));
        }
        if from == to {
            return Ok(());
        }
        Self::refuse_into_self(from, to)?;
        let src = self.resolve_entry(from).await?;
        let src_meta = fs::symlink_metadata(&src).await?;
        let dst = self.resolve_entry(to).await?;
        Self::prepare_target(&dst, overwrite).await?;
        match fs::rename(&src, &dst).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                // Different file systems (e.g. some mergerfs set-ups): copy then delete.
                if src_meta.is_dir() {
                    Self::copy_tree(src.clone(), dst).await?;
                    fs::remove_dir_all(&src).await?;
                } else {
                    Self::copy_file_atomic(&src, &dst).await?;
                    fs::remove_file(&src).await?;
                }
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn copy(&self, from: &VPath, to: &VPath, overwrite: bool) -> StorageResult<()> {
        if to.is_root() {
            return Err(StorageError::AlreadyExists);
        }
        Self::refuse_into_self(from, to)?;
        if from == to {
            return Err(StorageError::AlreadyExists);
        }
        let src = self.resolve_follow(from).await?;
        let dst = self.resolve_entry(to).await?;
        Self::prepare_target(&dst, overwrite).await?;
        if fs::metadata(&src).await?.is_dir() {
            if fs::symlink_metadata(&dst).await.is_ok() {
                return Err(StorageError::AlreadyExists);
            }
            Self::copy_tree(src, dst).await
        } else {
            Self::copy_file_atomic(&src, &dst).await
        }
    }

    async fn delete(&self, vp: &VPath) -> StorageResult<()> {
        if vp.is_root() {
            return Err(StorageError::InvalidPath(
                "cannot delete the source root".into(),
            ));
        }
        let p = self.resolve_entry(vp).await?;
        let meta = fs::symlink_metadata(&p).await?;
        if meta.file_type().is_symlink() {
            // Remove the link itself, never its target.
            match fs::remove_file(&p).await {
                Ok(()) => Ok(()),
                Err(_) if cfg!(windows) => Ok(fs::remove_dir(&p).await?),
                Err(e) => Err(e.into()),
            }
        } else if meta.is_dir() {
            // std's remove_dir_all does not follow symlinks inside the tree.
            Ok(fs::remove_dir_all(&p).await?)
        } else {
            Ok(fs::remove_file(&p).await?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::TryStreamExt;

    async fn read_all(s: &LocalStorage, p: &str, r: Option<Range<u64>>) -> StorageResult<Vec<u8>> {
        let stream = s.read(&VPath::parse(p).unwrap(), r).await?;
        let chunks: Vec<bytes::Bytes> = stream.try_collect().await?;
        Ok(chunks.concat())
    }

    fn reader(data: &'static [u8]) -> ByteReader {
        Box::pin(data)
    }

    fn vp(s: &str) -> VPath {
        VPath::parse(s).unwrap()
    }

    fn setup() -> (tempfile::TempDir, LocalStorage) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("root")).unwrap();
        std::fs::write(dir.path().join("secret.txt"), b"top secret").unwrap();
        let s = LocalStorage::new(dir.path().join("root")).unwrap();
        (dir, s)
    }

    #[tokio::test]
    async fn write_read_list_stat() {
        let (_d, s) = setup();
        s.mkdir(&vp("a/b")).await.unwrap();
        assert_eq!(
            s.write(&vp("a/b/f.txt"), reader(b"hello world"), false)
                .await
                .unwrap(),
            11
        );
        assert_eq!(
            read_all(&s, "a/b/f.txt", None).await.unwrap(),
            b"hello world"
        );
        assert_eq!(
            read_all(&s, "a/b/f.txt", Some(6..11)).await.unwrap(),
            b"world"
        );
        assert_eq!(read_all(&s, "a/b/f.txt", Some(0..0)).await.unwrap(), b"");
        assert!(matches!(
            read_all(&s, "a/b/f.txt", Some(5..12)).await,
            Err(StorageError::InvalidRange { size: 11 })
        ));
        let list = s.list(&vp("a/b")).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "f.txt");
        assert_eq!(list[0].mime.as_deref(), Some("text/plain"));
        let st = s.stat(&vp("a")).await.unwrap();
        assert!(st.is_dir());
        assert!(matches!(
            s.stat(&vp("nope")).await,
            Err(StorageError::NotFound)
        ));
    }

    #[tokio::test]
    async fn write_refuses_overwrite_unless_asked() {
        let (_d, s) = setup();
        s.write(&vp("f"), reader(b"1"), false).await.unwrap();
        assert!(matches!(
            s.write(&vp("f"), reader(b"2"), false).await,
            Err(StorageError::AlreadyExists)
        ));
        s.write(&vp("f"), reader(b"2"), true).await.unwrap();
        assert_eq!(read_all(&s, "f", None).await.unwrap(), b"2");
        // No temp files left behind.
        assert_eq!(s.list(&VPath::root()).await.unwrap().len(), 1);
        assert_eq!(std::fs::read_dir(s.root()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn rename_copy_delete() {
        let (_d, s) = setup();
        s.mkdir(&vp("d/sub")).await.unwrap();
        s.write(&vp("d/sub/x"), reader(b"x"), false).await.unwrap();
        s.copy(&vp("d"), &vp("d2"), false).await.unwrap();
        assert_eq!(read_all(&s, "d2/sub/x", None).await.unwrap(), b"x");
        assert!(matches!(
            s.copy(&vp("d"), &vp("d/sub/inner"), false).await,
            Err(StorageError::InvalidPath(_))
        ));
        assert!(matches!(
            s.rename(&vp("d"), &vp("d/sub/inner"), false).await,
            Err(StorageError::InvalidPath(_))
        ));
        assert!(matches!(
            s.rename(&vp("d"), &vp("d2"), false).await,
            Err(StorageError::AlreadyExists)
        ));
        s.rename(&vp("d2"), &vp("moved"), false).await.unwrap();
        assert!(s.stat(&vp("moved/sub/x")).await.is_ok());
        s.delete(&vp("moved")).await.unwrap();
        assert!(matches!(
            s.stat(&vp("moved")).await,
            Err(StorageError::NotFound)
        ));
        assert!(s.delete(&VPath::root()).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_outside_root_are_refused() {
        use std::os::unix::fs::symlink;
        let (d, s) = setup();
        let root = d.path().join("root");
        symlink(d.path().join("secret.txt"), root.join("evil-file")).unwrap();
        symlink(d.path(), root.join("evil-dir")).unwrap();
        symlink(root.join("missing"), root.join("dangling")).unwrap();
        std::fs::write(root.join("ok.txt"), b"ok").unwrap();
        symlink(root.join("ok.txt"), root.join("good-link")).unwrap();

        // Reads through outside links are refused.
        assert!(matches!(
            read_all(&s, "evil-file", None).await,
            Err(StorageError::OutsideRoot)
        ));
        assert!(matches!(
            read_all(&s, "evil-dir/secret.txt", None).await,
            Err(StorageError::OutsideRoot)
        ));
        assert!(matches!(
            s.list(&vp("evil-dir")).await,
            Err(StorageError::OutsideRoot)
        ));
        assert!(matches!(
            s.stat(&vp("evil-dir")).await,
            Err(StorageError::OutsideRoot)
        ));
        // Writes through an outside directory link are refused.
        assert!(matches!(
            s.write(&vp("evil-dir/planted"), reader(b"x"), false).await,
            Err(StorageError::OutsideRoot)
        ));
        assert!(!d.path().join("planted").exists());
        // Overwriting through an outside file link is refused.
        assert!(matches!(
            s.write(&vp("evil-file"), reader(b"x"), true).await,
            Err(StorageError::OutsideRoot)
        ));
        assert_eq!(
            std::fs::read(d.path().join("secret.txt")).unwrap(),
            b"top secret"
        );
        // mkdir through an outside link is refused.
        assert!(matches!(
            s.mkdir(&vp("evil-dir/new")).await,
            Err(StorageError::OutsideRoot)
        ));
        // Copying out of or into outside links is refused.
        assert!(
            s.copy(&vp("evil-file"), &vp("stolen"), false)
                .await
                .is_err()
        );
        assert!(
            s.copy(&vp("ok.txt"), &vp("evil-dir/ok.txt"), false)
                .await
                .is_err()
        );
        // Outside and dangling links are hidden from listings.
        let mut names: Vec<_> = s
            .list(&VPath::root())
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        names.sort();
        assert_eq!(names, ["good-link", "ok.txt"]);
        // Links inside the root work.
        assert_eq!(read_all(&s, "good-link", None).await.unwrap(), b"ok");
        // Deleting a link removes the link only.
        s.delete(&vp("good-link")).await.unwrap();
        assert!(root.join("ok.txt").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn delete_tree_does_not_follow_links() {
        use std::os::unix::fs::symlink;
        let (d, s) = setup();
        let root = d.path().join("root");
        std::fs::create_dir(root.join("t")).unwrap();
        symlink(d.path(), root.join("t/escape")).unwrap();
        s.delete(&vp("t")).await.unwrap();
        assert!(d.path().join("secret.txt").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn root_itself_may_be_a_symlink() {
        use std::os::unix::fs::symlink;
        let (d, _) = setup();
        symlink(d.path().join("root"), d.path().join("rootlink")).unwrap();
        let s = LocalStorage::new(d.path().join("rootlink")).unwrap();
        s.write(&vp("f"), reader(b"1"), false).await.unwrap();
        assert!(d.path().join("root/f").exists());
    }
}
