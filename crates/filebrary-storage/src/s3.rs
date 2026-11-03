//! The S3-compatible object storage backend.
//!
//! Works with AWS S3 and anything that speaks its API: MinIO, Garage,
//! SeaweedFS, Ceph RGW, Cloudflare R2, Backblaze B2, Wasabi, ...
//!
//! # Mapping files onto objects
//!
//! A source is a bucket, optionally narrowed to a key prefix. A file at
//! `/a/b.txt` is the object `<prefix>/a/b.txt`. Folders are key prefixes, so
//! they exist as long as something is inside them. To let users create empty
//! folders, [`S3Storage::mkdir`] writes a zero-byte marker object named
//! [`DIR_MARKER`] inside the folder; markers are hidden from listings and
//! cannot be addressed through the API. Folder objects written by other tools
//! (keys ending in `/`) are understood too.
//!
//! Keys are the path components joined with `/`, unencoded, so objects
//! uploaded by other S3 tools show up under their real names.
//!
//! # Limits
//!
//! S3 has no rename: [`S3Storage::rename`] copies every object server side,
//! then deletes the originals, so moving a large folder takes time and is not
//! atomic. A failure part way leaves both copies; nothing is lost. The
//! `overwrite = false` check before a write is a separate request, so two
//! clients writing the same new name at the same moment can both succeed (the
//! last one wins). Each object write itself is atomic: readers see the old or
//! the new content, never a mix.
//!
//! # Path safety
//!
//! Every key is built from a validated [`VPath`] under the configured prefix,
//! and object stores have no links, so nothing can resolve outside the prefix.

use std::{ops::Range, sync::Arc};

use async_trait::async_trait;
use futures::{StreamExt, TryStreamExt};
use object_store::{
    GetOptions, GetRange, ObjectStore, ObjectStoreExt, WriteMultipart,
    aws::{AmazonS3, AmazonS3Builder},
    path::Path,
};
use serde::Deserialize;
use time::OffsetDateTime;

use crate::{
    ByteReader, ByteStream, Entry, EntryKind, Storage, StorageError, StorageResult, VPath,
    guess_mime,
    util::{check_range, io_other, read_chunk, refuse_into_self, refuse_root},
};

/// Name of the zero-byte object that keeps an empty folder in existence.
pub const DIR_MARKER: &str = ".filebrary-folder";

/// Uploads up to this size go up in one request; larger ones use multipart
/// upload with parts of this size (S3 needs at least 5 MiB per part).
const PART_SIZE: usize = 8 * 1024 * 1024;

/// Multipart parts uploaded at the same time.
const PART_CONCURRENCY: usize = 4;

/// Settings for an S3-compatible source.
///
/// ```toml
/// [[sources]]
/// name = "photos"
/// kind = "s3"
///
/// [sources.s3]
/// bucket = "family-photos"
/// prefix = "library"                 # optional
/// endpoint = "https://minio.lan:9000" # omit for AWS
/// region = "us-east-1"
/// access_key_id = "AKIA..."
/// secret_access_key_file = "/run/secrets/s3"
/// ```
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct S3Config {
    /// Bucket name.
    pub bucket: String,
    /// Key prefix inside the bucket that acts as the source root (optional).
    #[serde(default)]
    pub prefix: String,
    /// Endpoint URL for non-AWS services, e.g. `http://minio:9000`.
    /// Plain `http://` endpoints are accepted only when given explicitly here.
    pub endpoint: Option<String>,
    /// Region (default `us-east-1`; most non-AWS services ignore it).
    pub region: Option<String>,
    /// Access key ID. When unset, credentials come from the usual AWS
    /// environment variables, web identity or instance metadata.
    pub access_key_id: Option<String>,
    /// Secret access key.
    pub secret_access_key: Option<String>,
    /// File holding the secret access key (wins over `secret_access_key`).
    pub secret_access_key_file: Option<std::path::PathBuf>,
    /// Session token for temporary credentials.
    pub session_token: Option<String>,
    /// Use virtual-hosted-style URLs (`bucket.host`). Default: `false` when an
    /// `endpoint` is set (path style, which MinIO and most others want), `true`
    /// for AWS.
    pub virtual_hosted_style: Option<bool>,
}

impl S3Config {
    fn secret(&self) -> std::io::Result<Option<String>> {
        if let Some(f) = &self.secret_access_key_file {
            return Ok(Some(std::fs::read_to_string(f)?.trim().to_owned()));
        }
        Ok(self.secret_access_key.clone())
    }
}

/// A source backed by an S3-compatible bucket.
#[derive(Debug, Clone)]
pub struct S3Storage {
    store: Arc<AmazonS3>,
    /// Key prefix components, validated like any other path.
    prefix: VPath,
}

impl S3Storage {
    /// Builds the client. No request is made until the source is used.
    ///
    /// # Errors
    ///
    /// Fails on an invalid bucket name, endpoint or prefix, or an unreadable
    /// secret file.
    pub fn new(cfg: &S3Config) -> StorageResult<Self> {
        install_tls_provider();
        if cfg.bucket.is_empty() {
            return Err(StorageError::InvalidPath("s3: bucket is required".into()));
        }
        let prefix = VPath::parse(&cfg.prefix)?;
        let mut b = AmazonS3Builder::new()
            .with_bucket_name(&cfg.bucket)
            .with_region(cfg.region.as_deref().unwrap_or("us-east-1"));
        if let Some(ep) = &cfg.endpoint {
            b = b
                .with_endpoint(ep)
                .with_allow_http(ep.starts_with("http://"))
                .with_virtual_hosted_style_request(cfg.virtual_hosted_style.unwrap_or(false));
        } else if let Some(v) = cfg.virtual_hosted_style {
            b = b.with_virtual_hosted_style_request(v);
        }
        if let Some(id) = &cfg.access_key_id {
            b = b.with_access_key_id(id);
        }
        if let Some(secret) = cfg.secret()? {
            b = b.with_secret_access_key(secret);
        }
        if let Some(t) = &cfg.session_token {
            b = b.with_token(t);
        }
        let store = b.build().map_err(|e| io_other(format!("s3: {e}")))?;
        Ok(Self {
            store: Arc::new(store),
            prefix,
        })
    }

    /// The object key for a path (the prefix itself for the root).
    fn key(&self, vp: &VPath) -> StorageResult<Path> {
        let full = self.prefix.join_path(vp);
        let s = full.components().join("/");
        Path::parse(&s).map_err(|e| StorageError::InvalidPath(e.to_string()))
    }

    /// Maps a key back to a source path, or `None` for keys outside the prefix.
    fn vpath_of(&self, key: &Path) -> Option<VPath> {
        let parsed = VPath::parse(key.as_ref()).ok()?;
        parsed.strip_prefix(&self.prefix)
    }

    /// The key prefix of everything inside `vp`, i.e. `key/` (or the prefix
    /// root).
    fn child_prefix(&self, vp: &VPath) -> StorageResult<Option<Path>> {
        let key = self.key(vp)?;
        Ok(if key.as_ref().is_empty() {
            None
        } else {
            Some(key)
        })
    }

    fn refuse_marker(vp: &VPath) -> StorageResult<()> {
        if vp.name() == Some(DIR_MARKER) {
            return Err(StorageError::InvalidPath(format!(
                "{DIR_MARKER} is reserved"
            )));
        }
        Ok(())
    }

    /// Returns the object's metadata if a file exists at `vp`.
    async fn head_file(&self, vp: &VPath) -> StorageResult<Option<object_store::ObjectMeta>> {
        if vp.is_root() {
            return Ok(None);
        }
        match self.store.head(&self.key(vp)?).await {
            Ok(m) => Ok(Some(m)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }

    /// Returns `true` if anything exists under `vp/` (or `vp` is the root).
    async fn dir_exists(&self, vp: &VPath) -> StorageResult<bool> {
        if vp.is_root() {
            return Ok(true);
        }
        let prefix = self.key(vp)?;
        let mut stream = self.store.list(Some(&prefix));
        match stream.next().await {
            Some(Ok(_)) => Ok(true),
            Some(Err(e)) => Err(map_err(e)),
            None => Ok(false),
        }
    }

    /// Lists every object key under `vp/`, recursively (markers included).
    async fn list_tree(&self, vp: &VPath) -> StorageResult<Vec<object_store::ObjectMeta>> {
        let prefix = self.child_prefix(vp)?;
        self.store
            .list(prefix.as_ref())
            .map_err(map_err)
            .try_collect()
            .await
    }

    /// Fails unless the parent of `vp` is an existing folder.
    async fn require_parent(&self, vp: &VPath) -> StorageResult<()> {
        let Some(parent) = vp.parent() else {
            return Ok(());
        };
        if parent.is_root() {
            return Ok(());
        }
        if self.dir_exists(&parent).await? {
            Ok(())
        } else if self.head_file(&parent).await?.is_some() {
            Err(StorageError::NotADirectory)
        } else {
            Err(StorageError::NotFound)
        }
    }

    /// Fails if `to` exists and may not be replaced: folders never are, files
    /// only with `overwrite`.
    async fn prepare_target(&self, to: &VPath, overwrite: bool) -> StorageResult<()> {
        if self.dir_exists(to).await? {
            return Err(StorageError::AlreadyExists);
        }
        if !overwrite && self.head_file(to).await?.is_some() {
            return Err(StorageError::AlreadyExists);
        }
        Ok(())
    }

    /// Copies a file or folder; the source must exist.
    async fn copy_any(&self, from: &VPath, to: &VPath) -> StorageResult<bool> {
        if self.head_file(from).await?.is_some() {
            self.store
                .copy(&self.key(from)?, &self.key(to)?)
                .await
                .map_err(map_err)?;
            return Ok(false);
        }
        let objects = self.list_tree(from).await?;
        if objects.is_empty() {
            return Err(StorageError::NotFound);
        }
        for o in &objects {
            let Some(rel) = self
                .vpath_of(&o.location)
                .and_then(|p| p.strip_prefix(from))
            else {
                continue;
            };
            let dst = self.key(&to.join_path(&rel))?;
            self.store.copy(&o.location, &dst).await.map_err(map_err)?;
        }
        Ok(true)
    }

    async fn delete_keys(&self, keys: Vec<Path>) -> StorageResult<()> {
        let stream = futures::stream::iter(keys.into_iter().map(Ok)).boxed();
        self.store
            .delete_stream(stream)
            .map_err(map_err)
            .try_for_each(|_| async { Ok(()) })
            .await
    }

    fn file_entry(vp: VPath, meta: &object_store::ObjectMeta) -> Entry {
        let name = vp.name().unwrap_or_default().to_owned();
        Entry {
            mime: Some(guess_mime(&name)),
            name,
            path: vp,
            kind: EntryKind::File,
            size: meta.size,
            modified: to_time(meta),
        }
    }

    fn dir_entry(vp: VPath) -> Entry {
        Entry {
            name: vp.name().unwrap_or_default().to_owned(),
            path: vp,
            kind: EntryKind::Dir,
            size: 0,
            modified: None,
            mime: None,
        }
    }
}

#[async_trait]
impl Storage for S3Storage {
    async fn list(&self, vp: &VPath) -> StorageResult<Vec<Entry>> {
        let prefix = self.child_prefix(vp)?;
        let res = self
            .store
            .list_with_delimiter(prefix.as_ref())
            .await
            .map_err(map_err)?;
        if res.objects.is_empty() && res.common_prefixes.is_empty() && !vp.is_root() {
            return Err(if self.head_file(vp).await?.is_some() {
                StorageError::NotADirectory
            } else {
                StorageError::NotFound
            });
        }
        let mut out = Vec::new();
        for p in &res.common_prefixes {
            if let Some(child) = self.vpath_of(p).filter(|c| c.parent().as_ref() == Some(vp)) {
                out.push(Self::dir_entry(child));
            }
        }
        for o in &res.objects {
            let Some(child) = self.vpath_of(&o.location) else {
                continue;
            };
            // A `dir/` folder object lists as the folder itself; skip it, and
            // skip our own markers.
            if child == *vp || child.name() == Some(DIR_MARKER) {
                continue;
            }
            if child.parent().as_ref() == Some(vp) {
                out.push(Self::file_entry(child, o));
            }
        }
        Ok(out)
    }

    async fn stat(&self, vp: &VPath) -> StorageResult<Entry> {
        Self::refuse_marker(vp)?;
        if let Some(m) = self.head_file(vp).await? {
            return Ok(Self::file_entry(vp.clone(), &m));
        }
        if self.dir_exists(vp).await? {
            return Ok(Self::dir_entry(vp.clone()));
        }
        Err(StorageError::NotFound)
    }

    async fn read(&self, vp: &VPath, range: Option<Range<u64>>) -> StorageResult<ByteStream> {
        Self::refuse_marker(vp)?;
        let Some(meta) = self.head_file(vp).await? else {
            return Err(if self.dir_exists(vp).await? {
                StorageError::IsADirectory
            } else {
                StorageError::NotFound
            });
        };
        if let Some(r) = &range {
            check_range(r, meta.size)?;
            if r.start == r.end {
                return Ok(futures::stream::empty().boxed());
            }
        }
        if meta.size == 0 {
            return Ok(futures::stream::empty().boxed());
        }
        let opts = GetOptions {
            range: range.map(GetRange::Bounded),
            // Fail rather than mix two versions if the object changes between
            // the HEAD above and this GET.
            if_match: meta.e_tag.clone(),
            ..Default::default()
        };
        let res = self
            .store
            .get_opts(&self.key(vp)?, opts)
            .await
            .map_err(map_err)?;
        Ok(res
            .into_stream()
            .map_err(|e| std::io::Error::other(e.to_string()))
            .boxed())
    }

    async fn write(&self, vp: &VPath, mut data: ByteReader, overwrite: bool) -> StorageResult<u64> {
        if vp.is_root() || self.dir_exists(vp).await? {
            return Err(StorageError::IsADirectory);
        }
        Self::refuse_marker(vp)?;
        self.require_parent(vp).await?;
        if !overwrite && self.head_file(vp).await?.is_some() {
            return Err(StorageError::AlreadyExists);
        }
        let key = self.key(vp)?;
        let first = read_chunk(&mut data, PART_SIZE).await?;
        if first.len() < PART_SIZE {
            let n = first.len() as u64;
            self.store.put(&key, first.into()).await.map_err(map_err)?;
            return Ok(n);
        }
        let upload = self.store.put_multipart(&key).await.map_err(map_err)?;
        let mut w = WriteMultipart::new_with_chunk_size(upload, PART_SIZE);
        let mut total = first.len() as u64;
        w.put(first);
        let result: StorageResult<()> = async {
            loop {
                w.wait_for_capacity(PART_CONCURRENCY)
                    .await
                    .map_err(map_err)?;
                let chunk = read_chunk(&mut data, PART_SIZE).await?;
                if chunk.is_empty() {
                    return Ok(());
                }
                total += chunk.len() as u64;
                w.put(chunk);
            }
        }
        .await;
        match result {
            Ok(()) => {
                w.finish().await.map_err(map_err)?;
                Ok(total)
            }
            Err(e) => {
                if let Err(abort) = w.abort().await {
                    tracing::warn!(error = %abort, "aborting multipart upload failed");
                }
                Err(e)
            }
        }
    }

    async fn mkdir(&self, vp: &VPath) -> StorageResult<()> {
        if vp.is_root() {
            return Ok(());
        }
        Self::refuse_marker(vp)?;
        // Every existing ancestor must be a folder, not a file.
        let mut cur = VPath::root();
        for c in vp.components() {
            cur = cur.join(c)?;
            if self.head_file(&cur).await?.is_some() {
                return Err(StorageError::NotADirectory);
            }
        }
        if self.dir_exists(vp).await? {
            return Ok(());
        }
        let marker = self.key(&vp.join(DIR_MARKER)?)?;
        self.store
            .put(&marker, bytes::Bytes::new().into())
            .await
            .map_err(map_err)?;
        Ok(())
    }

    async fn rename(&self, from: &VPath, to: &VPath, overwrite: bool) -> StorageResult<()> {
        refuse_root(from, "move")?;
        refuse_root(to, "move onto")?;
        Self::refuse_marker(from)?;
        Self::refuse_marker(to)?;
        if from == to {
            return Ok(());
        }
        refuse_into_self(from, to)?;
        if self.head_file(from).await?.is_none() && !self.dir_exists(from).await? {
            return Err(StorageError::NotFound);
        }
        self.prepare_target(to, overwrite).await?;
        self.require_parent(to).await?;
        if self.copy_any(from, to).await? {
            let keys = self
                .list_tree(from)
                .await?
                .into_iter()
                .map(|o| o.location)
                .collect();
            self.delete_keys(keys).await
        } else {
            self.store.delete(&self.key(from)?).await.map_err(map_err)
        }
    }

    async fn copy(&self, from: &VPath, to: &VPath, overwrite: bool) -> StorageResult<()> {
        if to.is_root() || from == to {
            return Err(StorageError::AlreadyExists);
        }
        Self::refuse_marker(from)?;
        Self::refuse_marker(to)?;
        refuse_into_self(from, to)?;
        if self.head_file(from).await?.is_none() && !self.dir_exists(from).await? {
            return Err(StorageError::NotFound);
        }
        self.prepare_target(to, overwrite).await?;
        self.require_parent(to).await?;
        self.copy_any(from, to).await.map(|_| ())
    }

    async fn delete(&self, vp: &VPath) -> StorageResult<()> {
        refuse_root(vp, "delete")?;
        Self::refuse_marker(vp)?;
        let file = self.head_file(vp).await?.is_some();
        let mut keys: Vec<Path> = self
            .list_tree(vp)
            .await?
            .into_iter()
            .map(|o| o.location)
            .collect();
        if file {
            keys.push(self.key(vp)?);
        }
        if keys.is_empty() {
            return Err(StorageError::NotFound);
        }
        self.delete_keys(keys).await
    }
}

fn to_time(meta: &object_store::ObjectMeta) -> Option<OffsetDateTime> {
    let t = meta.last_modified;
    let nanos = i128::from(t.timestamp()) * 1_000_000_000 + i128::from(t.timestamp_subsec_nanos());
    OffsetDateTime::from_unix_timestamp_nanos(nanos).ok()
}

fn map_err(e: object_store::Error) -> StorageError {
    match e {
        object_store::Error::NotFound { .. } => StorageError::NotFound,
        object_store::Error::AlreadyExists { .. } => StorageError::AlreadyExists,
        object_store::Error::InvalidPath { source } => {
            StorageError::InvalidPath(source.to_string())
        }
        other => io_other(format!("s3: {other}")),
    }
}

/// Makes rustls use the ring provider (the build carries no other), once.
fn install_tls_provider() {
    // Fails harmlessly if a provider is already installed.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage(prefix: &str) -> S3Storage {
        S3Storage::new(&S3Config {
            bucket: "b".into(),
            prefix: prefix.into(),
            endpoint: Some("http://127.0.0.1:1".into()),
            access_key_id: Some("k".into()),
            secret_access_key: Some("s".into()),
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn keys_are_prefixed_and_unencoded() {
        let s = storage("lib/2024");
        let k = s.key(&VPath::parse("a b/[x]%41.txt").unwrap()).unwrap();
        assert_eq!(k.as_ref(), "lib/2024/a b/[x]%41.txt");
        assert_eq!(s.key(&VPath::root()).unwrap().as_ref(), "lib/2024");
        let back = s.vpath_of(&k).unwrap();
        assert_eq!(back.to_string(), "/a b/[x]%41.txt");
        // Keys outside the prefix map to nothing.
        assert!(s.vpath_of(&Path::parse("lib/other/x").unwrap()).is_none());
        let root = storage("");
        assert_eq!(root.key(&VPath::parse("x").unwrap()).unwrap().as_ref(), "x");
        assert!(root.child_prefix(&VPath::root()).unwrap().is_none());
    }

    #[test]
    fn rejects_bad_config() {
        assert!(S3Storage::new(&S3Config::default()).is_err());
        let bad_prefix = S3Config {
            bucket: "b".into(),
            prefix: "../x".into(),
            ..Default::default()
        };
        assert!(S3Storage::new(&bad_prefix).is_err());
    }

    #[test]
    fn control_characters_are_refused() {
        let s = storage("");
        assert!(matches!(
            s.key(&VPath::parse("a\nb").unwrap()),
            Err(StorageError::InvalidPath(_))
        ));
    }

    #[test]
    fn markers_are_reserved() {
        assert!(S3Storage::refuse_marker(&VPath::parse("a/.filebrary-folder").unwrap()).is_err());
        assert!(S3Storage::refuse_marker(&VPath::parse("a/b").unwrap()).is_ok());
    }
}
