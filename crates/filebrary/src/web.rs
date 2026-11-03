//! The embedded web UI (built from `web/` into `web/dist`).
//!
//! Hashed assets under `/assets/` are cached for a year; every other path that
//! is not a file falls back to `index.html` so client-side routes work.

use axum::{
    body::Body,
    http::{HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

/// Files from `web/dist`, compiled into the binary in release builds.
#[derive(RustEmbed)]
#[folder = "../../web/dist/"]
#[allow_missing = true]
struct Assets;

/// Serves an embedded file, or `index.html` for unknown non-API paths.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Some(resp) = file(path) {
        return resp;
    }
    file("index.html").unwrap_or_else(|| {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "Filebrary is running, but the web UI was not built into this binary.\n\
             Build it with `npm --prefix web ci && npm --prefix web run build`, then rebuild.\n",
        )
            .into_response()
    })
}

fn file(path: &str) -> Option<Response> {
    let asset = Assets::get(if path.is_empty() { "index.html" } else { path })?;
    let mime = asset.metadata.mimetype().to_owned();
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let mut resp = Response::new(Body::from(asset.data.into_owned()));
    let h = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&mime) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    Some(resp)
}
