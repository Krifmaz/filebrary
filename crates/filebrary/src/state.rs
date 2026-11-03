//! Shared application state handed to every request handler.

use std::{collections::BTreeMap, ops::Deref, path::PathBuf, sync::Arc};

use anyhow::Context;
use filebrary_preview::{PreviewCache, PreviewConfig, PreviewService, Tools};
use sqlx::SqlitePool;

use crate::{
    config::{Config, SourceKind},
    storage::{Source, local::LocalStorage},
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
    /// Upload ids with a chunk currently being written (one writer at a time).
    pub uploads_in_flight: std::sync::Mutex<std::collections::HashSet<String>>,
    /// Login rate limiter and auth hooks.
    pub auth: crate::auth::AuthRuntime,
    /// Thumbnail and preview generator with its cache (`None` when disabled).
    pub previews: Option<PreviewService>,
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
            let storage = match sc.kind {
                SourceKind::Local => {
                    std::fs::create_dir_all(&sc.path).with_context(|| {
                        format!("creating source {} at {}", sc.name, sc.path.display())
                    })?;
                    LocalStorage::new(&sc.path).with_context(|| {
                        format!("opening source {} at {}", sc.name, sc.path.display())
                    })?
                }
            };
            sources.insert(
                sc.name.clone(),
                Source {
                    name: sc.name.clone(),
                    storage: Arc::new(storage),
                },
            );
        }
        let upload_dir = config.data_dir.join("uploads");
        std::fs::create_dir_all(&upload_dir)
            .with_context(|| format!("creating {}", upload_dir.display()))?;
        let auth = crate::auth::AuthRuntime::new(&config);
        let previews = if config.previews.enabled {
            Some(build_previews(&config)?)
        } else {
            None
        };
        Ok(Self(Arc::new(Inner {
            auth,
            config,
            db,
            sources,
            secret,
            upload_dir,
            uploads_in_flight: Default::default(),
            previews,
        })))
    }

    /// Looks up a source by name.
    pub fn source(&self, name: &str) -> Option<&Source> {
        self.sources.get(name)
    }
}

/// Sets up the preview service: finds the external tools on `PATH` and opens
/// the cache under `<data_dir>/cache/previews`.
fn build_previews(config: &Config) -> anyhow::Result<PreviewService> {
    let ps = &config.previews;
    let dir = config.data_dir.join("cache").join("previews");
    let cache = PreviewCache::new(&dir, ps.cache_mib.saturating_mul(1 << 20))
        .with_context(|| format!("creating {}", dir.display()))?;
    let temp_dir = config.data_dir.join("cache").join("tmp");
    std::fs::create_dir_all(&temp_dir)
        .with_context(|| format!("creating {}", temp_dir.display()))?;
    let mut cfg = PreviewConfig {
        temp_dir,
        thumb_size: ps.thumb_size,
        large_size: ps.large_size,
        ..PreviewConfig::default()
    };
    if ps.max_parallel > 0 {
        cfg.max_parallel = ps.max_parallel;
    }
    let tools = Tools::detect();
    tracing::info!(tools = %tools.summary(), "preview tools");
    Ok(PreviewService::new(cfg, tools, cache))
}
