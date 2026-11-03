//! Casting video and audio to Chromecast (and AirPlay) devices.
//!
//! A cast device fetches the media itself, without the browser's session
//! cookie. So before casting, the web UI asks for a short-lived *cast URL*:
//!
//! * `POST /api/cast` with `{source, path}` returns `{url, expires_at}`. The
//!   caller needs `download` on the file, and only audio, video and images
//!   can be cast.
//! * `GET /api/public/cast/{token}/{name}` serves the file with byte ranges
//!   (devices seek with `Range`). The token is an HMAC of the user, file and
//!   expiry under the server secret, so nothing is stored. Each request
//!   reloads the user and re-checks the permission, so disabling the account
//!   or revoking access stops a cast at the next request.
//!
//! The web UI plays the media from this URL and opens the device picker with
//! the browser's Remote Playback API (`media.remote.prompt()`), which Chrome
//! routes to Chromecast and Safari to AirPlay. No third-party script is
//! loaded, so the content security policy stays `'self'`.

use std::ops::Range;

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

use crate::api::files::scoped;
use crate::{
    auth::{AuthUser, Perms, load_user},
    crypto, db,
    error::{AppError, AppResult},
    state::AppState,
    storage::{VPath, guess_mime},
};

/// How long a cast URL stays valid, in seconds (long enough for a film).
pub const CAST_TTL_SECS: i64 = 6 * 3600;

/// `POST /api/cast`, merged into `/api`.
pub fn router() -> Router<AppState> {
    Router::new().route("/cast", post(create))
}

/// `GET /api/public/cast/{token}/{name}`, nested at `/api/public/cast`.
pub fn public_router() -> Router<AppState> {
    Router::new().route("/{token}/{name}", get(serve))
}

/// Body of `POST /api/cast`.
#[derive(Debug, Deserialize)]
pub struct CastRequest {
    /// Source name.
    pub source: String,
    /// Path as the caller sees it.
    pub path: String,
}

/// Response of `POST /api/cast`.
#[derive(Debug, Serialize)]
pub struct CastUrl {
    /// Path (relative to the server) the device can fetch without a cookie.
    pub url: String,
    /// Absolute URL when `public_url` is configured.
    pub absolute_url: Option<String>,
    /// Unix seconds.
    pub expires_at: i64,
}

/// What a cast token grants.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Claims {
    /// User id.
    u: i64,
    /// Source.
    s: String,
    /// Real path.
    p: String,
    /// Expiry (Unix seconds).
    e: i64,
}

fn key(state: &AppState) -> Vec<u8> {
    // Domain-separated so a cast token can never be confused with another MAC.
    crypto::sha256(&[&state.secret[..], b"filebrary-cast-v1"].concat())
}

fn sign(key: &[u8], c: &Claims) -> String {
    let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(c).expect("claims serialise"));
    let mac = crypto::hmac_b64(key, body.as_bytes());
    format!("{body}.{mac}")
}

fn verify(key: &[u8], token: &str, now: i64) -> Option<Claims> {
    let (body, mac) = token.split_once('.')?;
    if !crypto::ct_eq(
        mac.as_bytes(),
        crypto::hmac_b64(key, body.as_bytes()).as_bytes(),
    ) {
        return None;
    }
    let c: Claims = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(body).ok()?).ok()?;
    (c.e > now).then_some(c)
}

fn castable(mime: &str) -> bool {
    ["video/", "audio/", "image/"]
        .iter()
        .any(|p| mime.starts_with(p))
}

/// `POST /api/cast`.
async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CastRequest>,
) -> AppResult<Json<CastUrl>> {
    let visible = VPath::parse(&req.path)?;
    let real = auth.user.require(&req.source, Perms::DOWNLOAD, &visible)?;
    let entry = scoped(&state, &auth.user, &req.source, Perms::DOWNLOAD, &visible)
        .await?
        .stat(&visible)
        .await?;
    if entry.is_dir() {
        return Err(AppError::bad_request("folders cannot be cast"));
    }
    if !castable(&guess_mime(&entry.name)) {
        return Err(AppError::bad_request(
            "only video, audio and images can be cast",
        ));
    }
    let expires_at = db::now() + CAST_TTL_SECS;
    let token = sign(
        &key(&state),
        &Claims {
            u: auth.user.id,
            s: req.source,
            p: real.to_string(),
            e: expires_at,
        },
    );
    let name: String =
        percent_encoding::utf8_percent_encode(&entry.name, percent_encoding::NON_ALPHANUMERIC)
            .collect();
    let url = format!("/api/public/cast/{token}/{name}");
    let absolute_url = state
        .config
        .public_url
        .as_deref()
        .map(|b| format!("{}{url}", b.trim_end_matches('/')));
    Ok(Json(CastUrl {
        url,
        absolute_url,
        expires_at,
    }))
}

/// Parses a single-range `Range` header against a file of `size` bytes.
///
/// Returns `Ok(None)` for no (or an ignorable multi-range) header and
/// `Err` when the range cannot be satisfied.
fn parse_range(h: Option<&HeaderValue>, size: u64) -> AppResult<Option<Range<u64>>> {
    let Some(spec) = h
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().strip_prefix("bytes="))
    else {
        return Ok(None);
    };
    if spec.contains(',') {
        return Ok(None); // multi-range: serve the whole file
    }
    let bad = || AppError::RangeNotSatisfiable { size };
    let (a, b) = spec.split_once('-').ok_or_else(bad)?;
    let (a, b) = (a.trim(), b.trim());
    let range = if a.is_empty() {
        let n: u64 = b.parse().map_err(|_| bad())?;
        if n == 0 {
            return Err(bad());
        }
        size.saturating_sub(n)..size
    } else {
        let start: u64 = a.parse().map_err(|_| bad())?;
        let end = if b.is_empty() {
            size
        } else {
            b.parse::<u64>()
                .map_err(|_| bad())?
                .saturating_add(1)
                .min(size)
        };
        start..end
    };
    if range.start >= range.end || range.start >= size {
        return Err(bad());
    }
    Ok(Some(range))
}

/// `GET /api/public/cast/{token}/{name}`.
async fn serve(
    State(state): State<AppState>,
    Path((token, _name)): Path<(String, String)>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let c = verify(&key(&state), &token, db::now()).ok_or(AppError::NotFound)?;
    let user = load_user(&state, c.u).await?.ok_or(AppError::NotFound)?;
    let real = VPath::parse(&c.p).map_err(|_| AppError::NotFound)?;
    let visible = user.visible_path(&c.s, &real).ok_or(AppError::NotFound)?;
    let storage = scoped(&state, &user, &c.s, Perms::DOWNLOAD, &visible)
        .await
        .map_err(|_| AppError::NotFound)?;
    let entry = storage.stat(&visible).await?;
    if entry.is_dir() {
        return Err(AppError::NotFound);
    }
    let range = parse_range(headers.get(header::RANGE), entry.size)?;
    let stream = storage.read(&visible, range.clone()).await?;
    let mut resp = Response::new(Body::from_stream(stream));
    let h = resp.headers_mut();
    let mime = guess_mime(&entry.name);
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&mime)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    // Cast receivers run on another origin and may fetch with CORS.
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    h.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("Content-Length, Content-Range, Accept-Ranges"),
    );
    let len = match &range {
        Some(r) => {
            *resp.status_mut() = StatusCode::PARTIAL_CONTENT;
            let v = format!("bytes {}-{}/{}", r.start, r.end - 1, entry.size);
            resp.headers_mut().insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&v).map_err(anyhow::Error::from)?,
            );
            r.end - r.start
        }
        None => entry.size,
    };
    resp.headers_mut()
        .insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    Ok(resp.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hv(s: &str) -> HeaderValue {
        HeaderValue::from_str(s).unwrap()
    }

    #[test]
    fn tokens() {
        let k = b"key".to_vec();
        let c = Claims {
            u: 7,
            s: "media".into(),
            p: "/films/a.mp4".into(),
            e: 100,
        };
        let t = sign(&k, &c);
        assert_eq!(verify(&k, &t, 99), Some(c));
        assert_eq!(verify(&k, &t, 100), None, "expired");
        assert_eq!(verify(b"other", &t, 0), None, "wrong key");
        // Tampering with the claims breaks the MAC.
        let (body, mac) = t.split_once('.').unwrap();
        let forged = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&Claims {
                u: 1,
                s: "media".into(),
                p: "/films/a.mp4".into(),
                e: 100,
            })
            .unwrap(),
        );
        assert_ne!(forged, body);
        assert_eq!(verify(&k, &format!("{forged}.{mac}"), 0), None);
        assert_eq!(verify(&k, "garbage", 0), None);
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range(None, 10).unwrap(), None);
        assert_eq!(parse_range(Some(&hv("bytes=0-4")), 10).unwrap(), Some(0..5));
        assert_eq!(parse_range(Some(&hv("bytes=5-")), 10).unwrap(), Some(5..10));
        assert_eq!(parse_range(Some(&hv("bytes=-3")), 10).unwrap(), Some(7..10));
        assert_eq!(
            parse_range(Some(&hv("bytes=-30")), 10).unwrap(),
            Some(0..10)
        );
        assert_eq!(
            parse_range(Some(&hv("bytes=8-100")), 10).unwrap(),
            Some(8..10)
        );
        assert_eq!(parse_range(Some(&hv("bytes=0-1,4-5")), 10).unwrap(), None);
        for bad in ["bytes=10-", "bytes=5-2", "bytes=-0", "bytes=x-1", "bytes=4"] {
            assert!(parse_range(Some(&hv(bad)), 10).is_err(), "{bad}");
        }
    }

    #[test]
    fn castable_types() {
        assert!(castable("video/mp4"));
        assert!(castable("audio/flac"));
        assert!(castable("image/jpeg"));
        assert!(!castable("text/html"));
        assert!(!castable("application/pdf"));
    }
}
