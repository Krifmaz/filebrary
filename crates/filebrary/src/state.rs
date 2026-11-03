//! Shared application state handed to every request handler.

use std::{collections::BTreeMap, ops::Deref, path::PathBuf, sync::Arc};

use anyhow::Context;
use sqlx::SqlitePool;

use crate::{
    config::{Config, SourceKind},
    search::Search,
    storage::{
        Source, Storage, local::LocalStorage, s3::S3Storage, sftp::SftpStorage, smb::SmbStorage,
    },
};

/// Cheaply cloneable handle to the server state.
#[derive(Clone)]
pub struct AppState(Arc<Inner>);

/// The state itself. Reach it through [`AppState`]'s `Deref`.
pub struct Inner {
    /// Effective configuration.
    pub config: Config,
    /// Database pool.
    pub db: SqlitePool,
    /// Configured sources by name.
    pub sources: BTreeMap<String, Source>,
    /// 32-byte server secret (signs share-unlock cookies and similar).
    pub secret: [u8; 32],
    /// Directory where resumable uploads are staged.
    pub upload_dir: PathBuf,
    /// Indexed search. Background indexing starts with [`crate::search::Search::start`].
    pub search: Search,
    /// Upload ids with a chunk currently being written (one writer at a time).
    pub uploads_in_flight: std::sync::Mutex<std::collections::HashSet<String>>,
    /// The full-text content index, or `None` when `[fulltext] enabled =
    /// false` or no source has `content_search` switched on.
    pub content_index: Option<Arc<filebrary_fulltext::ContentIndex>>,
    /// Login rate limiter and auth hooks.
    pub auth: crate::auth::AuthRuntime,
}

impl Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl AppState {
    /// Builds the state: opens every source (creating missing local roots).
    ///
    /// # Errors
    ///
    /// Fails if a source root cannot be opened or the upload directory cannot
    /// be created.
    pub fn new(config: Config, db: SqlitePool, secret: [u8; 32]) -> anyhow::Result<Self> {
        let mut sources = BTreeMap::new();
        for sc in &config.sources {
            let storage = open_source(sc)?;
            sources.insert(
                sc.name.clone(),
                Source {
                    name: sc.name.clone(),
                    storage,
                },
            );
        }
        let upload_dir = config.data_dir.join("uploads");
        std::fs::create_dir_all(&upload_dir)
            .with_context(|| format!("creating {}", upload_dir.display()))?;
        let names: Vec<String> = sources.keys().cloned().collect();
        let search = Search::new(db.clone(), config.search.clone(), &names);
        let content_index = crate::content::open_index(&config)?;
        let auth = crate::auth::AuthRuntime::new(&config);
        Ok(Self(Arc::new(Inner {
            search,
            auth,
            config,
            db,
            sources,
            secret,
            upload_dir,
            uploads_in_flight: Default::default(),
            content_index,
        })))
    }

    /// Looks up a source by name.
    pub fn source(&self, name: &str) -> Option<&Source> {
        self.sources.get(name)
    }
}

/// Opens the backend for one source. Remote backends only check their
/// settings here and connect on first use, so an unreachable server does not
/// stop the others from starting.
fn open_source(sc: &crate::config::SourceConfig) -> anyhow::Result<Arc<dyn Storage>> {
    let what = |kind: &str| format!("opening {kind} source {}", sc.name);
    // `Config::validate` guarantees the table for the kind is present.
    let missing = || anyhow::anyhow!("source {}: backend settings missing", sc.name);
    Ok(match sc.kind {
        SourceKind::Local => {
            std::fs::create_dir_all(&sc.path)
                .with_context(|| format!("creating source {} at {}", sc.name, sc.path.display()))?;
            Arc::new(
                LocalStorage::new(&sc.path).with_context(|| {
                    format!("opening source {} at {}", sc.name, sc.path.display())
                })?,
            )
        }
        SourceKind::S3 => Arc::new(
            S3Storage::new(sc.s3.as_ref().ok_or_else(missing)?).with_context(|| what("S3"))?,
        ),
        SourceKind::Sftp => Arc::new(
            SftpStorage::new(sc.sftp.as_ref().ok_or_else(missing)?)
                .with_context(|| what("SFTP"))?,
        ),
        SourceKind::Smb => Arc::new(
            SmbStorage::new(sc.smb.as_ref().ok_or_else(missing)?).with_context(|| what("SMB"))?,
        ),
    })
}
