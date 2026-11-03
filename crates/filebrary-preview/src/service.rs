//! [`PreviewService`]: the one entry point the server uses.
//!
//! It picks a pipeline from the file kind, serves from the cache when it can,
//! makes sure two requests for the same preview only generate it once, and
//! limits how many generations run at the same time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use tokio::io::AsyncReadExt;
use tokio::sync::Semaphore;

use crate::cache::{CacheKey, CachedFile, PreviewCache};
use crate::document;
use crate::error::{PreviewError, Result};
use crate::kind::FileKind;
use crate::media::{self, MediaInfo, TranscodeStream, TranscodeTarget};
use crate::raster::{self, Encoded};
use crate::tools::Tools;

/// Which preview of a file is wanted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variant {
    /// A small image for grids and lists ([`PreviewConfig::thumb_size`]).
    Thumb,
    /// A screen-sized image for the viewer ([`PreviewConfig::large_size`]),
    /// for formats the browser cannot show itself (RAW, HEIC, TIFF, ...).
    Large,
    /// A PDF rendering, for Office documents.
    Pdf,
}

impl Variant {
    /// Name used in URLs and cache keys.
    pub fn as_str(self) -> &'static str {
        match self {
            Variant::Thumb => "thumb",
            Variant::Large => "large",
            Variant::Pdf => "pdf",
        }
    }

    /// Parse the URL name.
    pub fn parse(s: &str) -> Option<Variant> {
        Some(match s {
            "thumb" | "small" => Variant::Thumb,
            "large" | "big" => Variant::Large,
            "pdf" => Variant::Pdf,
            _ => return None,
        })
    }
}

/// Tunables. [`Default`] gives sensible values.
#[derive(Debug, Clone)]
pub struct PreviewConfig {
    /// Longest side of thumbnails, in pixels.
    pub thumb_size: u32,
    /// Longest side of large previews, in pixels.
    pub large_size: u32,
    /// How many previews may be generated at once.
    pub max_parallel: usize,
    /// How many LibreOffice conversions may run at once (each is a whole
    /// office suite in memory).
    pub max_parallel_office: usize,
    /// Time limit for ffmpeg, pdftoppm and heif-dec.
    pub tool_timeout: Duration,
    /// Time limit for one LibreOffice conversion.
    pub office_timeout: Duration,
    /// Images larger than this are not decoded in-process.
    pub max_image_bytes: u64,
    /// RAW files larger than this are not scanned.
    pub max_raw_bytes: u64,
    /// Office files larger than this are not converted.
    pub max_office_bytes: u64,
    /// Where tools write scratch files and LibreOffice its per-run profile.
    /// Defaults to the system temp directory; the server points it at
    /// `<data_dir>/cache/tmp` so it works on a read-only root filesystem.
    pub temp_dir: PathBuf,
}

impl Default for PreviewConfig {
    fn default() -> Self {
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2);
        Self {
            thumb_size: 256,
            large_size: 1920,
            max_parallel: cpus.clamp(1, 8),
            max_parallel_office: 1,
            tool_timeout: Duration::from_secs(60),
            office_timeout: Duration::from_secs(180),
            max_image_bytes: 256 << 20,
            max_raw_bytes: 512 << 20,
            max_office_bytes: 100 << 20,
            temp_dir: std::env::temp_dir(),
        }
    }
}

/// The file a preview is wanted for.
#[derive(Debug, Clone)]
pub struct PreviewInput {
    /// Source name (part of the cache key).
    pub source: String,
    /// Path inside the source, as shown to users (part of the cache key and
    /// used to pick the pipeline from the extension).
    pub path: String,
    /// Absolute local path the tools read. For remote sources this is a
    /// temporary copy the caller made.
    pub local: PathBuf,
    /// File size in bytes.
    pub size: u64,
    /// Modification time, if known.
    pub modified: Option<SystemTime>,
}

/// What to send back for a preview request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewOutput {
    /// Serve this cached file.
    Cached(CachedFile),
    /// The browser can show the original file; send it (or redirect to the
    /// download URL) instead of a converted copy.
    Original,
}

/// Generates and caches previews. Cheap to clone.
#[derive(Clone)]
pub struct PreviewService {
    inner: Arc<Inner>,
}

struct Inner {
    cfg: PreviewConfig,
    tools: Tools,
    cache: PreviewCache,
    jobs: Semaphore,
    office: Semaphore,
    inflight: Mutex<HashMap<CacheKey, Arc<tokio::sync::Mutex<()>>>>,
}

impl PreviewService {
    /// Build a service over `cache` using `tools`.
    pub fn new(cfg: PreviewConfig, tools: Tools, cache: PreviewCache) -> Self {
        let jobs = Semaphore::new(cfg.max_parallel.max(1));
        let office = Semaphore::new(cfg.max_parallel_office.max(1));
        Self {
            inner: Arc::new(Inner {
                cfg,
                tools,
                cache,
                jobs,
                office,
                inflight: Mutex::default(),
            }),
        }
    }

    /// The tools this service found.
    pub fn tools(&self) -> &Tools {
        &self.inner.tools
    }

    /// The cache.
    pub fn cache(&self) -> &PreviewCache {
        &self.inner.cache
    }

    /// The configuration.
    pub fn config(&self) -> &PreviewConfig {
        &self.inner.cfg
    }

    /// Whether a thumbnail can be made for files of this kind with the tools
    /// that are installed. The UI uses this to avoid requesting thumbnails
    /// that would only fail.
    pub fn supports(&self, kind: FileKind) -> bool {
        let t = &self.inner.tools;
        match kind {
            FileKind::Image | FileKind::Raw => true,
            FileKind::Heif => t.heif_dec.is_some() || t.ffmpeg.is_some(),
            FileKind::Video | FileKind::Audio => t.ffmpeg.is_some(),
            FileKind::Pdf => t.pdftoppm.is_some(),
            FileKind::Office => t.soffice.is_some() && t.pdftoppm.is_some(),
        }
    }

    /// Get (generating if needed) a preview of `input`.
    pub async fn get(&self, input: &PreviewInput, variant: Variant) -> Result<PreviewOutput> {
        let head = read_head(&input.local).await;
        let kind = FileKind::detect(Path::new(&input.path), head.as_deref())
            .ok_or(PreviewError::Unsupported)?;
        match (kind, variant) {
            (FileKind::Pdf, Variant::Pdf) => return Ok(PreviewOutput::Original),
            (FileKind::Office, Variant::Pdf) => {}
            (_, Variant::Pdf) => return Err(PreviewError::Unsupported),
            (FileKind::Image, Variant::Large)
                if FileKind::browser_native(Path::new(&input.path)) =>
            {
                return Ok(PreviewOutput::Original);
            }
            _ => {}
        }
        if !self.supports(kind)
            && !(kind == FileKind::Office
                && variant == Variant::Pdf
                && self.inner.tools.soffice.is_some())
        {
            return Err(PreviewError::ToolMissing(missing_tool(kind)));
        }

        let key = CacheKey::new(
            &input.source,
            &input.path,
            input.size,
            input.modified,
            variant.as_str(),
        );
        let cache = &self.inner.cache;
        if let Some(hit) = cache.get(&key).await {
            return Ok(PreviewOutput::Cached(hit));
        }
        if cache.failed_recently(&key).await {
            return Err(PreviewError::RecentlyFailed);
        }

        // Office thumbnails are rendered from the cached PDF conversion.
        // Get it before taking a generation slot so the two never wait on
        // each other.
        let office_pdf = if kind == FileKind::Office && variant != Variant::Pdf {
            match Box::pin(self.get(input, Variant::Pdf)).await? {
                PreviewOutput::Cached(c) => Some(c.path),
                PreviewOutput::Original => None,
            }
        } else {
            None
        };

        // Single flight: one generation per key, later callers wait for it.
        let lock = {
            let mut map = self.inner.inflight.lock().expect("inflight lock");
            map.entry(key.clone()).or_default().clone()
        };
        let _guard = lock.lock().await;
        let result = async {
            if let Some(hit) = cache.get(&key).await {
                return Ok(PreviewOutput::Cached(hit));
            }
            if cache.failed_recently(&key).await {
                return Err(PreviewError::RecentlyFailed);
            }
            let generated = self
                .generate(kind, variant, input, office_pdf.as_deref())
                .await;
            match generated {
                Ok((ext, bytes)) => Ok(PreviewOutput::Cached(cache.put(&key, ext, bytes).await?)),
                Err(e) => {
                    if e.is_cacheable()
                        || matches!(e, PreviewError::Unsupported | PreviewError::TooLarge)
                    {
                        cache.mark_failed(&key, &e.to_string()).await;
                    }
                    tracing::debug!(path = %input.path, error = %e, "preview generation failed");
                    Err(e)
                }
            }
        }
        .await;
        drop(_guard);
        self.inner
            .inflight
            .lock()
            .expect("inflight lock")
            .remove(&key);
        result
    }

    async fn generate(
        &self,
        kind: FileKind,
        variant: Variant,
        input: &PreviewInput,
        office_pdf: Option<&Path>,
    ) -> Result<(&'static str, Vec<u8>)> {
        let cfg = &self.inner.cfg;
        let tools = &self.inner.tools;
        let max = if variant == Variant::Large {
            cfg.large_size
        } else {
            cfg.thumb_size
        };
        let local = input.local.as_path();

        if variant == Variant::Pdf {
            if input.size > cfg.max_office_bytes {
                return Err(PreviewError::TooLarge);
            }
            let _permit = self.inner.office.acquire().await.expect("semaphore open");
            let pdf =
                document::office_to_pdf(tools, local, &cfg.temp_dir, cfg.office_timeout).await?;
            return Ok(("pdf", pdf));
        }

        let _permit = self.inner.jobs.acquire().await.expect("semaphore open");
        let encoded: Encoded = match kind {
            FileKind::Image | FileKind::Raw => {
                let limit = if kind == FileKind::Raw {
                    cfg.max_raw_bytes
                } else {
                    cfg.max_image_bytes
                };
                if input.size > limit {
                    return Err(PreviewError::TooLarge);
                }
                let bytes = tokio::fs::read(local).await?;
                blocking(move || {
                    let img = if kind == FileKind::Raw {
                        raster::decode_raw(&bytes)?
                    } else {
                        raster::decode(&bytes)?
                    };
                    raster::resize_and_encode(&img, max)
                })
                .await?
            }
            FileKind::Heif => {
                let png = media::heif_to_png(tools, local, &cfg.temp_dir, cfg.tool_timeout).await?;
                blocking(move || raster::resize_and_encode(&raster::decode(&png)?, max)).await?
            }
            FileKind::Video => {
                let info = match media::probe(tools, local, cfg.tool_timeout).await {
                    Ok(i) => Some(i),
                    Err(PreviewError::ToolMissing(_)) => None,
                    Err(e) => return Err(e),
                };
                let at = media::poster_time(info.and_then(|i| i.duration));
                let png = media::video_frame(tools, local, at, max, cfg.tool_timeout).await?;
                blocking(move || raster::resize_and_encode(&raster::decode(&png)?, max)).await?
            }
            FileKind::Audio => {
                let info = media::probe(tools, local, cfg.tool_timeout).await?;
                if !info.has_cover {
                    return Err(PreviewError::Unsupported);
                }
                let png = media::cover_art(tools, local, cfg.tool_timeout).await?;
                blocking(move || raster::resize_and_encode(&raster::decode(&png)?, max)).await?
            }
            FileKind::Pdf | FileKind::Office => {
                let pdf = office_pdf.unwrap_or(local);
                let png =
                    document::pdf_first_page(tools, pdf, max, &cfg.temp_dir, cfg.tool_timeout)
                        .await?;
                blocking(move || raster::encode(&raster::decode(&png)?)).await?
            }
        };
        Ok((encoded.extension(), encoded.bytes))
    }

    /// Media metadata (duration, codecs, tags) for the player UI.
    pub async fn probe(&self, input: &PreviewInput) -> Result<MediaInfo> {
        let _permit = self.inner.jobs.acquire().await.expect("semaphore open");
        media::probe(&self.inner.tools, &input.local, self.inner.cfg.tool_timeout).await
    }

    /// Start a browser-playable stream of `input` from `start` seconds. Used
    /// for files [`MediaInfo::browser_playable`] says the browser cannot play.
    pub async fn transcode(
        &self,
        input: &PreviewInput,
        start: f64,
        video: bool,
    ) -> Result<TranscodeStream> {
        let info = self.probe(input).await?;
        let target = if video && info.video_codec.is_some() {
            TranscodeTarget::Video { max_height: 720 }
        } else {
            TranscodeTarget::Audio
        };
        if target == TranscodeTarget::Audio && info.audio_codec.is_none() {
            return Err(PreviewError::Unsupported);
        }
        media::transcode(&self.inner.tools, &input.local, start, &info, target)
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| PreviewError::Io(std::io::Error::other(e)))?
}

async fn read_head(path: &Path) -> Option<Vec<u8>> {
    let mut f = tokio::fs::File::open(path).await.ok()?;
    let mut buf = vec![0u8; 32];
    let n = f.read(&mut buf).await.ok()?;
    buf.truncate(n);
    Some(buf)
}

fn missing_tool(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Heif => "heif-dec or ffmpeg",
        FileKind::Video | FileKind::Audio => "ffmpeg",
        FileKind::Pdf => "pdftoppm",
        FileKind::Office => "soffice and pdftoppm",
        FileKind::Image | FileKind::Raw => "nothing",
    }
}
