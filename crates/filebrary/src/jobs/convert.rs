//! Conversion jobs. Each takes one file and writes a new file next to it (or
//! into `dest`), never overwriting anything.
//!
//! | Kind               | Tool                    | Parameters (besides `source`, `path`, `dest`) |
//! |--------------------|-------------------------|-----------------------------------------------|
//! | `convert.document` | LibreOffice (headless)  | none: always to PDF                           |
//! | `convert.video`    | ffmpeg                  | `format`, `mode`, `max_height`, `crf`, `audio_kbps` |
//! | `convert.image`    | built in (ffmpeg for HEIC and other formats the built-in decoder lacks) | `format`, `max_width`, `max_height`, `quality` |
//!
//! The user needs `download` on the input and `upload` on the destination.
//! The input is copied to the job's private folder under a fixed name
//! (`input.<ext>`), so the user's file name never reaches a command line.

use std::{path::Path, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    JobContext, JobError, JobHandler, JobResult, Prepared,
    process::{Cmd, run, which},
    stage,
};
use crate::{
    auth::{Perms, User},
    error::{AppError, AppResult},
    state::AppState,
    storage::{Storage, VPath},
};

/// Where a conversion reads and writes, shared by every converter.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Target {
    source: String,
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dest: Option<String>,
}

struct Resolved {
    /// Backend for reading the input (user-scoped).
    input_storage: Arc<dyn Storage>,
    /// Backend for writing the output (user-scoped).
    output_storage: Arc<dyn Storage>,
    input: VPath,
    dest: VPath,
    stem: String,
    ext: String,
    size: u64,
}

impl Target {
    /// Checks permissions and that the input is a file and `dest` a folder.
    async fn resolve(&self, state: &AppState, user: &User) -> AppResult<Resolved> {
        let (input_storage, input) =
            stage::resolve(state, user, &self.source, &self.path, Perms::DOWNLOAD).await?;
        let entry = input_storage.stat(&input).await?;
        if entry.is_dir() {
            return Err(AppError::bad_request("select a file, not a folder"));
        }
        let dest = match &self.dest {
            Some(d) => d.clone(),
            None => input.parent().unwrap_or_default().to_string(),
        };
        let (output_storage, dest) =
            stage::resolve(state, user, &self.source, &dest, Perms::UPLOAD).await?;
        if !output_storage.stat(&dest).await?.is_dir() {
            return Err(AppError::bad_request("the destination is not a folder"));
        }
        let name = input.name().unwrap_or("file").to_owned();
        let (stem, _) = stage::split_name(&name);
        Ok(Resolved {
            stem: stem.to_owned(),
            ext: stage::safe_ext(&name),
            size: entry.size,
            input_storage,
            output_storage,
            input,
            dest,
        })
    }
}

/// Downloads the input into the job folder as `input.<ext>`, reporting the
/// first `share` of progress.
async fn fetch_input(ctx: &JobContext, r: &Resolved, share: f64) -> JobResult<std::path::PathBuf> {
    let local = ctx.tmp.join(if r.ext.is_empty() {
        "input".to_owned()
    } else {
        format!("input.{}", r.ext)
    });
    let mut got = 0u64;
    let total = r.size.max(1);
    let mut last = std::time::Instant::now();
    let mut pending = None;
    stage::download(
        r.input_storage.as_ref(),
        &r.input,
        &local,
        &ctx.cancel,
        |n| {
            got += n;
            if last.elapsed() > Duration::from_millis(500) {
                last = std::time::Instant::now();
                pending = Some(got as f64 / total as f64 * share);
            }
        },
    )
    .await?;
    if let Some(p) = pending {
        ctx.progress(p, Some("copying input")).await;
    }
    Ok(local)
}

/// Uploads `out` as `<stem>.<ext>` in the destination and builds the result.
async fn store_output(
    ctx: &JobContext,
    t: &Target,
    r: &Resolved,
    out: &Path,
    ext: &str,
) -> JobResult<Value> {
    ctx.check_cancel()?;
    ctx.progress(0.97, Some("saving")).await;
    let user = ctx.user().await?;
    let writer = stage::Writer {
        state: &ctx.state,
        user: &user,
        source: &t.source,
    };
    let written = stage::upload_new(
        &writer,
        r.output_storage.as_ref(),
        &r.dest,
        &r.stem,
        ext,
        out,
    )
    .await?;
    ctx.record_write(
        &user,
        &t.source,
        &written,
        crate::activity::action::UPLOAD,
        "convert",
    )
    .await;
    let size = tokio::fs::metadata(out).await.map(|m| m.len()).unwrap_or(0);
    Ok(json!({
        "source": t.source,
        "path": written.to_string(),
        "size": size,
    }))
}

fn tool(state: &AppState, configured: &str, what: &str) -> Result<std::path::PathBuf, String> {
    let _ = state;
    which(configured).ok_or_else(|| {
        format!("{what} ({configured}) is not installed on this server; use the -full Docker image or install it")
    })
}

fn timeout(state: &AppState) -> Duration {
    Duration::from_secs(state.config.jobs.timeout_secs)
}

// ---------------------------------------------------------------------------
// Documents

/// Extensions LibreOffice is asked to convert.
pub const DOCUMENT_EXTENSIONS: &[&str] = &[
    "doc", "docx", "docm", "dot", "dotx", "odt", "ott", "rtf", "txt", "wpd", "pages", "xls",
    "xlsx", "xlsm", "ods", "ots", "csv", "numbers", "ppt", "pptx", "pps", "ppsx", "odp", "otp",
    "key", "odg", "vsd", "vsdx", "pub", "html", "htm", "xml", "epub",
];

/// Handler for `convert.document`: any office document to PDF.
pub struct DocumentToPdf;

#[async_trait]
impl JobHandler for DocumentToPdf {
    fn available(&self, state: &AppState) -> Result<(), String> {
        tool(state, &state.config.jobs.soffice, "LibreOffice").map(drop)
    }

    fn describe(&self) -> Value {
        json!({ "title": "Convert to PDF", "extensions": DOCUMENT_EXTENSIONS, "formats": ["pdf"] })
    }

    async fn prepare(&self, state: &AppState, user: &User, params: Value) -> AppResult<Prepared> {
        let t: Target = parse(params)?;
        let r = t.resolve(state, user).await?;
        if !DOCUMENT_EXTENSIONS.contains(&r.ext.as_str()) {
            return Err(AppError::bad_request(
                "this file type cannot be converted to PDF",
            ));
        }
        Ok(Prepared {
            title: format!("Convert {} to PDF", r.input.name().unwrap_or("file")),
            params: serde_json::to_value(&t).map_err(anyhow::Error::from)?,
        })
    }

    async fn run(&self, ctx: &JobContext, params: &Value) -> JobResult<Value> {
        let t: Target = parse(params.clone())?;
        let user = ctx.user().await?;
        let r = t.resolve(&ctx.state, &user).await?;
        let soffice = tool(&ctx.state, &ctx.state.config.jobs.soffice, "LibreOffice")
            .map_err(JobError::Failed)?;
        let input = fetch_input(ctx, &r, 0.1).await?;
        ctx.progress(0.1, Some("converting")).await;
        let out_dir = ctx.tmp.join("out");
        let profile = ctx.tmp.join("lo-profile");
        tokio::fs::create_dir_all(&out_dir).await?;
        tokio::fs::create_dir_all(&profile).await?;
        // A private profile per job lets conversions run in parallel.
        let profile_url = file_url(&profile);
        let cmd = Cmd::new(soffice, &ctx.tmp, timeout(&ctx.state))
            .arg(format!("-env:UserInstallation={profile_url}"))
            .args([
                "--headless",
                "--invisible",
                "--nodefault",
                "--nofirststartwizard",
                "--nolockcheck",
                "--nologo",
                "--norestore",
                "--convert-to",
                "pdf",
                "--outdir",
            ])
            .arg(&out_dir)
            .arg(&input);
        run(&cmd, &ctx.cancel, |_| {}).await?;
        let out = out_dir.join("input.pdf");
        if !tokio::fs::try_exists(&out).await.unwrap_or(false) {
            return Err(JobError::failed(
                "LibreOffice could not convert this document",
            ));
        }
        store_output(ctx, &t, &r, &out, "pdf").await
    }
}

/// A `file://` URL for a local directory, as LibreOffice expects.
fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let encoded: String = s
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}

// ---------------------------------------------------------------------------
// Video and audio

/// Output containers for `convert.video`.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VideoFormat {
    /// MP4 with H.264 and AAC (plays everywhere).
    Mp4,
    /// Matroska with H.264 and AAC.
    Mkv,
    /// WebM with VP9 and Opus.
    Webm,
    /// QuickTime with H.264 and AAC.
    Mov,
    /// Audio only, MP3.
    Mp3,
    /// Audio only, AAC in MP4.
    M4a,
    /// Audio only, Opus in Ogg.
    Opus,
}

impl VideoFormat {
    fn ext(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mkv => "mkv",
            Self::Webm => "webm",
            Self::Mov => "mov",
            Self::Mp3 => "mp3",
            Self::M4a => "m4a",
            Self::Opus => "opus",
        }
    }

    fn audio_only(self) -> bool {
        matches!(self, Self::Mp3 | Self::M4a | Self::Opus)
    }
}

/// Transcode (re-encode) or remux (copy streams into a new container).
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VideoMode {
    /// Re-encode audio and video.
    #[default]
    Transcode,
    /// Copy the streams unchanged into the new container (fast, lossless).
    Remux,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct VideoParams {
    source: String,
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dest: Option<String>,
    format: VideoFormat,
    #[serde(default)]
    mode: VideoMode,
    /// Scale down to at most this many lines (never up).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_height: Option<u32>,
    /// Quality: 0 (best) to 51 (worst). Default 23 (H.264) / 32 (VP9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    crf: Option<u8>,
    /// Audio bitrate in kbit/s. Default 160.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    audio_kbps: Option<u32>,
}

impl VideoParams {
    fn target(&self) -> Target {
        Target {
            source: self.source.clone(),
            path: self.path.clone(),
            dest: self.dest.clone(),
        }
    }

    fn validate(&self) -> AppResult<()> {
        if self.mode == VideoMode::Remux {
            if self.format.audio_only() {
                return Err(AppError::bad_request(
                    "remux needs a video format; use transcode for audio",
                ));
            }
            if self.max_height.is_some() || self.crf.is_some() || self.audio_kbps.is_some() {
                return Err(AppError::bad_request(
                    "remux copies streams unchanged; max_height, crf and audio_kbps need transcode",
                ));
            }
        }
        if let Some(h) = self.max_height
            && !(16..=8640).contains(&h)
        {
            return Err(AppError::bad_request(
                "max_height must be between 16 and 8640",
            ));
        }
        if self.crf.is_some_and(|c| c > 63) {
            return Err(AppError::bad_request("crf must be between 0 and 63"));
        }
        if let Some(k) = self.audio_kbps
            && !(16..=512).contains(&k)
        {
            return Err(AppError::bad_request(
                "audio_kbps must be between 16 and 512",
            ));
        }
        Ok(())
    }

    /// The ffmpeg arguments between the input and the output file.
    fn codec_args(&self) -> Vec<String> {
        let mut a: Vec<String> = Vec::new();
        let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        let kbps = format!("{}k", self.audio_kbps.unwrap_or(160));
        if self.mode == VideoMode::Remux {
            a.extend(match self.format {
                VideoFormat::Mkv => s(&["-map", "0", "-c", "copy"]),
                _ => s(&["-map", "0:v", "-map", "0:a?", "-c", "copy"]),
            });
        } else if self.format.audio_only() {
            a.extend(s(&["-map", "0:a:0", "-vn"]));
            a.extend(match self.format {
                VideoFormat::Mp3 => s(&["-c:a", "libmp3lame"]),
                VideoFormat::Opus => s(&["-c:a", "libopus"]),
                _ => s(&["-c:a", "aac"]),
            });
            a.extend(["-b:a".to_owned(), kbps]);
        } else {
            a.extend(s(&["-map", "0:v:0", "-map", "0:a?"]));
            if let Some(h) = self.max_height {
                // Never upscale; keep the width even for the encoders.
                a.extend(["-vf".to_owned(), format!("scale=-2:min(ih\\,{h})")]);
            }
            if self.format == VideoFormat::Webm {
                let crf = self.crf.unwrap_or(32).to_string();
                a.extend(s(&[
                    "-c:v",
                    "libvpx-vp9",
                    "-b:v",
                    "0",
                    "-row-mt",
                    "1",
                    "-deadline",
                    "good",
                    "-cpu-used",
                    "4",
                    "-crf",
                ]));
                a.push(crf);
                a.extend(s(&["-c:a", "libopus"]));
            } else {
                let crf = self.crf.unwrap_or(23).min(51).to_string();
                a.extend(s(&[
                    "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", "-crf",
                ]));
                a.push(crf);
                a.extend(s(&["-c:a", "aac"]));
            }
            a.extend(["-b:a".to_owned(), kbps]);
        }
        if matches!(
            self.format,
            VideoFormat::Mp4 | VideoFormat::Mov | VideoFormat::M4a
        ) {
            a.extend(s(&["-movflags", "+faststart"]));
        }
        a
    }
}

/// Handler for `convert.video`: transcode or remux video, or extract audio.
pub struct Video;

#[async_trait]
impl JobHandler for Video {
    fn available(&self, state: &AppState) -> Result<(), String> {
        tool(state, &state.config.jobs.ffmpeg, "ffmpeg").map(drop)
    }

    fn describe(&self) -> Value {
        json!({
            "title": "Convert video or audio",
            "formats": ["mp4", "mkv", "webm", "mov", "mp3", "m4a", "opus"],
            "modes": ["transcode", "remux"],
        })
    }

    async fn prepare(&self, state: &AppState, user: &User, params: Value) -> AppResult<Prepared> {
        let p: VideoParams = parse(params)?;
        p.validate()?;
        let r = p.target().resolve(state, user).await?;
        let verb = if p.mode == VideoMode::Remux {
            "Remux"
        } else {
            "Convert"
        };
        Ok(Prepared {
            title: format!(
                "{verb} {} to {}",
                r.input.name().unwrap_or("file"),
                p.format.ext().to_uppercase()
            ),
            params: serde_json::to_value(&p).map_err(anyhow::Error::from)?,
        })
    }

    async fn run(&self, ctx: &JobContext, params: &Value) -> JobResult<Value> {
        let p: VideoParams = parse(params.clone())?;
        p.validate()?;
        let user = ctx.user().await?;
        let r = p.target().resolve(&ctx.state, &user).await?;
        let jobs = &ctx.state.config.jobs;
        let ffmpeg = tool(&ctx.state, &jobs.ffmpeg, "ffmpeg").map_err(JobError::Failed)?;
        let input = fetch_input(ctx, &r, 0.05).await?;

        let duration_us = match which(&jobs.ffprobe) {
            Some(probe) => probe_duration_us(ctx, &probe, &input).await,
            None => None,
        };
        ctx.progress(0.05, Some("converting")).await;

        let out = ctx.tmp.join(format!("output.{}", p.format.ext()));
        let cmd = Cmd::new(ffmpeg, &ctx.tmp, timeout(&ctx.state))
            .args([
                "-hide_banner",
                "-nostdin",
                "-nostats",
                "-loglevel",
                "error",
                "-y",
                "-i",
            ])
            .arg(&input)
            .args(p.codec_args())
            .args(["-progress", "pipe:1"])
            .arg(&out);
        let (tx, mut rx) = tokio::sync::watch::channel(0.0f64);
        let runner = run(&cmd, &ctx.cancel, move |line| {
            if let (Some(total), Some(us)) = (duration_us, parse_out_time_us(line))
                && total > 0
            {
                let _ = tx.send(us as f64 / total as f64);
            }
        });
        tokio::pin!(runner);
        loop {
            tokio::select! {
                r = &mut runner => { r?; break; }
                Ok(()) = rx.changed() => {
                    let f = *rx.borrow_and_update();
                    ctx.progress(0.05 + f.clamp(0.0, 1.0) * 0.9, Some("converting")).await;
                }
            }
        }
        store_output(ctx, &p.target(), &r, &out, p.format.ext()).await
    }
}

async fn probe_duration_us(ctx: &JobContext, ffprobe: &Path, input: &Path) -> Option<u64> {
    let cmd = Cmd::new(ffprobe, &ctx.tmp, Duration::from_secs(60))
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(input);
    let mut secs = None;
    run(&cmd, &ctx.cancel, |l| {
        if let Ok(v) = l.trim().parse::<f64>() {
            secs = Some(v);
        }
    })
    .await
    .ok()?;
    secs.filter(|s| s.is_finite() && *s > 0.0)
        .map(|s| (s * 1e6) as u64)
}

/// Reads `out_time_us=` (or the misnamed `out_time_ms=`, also microseconds)
/// from an ffmpeg `-progress` line.
fn parse_out_time_us(line: &str) -> Option<u64> {
    let v = line
        .strip_prefix("out_time_us=")
        .or_else(|| line.strip_prefix("out_time_ms="))?;
    v.trim().parse().ok()
}

// ---------------------------------------------------------------------------
// Images

/// Output formats for `convert.image`.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    /// JPEG (lossy; `quality` applies).
    #[serde(alias = "jpg")]
    Jpeg,
    /// PNG (lossless).
    Png,
    /// WebP (lossless).
    Webp,
    /// GIF.
    Gif,
    /// TIFF.
    Tiff,
    /// BMP.
    Bmp,
}

impl ImageFormat {
    fn ext(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Gif => "gif",
            Self::Tiff => "tiff",
            Self::Bmp => "bmp",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ImageParams {
    source: String,
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dest: Option<String>,
    format: ImageFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_height: Option<u32>,
    /// JPEG quality, 1–100 (default 85).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quality: Option<u8>,
}

/// Largest image width or height accepted, in pixels.
const MAX_IMAGE_SIDE: u32 = 30_000;

impl ImageParams {
    fn target(&self) -> Target {
        Target {
            source: self.source.clone(),
            path: self.path.clone(),
            dest: self.dest.clone(),
        }
    }

    fn validate(&self) -> AppResult<()> {
        for v in [self.max_width, self.max_height].into_iter().flatten() {
            if v == 0 || v > MAX_IMAGE_SIDE {
                return Err(AppError::bad_request(format!(
                    "max_width and max_height must be between 1 and {MAX_IMAGE_SIDE}"
                )));
            }
        }
        if self.quality.is_some_and(|q| q == 0 || q > 100) {
            return Err(AppError::bad_request("quality must be between 1 and 100"));
        }
        Ok(())
    }
}

/// Handler for `convert.image`: resize and change format.
pub struct Image;

#[async_trait]
impl JobHandler for Image {
    fn describe(&self) -> Value {
        json!({ "title": "Resize or convert image", "formats": ["jpeg", "png", "webp", "gif", "tiff", "bmp"] })
    }

    async fn prepare(&self, state: &AppState, user: &User, params: Value) -> AppResult<Prepared> {
        let p: ImageParams = parse(params)?;
        p.validate()?;
        let r = p.target().resolve(state, user).await?;
        let verb = if p.max_width.is_some() || p.max_height.is_some() {
            "Resize"
        } else {
            "Convert"
        };
        Ok(Prepared {
            title: format!(
                "{verb} {} to {}",
                r.input.name().unwrap_or("image"),
                p.format.ext().to_uppercase()
            ),
            params: serde_json::to_value(&p).map_err(anyhow::Error::from)?,
        })
    }

    async fn run(&self, ctx: &JobContext, params: &Value) -> JobResult<Value> {
        let p: ImageParams = parse(params.clone())?;
        p.validate()?;
        let user = ctx.user().await?;
        let r = p.target().resolve(&ctx.state, &user).await?;
        let input = fetch_input(ctx, &r, 0.2).await?;
        ctx.progress(0.2, Some("decoding")).await;

        let decoded = {
            let input = input.clone();
            tokio::task::spawn_blocking(move || decode_image(&input))
                .await
                .map_err(|e| JobError::failed(format!("image task failed: {e}")))?
        };
        let img = match decoded {
            Ok(img) => img,
            Err(first) => {
                // HEIC, RAW-ish and other formats: let ffmpeg decode to PNG.
                let Some(ffmpeg) = which(&ctx.state.config.jobs.ffmpeg) else {
                    return Err(first);
                };
                let png = ctx.tmp.join("decoded.png");
                let cmd = Cmd::new(ffmpeg, &ctx.tmp, timeout(&ctx.state))
                    .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"])
                    .arg(&input)
                    .args(["-frames:v", "1"])
                    .arg(&png);
                if run(&cmd, &ctx.cancel, |_| {}).await.is_err() {
                    return Err(first);
                }
                tokio::task::spawn_blocking(move || decode_image(&png))
                    .await
                    .map_err(|e| JobError::failed(format!("image task failed: {e}")))??
            }
        };
        ctx.check_cancel()?;
        ctx.progress(0.6, Some("encoding")).await;
        let out = ctx.tmp.join(format!("output.{}", p.format.ext()));
        {
            let (out, p2) = (out.clone(), p.clone());
            tokio::task::spawn_blocking(move || encode_image(img, &p2, &out))
                .await
                .map_err(|e| JobError::failed(format!("image task failed: {e}")))??;
        }
        store_output(ctx, &p.target(), &r, &out, p.format.ext()).await
    }
}

fn decode_image(path: &Path) -> JobResult<image::DynamicImage> {
    use image::ImageDecoder;
    let mut reader = image::ImageReader::open(path)?
        .with_guessed_format()
        .map_err(|e| JobError::failed(format!("cannot read image: {e}")))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    limits.max_alloc = Some(1024 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| JobError::failed(format!("unsupported image: {e}")))?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder)
        .map_err(|e| JobError::failed(format!("cannot decode image: {e}")))?;
    img.apply_orientation(orientation);
    Ok(img)
}

fn encode_image(img: image::DynamicImage, p: &ImageParams, out: &Path) -> JobResult<()> {
    let (w, h) = (img.width(), img.height());
    let (mw, mh) = (p.max_width.unwrap_or(w), p.max_height.unwrap_or(h));
    let img = if w > mw || h > mh {
        img.resize(mw, mh, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let file = std::io::BufWriter::new(std::fs::File::create(out)?);
    let err = |e: image::ImageError| JobError::failed(format!("cannot encode image: {e}"));
    match p.format {
        ImageFormat::Jpeg => {
            // JPEG has no alpha channel.
            let rgb = image::DynamicImage::ImageRgb8(img.to_rgb8());
            let enc =
                image::codecs::jpeg::JpegEncoder::new_with_quality(file, p.quality.unwrap_or(85));
            rgb.write_with_encoder(enc).map_err(err)?;
        }
        ImageFormat::Png => img
            .write_with_encoder(image::codecs::png::PngEncoder::new(file))
            .map_err(err)?,
        ImageFormat::Webp => {
            let rgba = image::DynamicImage::ImageRgba8(img.to_rgba8());
            rgba.write_with_encoder(image::codecs::webp::WebPEncoder::new_lossless(file))
                .map_err(err)?;
        }
        ImageFormat::Gif => {
            let rgba = image::DynamicImage::ImageRgba8(img.to_rgba8());
            rgba.write_with_encoder(image::codecs::gif::GifEncoder::new(file))
                .map_err(err)?;
        }
        ImageFormat::Tiff => {
            let mut buf = std::io::Cursor::new(Vec::new());
            img.write_to(&mut buf, image::ImageFormat::Tiff)
                .map_err(err)?;
            std::fs::write(out, buf.into_inner())?;
        }
        ImageFormat::Bmp => {
            let rgba = image::DynamicImage::ImageRgba8(img.to_rgba8());
            let mut f = file;
            rgba.write_with_encoder(image::codecs::bmp::BmpEncoder::new(&mut f))
                .map_err(err)?;
        }
    }
    Ok(())
}

fn parse<T: serde::de::DeserializeOwned>(v: Value) -> AppResult<T> {
    serde_json::from_value(v).map_err(|e| AppError::bad_request(format!("invalid parameters: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(v: Value) -> VideoParams {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn video_arguments() {
        let p = video(json!({"source":"s","path":"/a.mkv","format":"mp4","max_height":720}));
        p.validate().unwrap();
        let a = p.codec_args().join(" ");
        assert!(
            a.contains("libx264")
                && a.contains("scale=-2:min(ih\\,720)")
                && a.contains("+faststart"),
            "{a}"
        );
        let p = video(json!({"source":"s","path":"/a.mp4","format":"mkv","mode":"remux"}));
        assert_eq!(p.codec_args().join(" "), "-map 0 -c copy");
        let p = video(json!({"source":"s","path":"/a.mp4","format":"mp3","audio_kbps":192}));
        assert!(p.codec_args().join(" ").contains("libmp3lame -b:a 192k"));
        let p = video(json!({"source":"s","path":"/a.mp4","format":"webm"}));
        assert!(p.codec_args().join(" ").contains("libvpx-vp9"));
    }

    #[test]
    fn video_validation() {
        assert!(
            video(json!({"source":"s","path":"/a","format":"mp3","mode":"remux"}))
                .validate()
                .is_err()
        );
        assert!(
            video(json!({"source":"s","path":"/a","format":"mp4","mode":"remux","crf":20}))
                .validate()
                .is_err()
        );
        assert!(
            video(json!({"source":"s","path":"/a","format":"mp4","max_height":5}))
                .validate()
                .is_err()
        );
        assert!(
            serde_json::from_value::<VideoParams>(
                json!({"source":"s","path":"/a","format":"mp4","shell":"x"})
            )
            .is_err()
        );
    }

    #[test]
    fn progress_lines() {
        assert_eq!(parse_out_time_us("out_time_us=1500000"), Some(1_500_000));
        assert_eq!(parse_out_time_us("out_time_ms=42"), Some(42));
        assert_eq!(parse_out_time_us("frame=10"), None);
        assert_eq!(parse_out_time_us("out_time_us=N/A"), None);
    }

    #[test]
    fn file_urls() {
        assert_eq!(file_url(Path::new("/tmp/a b")), "file:///tmp/a%20b");
    }

    #[test]
    fn image_roundtrip_resizes() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("in.png");
        image::RgbaImage::from_pixel(400, 200, image::Rgba([10, 20, 30, 255]))
            .save(&src)
            .unwrap();
        let img = decode_image(&src).unwrap();
        let p: ImageParams = serde_json::from_value(json!({
            "source":"s","path":"/in.png","format":"jpeg","max_width":100,"quality":70
        }))
        .unwrap();
        let out = dir.path().join("out.jpg");
        encode_image(img, &p, &out).unwrap();
        let back = image::open(&out).unwrap();
        assert_eq!((back.width(), back.height()), (100, 50));
        // Not decodable: a clear error, not a panic.
        std::fs::write(dir.path().join("bad.png"), b"not an image").unwrap();
        assert!(decode_image(&dir.path().join("bad.png")).is_err());
    }
}
