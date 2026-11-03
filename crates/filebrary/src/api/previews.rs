//! Thumbnails, previews, media info and transcoded playback.
//!
//! | Route | Returns |
//! |-------|---------|
//! | `GET /api/previews` | which file kinds can be previewed on this server |
//! | `GET /api/sources/{id}/preview?path=&variant=thumb\|large\|pdf` | an image (or PDF) |
//! | `GET /api/sources/{id}/media?path=` | duration, codecs, tags, `browser_playable` |
//! | `GET /api/sources/{id}/stream?path=&start=&audio=` | fragmented MP4 for files the browser cannot play |
//!
//! All of them need the `download` permission on the file. When a file has
//! no preview the answer is `404` with `{"error": "no_preview", ...}` so the
//! UI can show an icon instead.
//!
//! Files the browser already understands (JPEG, PNG, PDF, ...) answer the
//! `large` and `pdf` variants with the original bytes. Seeking in natively
//! playable video goes through the download route's range support; for the
//! `stream` route the client passes `start` (seconds) and uses the duration
//! from `media` for its seek bar.

use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::SystemTime;

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use filebrary_preview::{
    FileKind, PreviewError, PreviewInput, PreviewOutput, PreviewService, TranscodeStream, Variant,
};
use futures::TryStreamExt;
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf};
use tokio_util::io::ReaderStream;

use crate::{
    auth::{AuthUser, Perms},
    error::{AppError, AppResult},
    state::AppState,
    storage::{Source, VPath, guess_mime},
};

/// Routes for this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/previews", get(capabilities))
        .route("/sources/{source}/preview", get(preview))
        .route("/sources/{source}/media", get(media))
        .route("/sources/{source}/stream", get(stream))
}

/// `?path=&variant=`
#[derive(Deserialize)]
pub struct PreviewQuery {
    /// File path inside the source, as the user sees it.
    pub path: String,
    /// `thumb` (default), `large` or `pdf`.
    pub variant: Option<String>,
}

/// `?path=&start=&audio=`
#[derive(Deserialize)]
pub struct StreamQuery {
    /// File path inside the source, as the user sees it.
    pub path: String,
    /// Start position in seconds.
    #[serde(default)]
    pub start: f64,
    /// Drop the picture and send audio only.
    #[serde(default)]
    pub audio: bool,
}

/// `GET /api/previews`: preview support on this server, so the UI only asks
/// for thumbnails that can exist.
async fn capabilities(State(state): State<AppState>, _user: AuthUser) -> Json<serde_json::Value> {
    let Some(svc) = &state.previews else {
        return Json(json!({ "enabled": false }));
    };
    let cfg = svc.config();
    let kinds = [
        ("image", FileKind::Image),
        ("raw", FileKind::Raw),
        ("heif", FileKind::Heif),
        ("video", FileKind::Video),
        ("audio", FileKind::Audio),
        ("pdf", FileKind::Pdf),
        ("office", FileKind::Office),
    ];
    let kinds: serde_json::Map<_, _> = kinds
        .iter()
        .map(|(n, k)| ((*n).to_owned(), json!(svc.supports(*k))))
        .collect();
    Json(json!({
        "enabled": true,
        "thumb_size": cfg.thumb_size,
        "large_size": cfg.large_size,
        "kinds": kinds,
        "transcode": svc.tools().ffmpeg.is_some(),
    }))
}

async fn preview(
    State(state): State<AppState>,
    AuthUser { user, .. }: AuthUser,
    Path(source): Path<String>,
    Query(q): Query<PreviewQuery>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        let vp = VPath::parse(&q.path)?;
        let real = user.require(&source, Perms::DOWNLOAD, &vp)?;
        let variant = match q.variant.as_deref() {
            None => Variant::Thumb,
            Some(v) => Variant::parse(v).ok_or_else(|| AppError::bad_request("unknown variant"))?,
        };
        Ok::<_, AppError>((real, variant))
    }
    .await;
    match result {
        Ok((real, variant)) => preview_response(&state, &source, &real, variant, &headers).await,
        Err(e) => e.into_response(),
    }
}

/// Answer a preview request for `real` (a path already checked against the
/// caller's permissions). Public so share links can serve previews too.
pub async fn preview_response(
    state: &AppState,
    source_name: &str,
    real: &VPath,
    variant: Variant,
    headers: &HeaderMap,
) -> Response {
    let Some(svc) = &state.previews else {
        return no_preview("previews are turned off");
    };
    let Some(source) = state.source(source_name) else {
        return AppError::NotFound.into_response();
    };
    let local = match Local::open(state, source, real).await {
        Ok(l) => l,
        Err(e) => return e.into_response(),
    };
    match svc.get(&local.input, variant).await {
        Ok(PreviewOutput::Cached(c)) => serve_cached(c, headers).await,
        Ok(PreviewOutput::Original) => serve_original(source, real).await,
        Err(e) => preview_error(e),
    }
}

async fn media(
    State(state): State<AppState>,
    AuthUser { user, .. }: AuthUser,
    Path(source): Path<String>,
    Query(q): Query<PreviewQuery>,
) -> Response {
    let r = async {
        let vp = VPath::parse(&q.path)?;
        let real = user.require(&source, Perms::DOWNLOAD, &vp)?;
        let svc = state.previews.as_ref().ok_or(AppError::NotFound)?;
        let src = state.source(&source).ok_or(AppError::NotFound)?;
        let local = Local::open(&state, src, &real).await?;
        Ok::<_, AppError>(svc.probe(&local.input).await)
    }
    .await;
    match r {
        Ok(Ok(info)) => {
            let playable = info.browser_playable();
            let mut v = serde_json::to_value(info).unwrap_or_default();
            v["browser_playable"] = json!(playable);
            Json(v).into_response()
        }
        Ok(Err(e)) => preview_error(e),
        Err(e) => e.into_response(),
    }
}

async fn stream(
    State(state): State<AppState>,
    AuthUser { user, .. }: AuthUser,
    Path(source): Path<String>,
    Query(q): Query<StreamQuery>,
) -> Response {
    if !q.start.is_finite() || q.start < 0.0 {
        return AppError::bad_request("start must be a positive number of seconds").into_response();
    }
    let r = async {
        let vp = VPath::parse(&q.path)?;
        let real = user.require(&source, Perms::DOWNLOAD, &vp)?;
        let svc: &PreviewService = state.previews.as_ref().ok_or(AppError::NotFound)?;
        let src = state.source(&source).ok_or(AppError::NotFound)?;
        let local = Local::open(&state, src, &real).await?;
        Ok::<_, AppError>((svc.transcode(&local.input, q.start, !q.audio).await, local))
    }
    .await;
    match r {
        Ok((Ok(stream), local)) => {
            let ct = if q.audio { "audio/mp4" } else { "video/mp4" };
            let body = Body::from_stream(ReaderStream::new(Transcoding {
                stream,
                _local: local,
            }));
            (
                [
                    (header::CONTENT_TYPE, HeaderValue::from_static(ct)),
                    (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
                ],
                body,
            )
                .into_response()
        }
        Ok((Err(e), _)) => preview_error(e),
        Err(e) => e.into_response(),
    }
}

/// A file made available on the local disk for the preview tools: the real
/// path for local sources, or a temporary copy for remote ones (deleted on
/// drop).
struct Local {
    input: PreviewInput,
    _temp: Option<tempfile::TempPath>,
}

impl Local {
    async fn open(state: &AppState, source: &Source, real: &VPath) -> AppResult<Local> {
        let entry = source.storage.stat(real).await?;
        if entry.is_dir() {
            return Err(AppError::bad_request("folders have no preview"));
        }
        let modified = entry.modified.map(SystemTime::from);
        let mut input = PreviewInput {
            source: source.name.clone(),
            path: real.to_string(),
            local: PathBuf::new(),
            size: entry.size,
            modified,
        };
        if let Some(p) = source.storage.local_path(real).await? {
            input.local = p;
            return Ok(Local { input, _temp: None });
        }
        // Remote source: copy it down once, within a size limit.
        if entry.size > state.config.previews.max_remote_mib.saturating_mul(1 << 20) {
            return Err(AppError::PayloadTooLarge);
        }
        let dir = state.config.data_dir.join("cache").join("spool");
        tokio::fs::create_dir_all(&dir).await?;
        // Keep the extension: tools such as LibreOffice pick a filter by it.
        let suffix = real
            .name()
            .and_then(|n| std::path::Path::new(n).extension())
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let tmp = tempfile::Builder::new().suffix(&suffix).tempfile_in(&dir)?;
        let (file, path) = tmp.into_parts();
        let mut file = tokio::fs::File::from_std(file);
        let mut data = source.storage.read(real, None).await?;
        while let Some(chunk) = data.try_next().await? {
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        input.local = path.to_path_buf();
        Ok(Local {
            input,
            _temp: Some(path),
        })
    }
}

/// Keeps a remote file's temporary copy alive for as long as ffmpeg reads it.
struct Transcoding {
    stream: TranscodeStream,
    _local: Local,
}

impl AsyncRead for Transcoding {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

async fn serve_cached(c: filebrary_preview::CachedFile, headers: &HeaderMap) -> Response {
    let etag = format!(
        "\"{}\"",
        c.path
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default()
    );
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag))
    {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    let file = match tokio::fs::File::open(&c.path).await {
        Ok(f) => f,
        // Swept between lookup and open: let the client simply retry.
        Err(e) => return AppError::from(e).into_response(),
    };
    let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    (
        [
            (header::CONTENT_TYPE, c.content_type.to_owned()),
            (header::CONTENT_LENGTH, len.to_string()),
            (header::CACHE_CONTROL, "private, max-age=604800".to_owned()),
            (header::ETAG, etag),
        ],
        Body::from_stream(ReaderStream::new(file)),
    )
        .into_response()
}

async fn serve_original(source: &Source, real: &VPath) -> Response {
    let name = real.name().unwrap_or_default();
    let mime = guess_mime(name);
    match source.storage.read(real, None).await {
        Ok(stream) => {
            let mut resp = (
                [
                    (header::CONTENT_TYPE, mime.clone()),
                    (header::CACHE_CONTROL, "private, max-age=3600".to_owned()),
                ],
                Body::from_stream(stream),
            )
                .into_response();
            // User files never run script in our origin. Chrome's PDF viewer
            // does not render under a sandbox CSP, so PDFs are left out; the
            // global CSP still applies to them.
            if mime != "application/pdf" {
                resp.headers_mut().insert(
                    header::CONTENT_SECURITY_POLICY,
                    HeaderValue::from_static("sandbox; default-src 'none'; img-src 'self' data:"),
                );
            }
            resp
        }
        Err(e) => AppError::from(e).into_response(),
    }
}

fn no_preview(message: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "no_preview", "message": message })),
    )
        .into_response()
}

fn preview_error(e: PreviewError) -> Response {
    match e {
        PreviewError::Io(io) => AppError::from(io).into_response(),
        PreviewError::Tool { .. } | PreviewError::Timeout(_) => {
            tracing::warn!(error = %e, "preview tool failed");
            no_preview(&e.to_string())
        }
        other => no_preview(&other.to_string()),
    }
}
