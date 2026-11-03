//! The on-disk preview cache.
//!
//! Entries live under `<root>/<first two hex chars>/<key>.<ext>`. A key is a
//! SHA-256 over the source name, the file path, its size, its modification
//! time and the variant, so editing a file simply produces a new key and the
//! old entry ages out. Writes are atomic (temp file plus rename), hits bump
//! the entry's modification time, and [`PreviewCache::sweep`] deletes the
//! least recently used entries once the cache grows past its size limit.
//!
//! Failed generations leave a small `<key>.fail` marker, so a corrupt file is
//! not re-processed on every request; markers expire after
//! [`PreviewCache::FAILURE_TTL`].

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::error::Result;

/// A cache key: lowercase hex SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey(String);

impl CacheKey {
    /// Build the key for one variant of one version of a file.
    pub fn new(
        source: &str,
        path: &str,
        size: u64,
        modified: Option<SystemTime>,
        variant: &str,
    ) -> Self {
        let mtime = modified
            .and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut h = Sha256::new();
        // Length-prefix the free-form parts so ("a","bc") and ("ab","c") differ.
        for part in [source.as_bytes(), path.as_bytes(), variant.as_bytes()] {
            h.update((part.len() as u64).to_le_bytes());
            h.update(part);
        }
        h.update(size.to_le_bytes());
        h.update(mtime.to_le_bytes());
        CacheKey(hex::encode(h.finalize()))
    }

    /// The hex string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A cached file and its content type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedFile {
    /// Absolute path of the cached file. Serve it, do not modify it.
    pub path: PathBuf,
    /// MIME type derived from the extension.
    pub content_type: &'static str,
}

/// Extensions an entry can have, with their MIME types.
const KINDS: &[(&str, &str)] = &[
    ("jpg", "image/jpeg"),
    ("png", "image/png"),
    ("pdf", "application/pdf"),
];

/// Handle to the cache directory.
#[derive(Debug, Clone)]
pub struct PreviewCache {
    root: PathBuf,
    max_bytes: u64,
}

impl PreviewCache {
    /// How long a failure marker suppresses retries.
    pub const FAILURE_TTL: Duration = Duration::from_secs(3600);

    /// Open (and create) a cache at `root` that [`sweep`](Self::sweep) keeps
    /// under `max_bytes`.
    pub fn new(root: impl Into<PathBuf>, max_bytes: u64) -> std::io::Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root, max_bytes })
    }

    /// The cache directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn base(&self, key: &CacheKey) -> PathBuf {
        self.root.join(&key.0[..2]).join(&key.0)
    }

    /// Look an entry up. A hit refreshes its modification time so the LRU
    /// sweep keeps it.
    pub async fn get(&self, key: &CacheKey) -> Option<CachedFile> {
        let base = self.base(key);
        for (ext, ct) in KINDS {
            let p = base.with_extension(ext);
            if tokio::fs::metadata(&p).await.is_ok() {
                let touch = p.clone();
                // Best effort: a failed touch only makes eviction less precise.
                let _ = tokio::task::spawn_blocking(move || {
                    std::fs::File::options()
                        .write(true)
                        .open(&touch)?
                        .set_modified(SystemTime::now())
                })
                .await;
                return Some(CachedFile {
                    path: p,
                    content_type: ct,
                });
            }
        }
        None
    }

    /// Store `bytes` under `key` with extension `ext` (`jpg`, `png` or `pdf`).
    pub async fn put(
        &self,
        key: &CacheKey,
        ext: &'static str,
        bytes: Vec<u8>,
    ) -> Result<CachedFile> {
        let content_type = KINDS
            .iter()
            .find(|(e, _)| *e == ext)
            .map(|(_, ct)| *ct)
            .expect("known cache extension");
        let target = self.base(key).with_extension(ext);
        let fail = self.base(key).with_extension("fail");
        let t = target.clone();
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            let dir = t.parent().expect("has parent");
            std::fs::create_dir_all(dir)?;
            let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
            tmp.write_all(&bytes)?;
            tmp.persist(&t).map_err(|e| e.error)?;
            let _ = std::fs::remove_file(fail);
            Ok(())
        })
        .await
        .map_err(std::io::Error::other)??;
        Ok(CachedFile {
            path: target,
            content_type,
        })
    }

    /// Whether a recent failure is recorded for `key`.
    pub async fn failed_recently(&self, key: &CacheKey) -> bool {
        let p = self.base(key).with_extension("fail");
        match tokio::fs::metadata(&p).await.and_then(|m| m.modified()) {
            Ok(m) => m
                .elapsed()
                .map(|age| age < Self::FAILURE_TTL)
                .unwrap_or(true),
            Err(_) => false,
        }
    }

    /// Record a failure for `key`, with the reason for debugging.
    pub async fn mark_failed(&self, key: &CacheKey, reason: &str) {
        let p = self.base(key).with_extension("fail");
        if let Some(dir) = p.parent() {
            let _ = tokio::fs::create_dir_all(dir).await;
        }
        let _ = tokio::fs::write(p, reason.as_bytes()).await;
    }

    /// Delete expired failure markers, stray temp files, and then the least
    /// recently used entries until the cache fits in its size limit. Returns
    /// the number of bytes freed.
    pub async fn sweep(&self) -> std::io::Result<u64> {
        let root = self.root.clone();
        let max = self.max_bytes;
        tokio::task::spawn_blocking(move || sweep_blocking(&root, max))
            .await
            .map_err(std::io::Error::other)?
    }

    /// Start a background task that runs [`sweep`](Self::sweep) every
    /// `every`. The task stops when the returned handle is aborted.
    pub fn spawn_janitor(&self, every: Duration) -> tokio::task::JoinHandle<()> {
        let cache = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            loop {
                tick.tick().await;
                match cache.sweep().await {
                    Ok(0) => {}
                    Ok(freed) => tracing::info!(freed, "preview cache swept"),
                    Err(e) => tracing::warn!(error = %e, "preview cache sweep failed"),
                }
            }
        })
    }
}

fn sweep_blocking(root: &Path, max: u64) -> std::io::Result<u64> {
    let mut entries = Vec::new();
    let mut total = 0u64;
    let mut freed = 0u64;
    let now = SystemTime::now();
    for shard in std::fs::read_dir(root)? {
        let shard = shard?;
        if !shard.file_type()?.is_dir() {
            continue;
        }
        for f in std::fs::read_dir(shard.path())? {
            let f = f?;
            let meta = match f.metadata() {
                Ok(m) if m.is_file() => m,
                _ => continue,
            };
            let path = f.path();
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let age = now.duration_since(mtime).unwrap_or_default();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default();
            let known = KINDS.iter().any(|(e, _)| *e == ext);
            let expired_fail = ext == "fail" && age >= PreviewCache::FAILURE_TTL;
            // Leftover temp files from a crash (tempfile names start with ".tmp").
            let stale_tmp = !known && ext != "fail" && age > Duration::from_secs(3600);
            if expired_fail || stale_tmp {
                if std::fs::remove_file(&path).is_ok() {
                    freed += meta.len();
                }
                continue;
            }
            if known {
                total += meta.len();
                entries.push((mtime, meta.len(), path));
            }
        }
    }
    if total > max {
        entries.sort_by_key(|(m, _, _)| *m);
        for (_, len, path) in entries {
            if total <= max {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total -= len;
                freed += len;
            }
        }
    }
    Ok(freed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_change_with_every_input() {
        let t = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(5));
        let k = CacheKey::new("s", "/a", 1, t, "thumb");
        assert_ne!(k, CacheKey::new("s2", "/a", 1, t, "thumb"));
        assert_ne!(k, CacheKey::new("s", "/b", 1, t, "thumb"));
        assert_ne!(k, CacheKey::new("s", "/a", 2, t, "thumb"));
        assert_ne!(k, CacheKey::new("s", "/a", 1, None, "thumb"));
        assert_ne!(k, CacheKey::new("s", "/a", 1, t, "large"));
        assert_ne!(
            CacheKey::new("a", "bc", 0, None, "v"),
            CacheKey::new("ab", "c", 0, None, "v")
        );
        assert_eq!(k.as_str().len(), 64);
    }

    #[tokio::test]
    async fn put_get_and_failure_markers() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PreviewCache::new(dir.path(), 1 << 20).unwrap();
        let k = CacheKey::new("s", "/a.jpg", 3, None, "thumb");
        assert!(cache.get(&k).await.is_none());
        cache.mark_failed(&k, "boom").await;
        assert!(cache.failed_recently(&k).await);
        let put = cache.put(&k, "png", vec![1, 2, 3]).await.unwrap();
        assert_eq!(put.content_type, "image/png");
        assert!(
            !cache.failed_recently(&k).await,
            "success clears the marker"
        );
        let got = cache.get(&k).await.unwrap();
        assert_eq!(got, put);
        assert_eq!(std::fs::read(got.path).unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn sweep_evicts_least_recently_used() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PreviewCache::new(dir.path(), 250).unwrap();
        let mut keys = Vec::new();
        for i in 0..3u64 {
            let k = CacheKey::new("s", "/f", i, None, "thumb");
            let f = cache.put(&k, "jpg", vec![0; 100]).await.unwrap();
            let old = SystemTime::now() - Duration::from_secs(100 - i * 10);
            std::fs::File::options()
                .write(true)
                .open(&f.path)
                .unwrap()
                .set_modified(old)
                .unwrap();
            keys.push(k);
        }
        // Use the oldest one: it becomes the most recent.
        assert!(cache.get(&keys[0]).await.is_some());
        let freed = cache.sweep().await.unwrap();
        assert_eq!(freed, 100);
        assert!(cache.get(&keys[0]).await.is_some());
        assert!(
            cache.get(&keys[1]).await.is_none(),
            "least recently used is gone"
        );
        assert!(cache.get(&keys[2]).await.is_some());
    }
}
