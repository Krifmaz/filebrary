//! Moving data between storage sources and a job's private work folder, and
//! a blocking `Read + Seek` view of a stored file for archive readers.

use std::{
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    sync::Arc,
};

use bytes::Bytes;
use futures::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::{JobError, JobResult};
use crate::{
    auth::{Perms, User},
    error::AppResult,
    state::AppState,
    storage::{ByteStream, Entry, Storage, StorageError, VPath},
};

/// Parses the user-visible `path`, checks `perm` on it and returns the
/// source's backend confined to the user's scope (see
/// [`crate::api::files::scoped`]) with the parsed path. Paths given to the
/// returned backend are user-visible paths, never scope-prefixed ones.
///
/// # Errors
///
/// `NotFound`, `Forbidden` or `BadRequest` as for any request.
pub async fn resolve(
    state: &AppState,
    user: &User,
    source: &str,
    path: &str,
    perm: Perms,
) -> AppResult<(Arc<dyn Storage>, VPath)> {
    let vp = VPath::parse(path)?;
    let storage = crate::api::files::scoped(state, user, source, perm, &vp).await?;
    Ok((storage, vp))
}

/// Splits a file name into stem and extension (`"a.tar.gz"` gives
/// `("a.tar", "gz")`; a leading dot is part of the stem).
pub fn split_name(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
    }
}

/// The lower-cased extension of `name` if it is 1–10 ASCII letters or
/// digits, else an empty string. Safe to use in a temporary file name.
pub fn safe_ext(name: &str) -> String {
    let (_, ext) = split_name(name);
    if !ext.is_empty() && ext.len() <= 10 && ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
        ext.to_ascii_lowercase()
    } else {
        String::new()
    }
}

/// Picks a name in `dir` that does not exist yet: `stem.ext`, then
/// `stem (1).ext`, `stem (2).ext`, ...
///
/// # Errors
///
/// Storage failures other than "not found", or more than 1000 taken names.
pub async fn unique_target(
    storage: &dyn Storage,
    dir: &VPath,
    stem: &str,
    ext: &str,
) -> JobResult<VPath> {
    let stem: String = stem.chars().take(200).collect();
    for i in 0..1000 {
        let name = match (i, ext.is_empty()) {
            (0, true) => stem.clone(),
            (0, false) => format!("{stem}.{ext}"),
            (_, true) => format!("{stem} ({i})"),
            (_, false) => format!("{stem} ({i}).{ext}"),
        };
        let cand = dir
            .join(&name)
            .map_err(|e| JobError::failed(format!("invalid output name: {e}")))?;
        match storage.stat(&cand).await {
            Err(StorageError::NotFound) => return Ok(cand),
            Err(e) => return Err(e.into()),
            Ok(_) => {}
        }
    }
    Err(JobError::failed("could not find a free output file name"))
}

/// Copies a stored file into the local file `dst`, calling `on_bytes` with
/// each chunk's size.
///
/// # Errors
///
/// Storage and I/O failures, or [`JobError::Cancelled`].
pub async fn download(
    storage: &dyn Storage,
    path: &VPath,
    dst: &Path,
    cancel: &CancellationToken,
    mut on_bytes: impl FnMut(u64),
) -> JobResult<u64> {
    let mut stream = storage.read(path, None).await?;
    let mut f = tokio::fs::File::create(dst).await?;
    let mut total = 0;
    loop {
        let chunk = tokio::select! {
            c = stream.next() => c,
            () = cancel.cancelled() => return Err(JobError::Cancelled),
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk?;
        f.write_all(&chunk).await?;
        total += chunk.len() as u64;
        on_bytes(chunk.len() as u64);
    }
    f.flush().await?;
    Ok(total)
}

/// Uploads the local file `src` to `dst` (atomically where the backend
/// supports it).
///
/// # Errors
///
/// Storage and I/O failures.
pub async fn upload(
    storage: &dyn Storage,
    dst: &VPath,
    src: &Path,
    overwrite: bool,
) -> JobResult<u64> {
    let f = tokio::fs::File::open(src).await?;
    Ok(storage.write(dst, Box::pin(f), overwrite).await?)
}

/// Uploads `src` as a new file in `dir` named `stem.ext` (or a free variant)
/// and returns the path written.
///
/// # Errors
///
/// As [`unique_target`] and [`upload`]. If another writer takes the name in
/// between, the next free name is tried.
pub async fn upload_new(
    storage: &dyn Storage,
    dir: &VPath,
    stem: &str,
    ext: &str,
    src: &Path,
) -> JobResult<VPath> {
    for _ in 0..5 {
        let target = unique_target(storage, dir, stem, ext).await?;
        match upload(storage, &target, src, false).await {
            Ok(_) => return Ok(target),
            Err(JobError::Failed(m)) if m.contains("already exists") => continue,
            Err(e) => return Err(e),
        }
    }
    Err(JobError::failed("could not find a free output file name"))
}

/// Walks `root` recursively (a file yields just itself), calling `visit` for
/// every entry below it, parents before children. Stops with an error after
/// `limit` entries.
///
/// # Errors
///
/// Storage failures, the limit, or [`JobError::Cancelled`].
pub async fn walk(
    storage: &dyn Storage,
    root: &VPath,
    limit: usize,
    cancel: &CancellationToken,
) -> JobResult<Vec<Entry>> {
    let top = storage.stat(root).await?;
    let mut out = vec![];
    let mut stack = vec![top];
    while let Some(e) = stack.pop() {
        if cancel.is_cancelled() {
            return Err(JobError::Cancelled);
        }
        if out.len() >= limit {
            return Err(JobError::failed(format!(
                "the selection has more than {limit} files and folders"
            )));
        }
        if e.is_dir() {
            let mut kids = storage.list(&e.path).await?;
            kids.sort_by(|a, b| b.name.cmp(&a.name));
            stack.extend(kids);
        }
        out.push(e);
    }
    Ok(out)
}

/// A blocking, seekable reader over a stored file, for archive readers that
/// need `Read + Seek` (zip and 7z keep their index at the end of the file).
///
/// Sequential reads reuse one open stream; a seek elsewhere opens a ranged
/// read from the new position, so only the parts an archive reader asks for
/// are transferred. Must only be used from a blocking thread
/// ([`tokio::task::spawn_blocking`]).
pub struct StorageReader {
    storage: Arc<dyn Storage>,
    path: VPath,
    size: u64,
    pos: u64,
    stream: Option<ByteStream>,
    stream_pos: u64,
    buf: Bytes,
    rt: tokio::runtime::Handle,
}

impl StorageReader {
    /// A reader for `path`, whose size is `size`. Call from async code; then
    /// move the reader into a blocking task.
    pub fn new(storage: Arc<dyn Storage>, path: VPath, size: u64) -> Self {
        Self {
            storage,
            path,
            size,
            pos: 0,
            stream: None,
            stream_pos: 0,
            buf: Bytes::new(),
            rt: tokio::runtime::Handle::current(),
        }
    }
}

impl Read for StorageReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.size || out.is_empty() {
            return Ok(0);
        }
        // The buffered chunk starts at `stream_pos - buf.len()`.
        let buf_start = self.stream_pos - self.buf.len() as u64;
        if self.stream.is_none() || self.pos < buf_start || self.pos > self.stream_pos {
            let (storage, path, pos, size) =
                (self.storage.clone(), self.path.clone(), self.pos, self.size);
            let s = self
                .rt
                .block_on(async move { storage.read(&path, Some(pos..size)).await })
                .map_err(storage_to_io)?;
            self.stream = Some(s);
            self.stream_pos = self.pos;
            self.buf = Bytes::new();
        }
        // Skip what lies before `pos` in the current buffer.
        let buf_start = self.stream_pos - self.buf.len() as u64;
        let skip = usize::try_from(self.pos - buf_start).unwrap_or(usize::MAX);
        if skip > 0 {
            self.buf = self.buf.slice(skip.min(self.buf.len())..);
        }
        if self.buf.is_empty() {
            let stream = self.stream.as_mut().expect("opened above");
            match self.rt.block_on(stream.next()) {
                None => return Ok(0),
                Some(chunk) => {
                    let chunk = chunk?;
                    self.stream_pos += chunk.len() as u64;
                    self.buf = chunk;
                }
            }
        }
        let n = out.len().min(self.buf.len());
        out[..n].copy_from_slice(&self.buf[..n]);
        self.buf = self.buf.slice(n..);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for StorageReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let new = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.size.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before start"))?;
        self.pos = new;
        Ok(new)
    }
}

fn storage_to_io(e: StorageError) -> io::Error {
    match e {
        StorageError::Io(e) => e,
        StorageError::NotFound => io::Error::new(io::ErrorKind::NotFound, "not found"),
        other => io::Error::other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::local::LocalStorage;

    #[test]
    fn names() {
        assert_eq!(split_name("a.tar.gz"), ("a.tar", "gz"));
        assert_eq!(split_name(".hidden"), (".hidden", ""));
        assert_eq!(split_name("noext"), ("noext", ""));
        assert_eq!(safe_ext("Movie.MKV"), "mkv");
        assert_eq!(safe_ext("x.$(id)"), "");
        assert_eq!(safe_ext("x.verylongextension"), "");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn storage_reader_reads_and_seeks() {
        let dir = tempfile::tempdir().unwrap();
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(dir.path().join("f.bin"), &data).unwrap();
        let st: Arc<dyn Storage> = Arc::new(LocalStorage::new(dir.path()).unwrap());
        let p = VPath::parse("f.bin").unwrap();
        let mut r = StorageReader::new(st, p, data.len() as u64);
        let data2 = data.clone();
        tokio::task::spawn_blocking(move || {
            let mut all = Vec::new();
            r.read_to_end(&mut all).unwrap();
            assert_eq!(all, data2);
            r.seek(SeekFrom::End(-10)).unwrap();
            let mut tail = [0u8; 10];
            r.read_exact(&mut tail).unwrap();
            assert_eq!(&tail[..], &data2[data2.len() - 10..]);
            r.seek(SeekFrom::Start(1000)).unwrap();
            let mut mid = [0u8; 5];
            r.read_exact(&mut mid).unwrap();
            assert_eq!(&mid[..], &data2[1000..1005]);
            r.seek(SeekFrom::Current(-5)).unwrap();
            r.read_exact(&mut mid).unwrap();
            assert_eq!(&mid[..], &data2[1000..1005]);
            assert!(r.seek(SeekFrom::Current(-100_000)).is_err());
        })
        .await
        .unwrap();
    }
}
