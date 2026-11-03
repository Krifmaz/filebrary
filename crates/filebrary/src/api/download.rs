//! Streaming file responses with HTTP range, `ETag` and safe content headers.
//!
//! Shared by the authenticated download route and public share links.

use std::ops::Range;

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

use crate::{
    error::{AppError, AppResult},
    storage::{Entry, Storage, VPath},
};

/// Content-Security-Policy applied to every served file, so an uploaded HTML
/// or SVG file viewed inline cannot run script in the application's origin.
pub const FILE_CSP: &str = "default-src 'none'; img-src 'self' data:; media-src 'self'; \
                            style-src 'unsafe-inline'; sandbox";

/// Characters left unescaped in RFC 5987 `filename*` values.
const ATTR_CHAR: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'!')
    .remove(b'#')
    .remove(b'$')
    .remove(b'&')
    .remove(b'+')
    .remove(b'-')
    .remove(b'.')
    .remove(b'^')
    .remove(b'_')
    .remove(b'`')
    .remove(b'|')
    .remove(b'~');

/// Outcome of parsing a `Range` header against a known size.
#[derive(Debug, PartialEq, Eq)]
pub enum RangeRequest {
    /// No (usable) range: send the whole file.
    Full,
    /// One satisfiable range.
    Partial(Range<u64>),
    /// A syntactically valid but unsatisfiable range.
    Unsatisfiable,
}

/// Parses a single-range `Range` header (`bytes=a-b`, `bytes=a-`, `bytes=-n`).
///
/// Multi-range requests and malformed headers fall back to [`RangeRequest::Full`],
/// which RFC 9110 permits.
pub fn parse_range(header: Option<&str>, size: u64) -> RangeRequest {
    let Some(spec) = header.and_then(|h| h.trim().strip_prefix("bytes=")) else {
        return RangeRequest::Full;
    };
    if spec.contains(',') {
        return RangeRequest::Full;
    }
    let Some((a, b)) = spec.split_once('-') else {
        return RangeRequest::Full;
    };
    let (a, b) = (a.trim(), b.trim());
    let range = if a.is_empty() {
        // Suffix range: the last n bytes.
        let Ok(n) = b.parse::<u64>() else {
            return RangeRequest::Full;
        };
        if n == 0 {
            return RangeRequest::Unsatisfiable;
        }
        size.saturating_sub(n)..size
    } else {
        let Ok(start) = a.parse::<u64>() else {
            return RangeRequest::Full;
        };
        let end = if b.is_empty() {
            size
        } else {
            match b.parse::<u64>() {
                Ok(e) if e >= start => e.saturating_add(1).min(size),
                _ => return RangeRequest::Full,
            }
        };
        if start >= size {
            return RangeRequest::Unsatisfiable;
        }
        start..end
    };
    if range.start >= range.end {
        return RangeRequest::Unsatisfiable;
    }
    RangeRequest::Partial(range)
}

/// Builds a `Content-Disposition` value with an ASCII fallback and an RFC 5987
/// UTF-8 `filename*`.
pub fn content_disposition(name: &str, inline: bool) -> HeaderValue {
    let kind = if inline { "inline" } else { "attachment" };
    let fallback: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded = utf8_percent_encode(name, ATTR_CHAR);
    HeaderValue::from_str(&format!(
        "{kind}; filename=\"{fallback}\"; filename*=UTF-8''{encoded}"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

/// A weak validator derived from size and modification time.
pub fn etag(entry: &Entry) -> String {
    let mtime = entry
        .modified
        .map(|m| m.unix_timestamp_nanos())
        .unwrap_or_default();
    format!("W/\"{:x}-{:x}\"", entry.size, mtime)
}

/// Streams the file at `path` (a real path inside the source), honouring
/// `Range` and `If-None-Match`.
///
/// # Errors
///
/// `NotFound`, `IsADirectory` (as bad request), or an unsatisfiable range.
pub async fn file_response(
    storage: &dyn Storage,
    path: &VPath,
    req_headers: &HeaderMap,
    inline: bool,
) -> AppResult<Response> {
    let entry = storage.stat(path).await?;
    if entry.is_dir() {
        return Err(AppError::bad_request(
            "cannot download a folder this way; use zip",
        ));
    }
    let tag = etag(&entry);
    let mut builder = Response::builder()
        .header(header::ETAG, &tag)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "private, no-cache")
        .header(header::CONTENT_SECURITY_POLICY, FILE_CSP)
        .header(
            header::CONTENT_DISPOSITION,
            content_disposition(&entry.name, inline),
        );
    if let Some(m) = entry.modified {
        builder = builder.header(header::LAST_MODIFIED, httpdate::fmt_http_date(m.into()));
    }
    if req_headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == tag || t.trim() == "*"))
    {
        return builder
            .status(StatusCode::NOT_MODIFIED)
            .body(Body::empty())
            .map_err(|e| AppError::Internal(e.into()));
    }
    let mime = entry.mime.as_deref().unwrap_or("application/octet-stream");
    builder = builder.header(header::CONTENT_TYPE, mime);
    let range = parse_range(
        req_headers.get(header::RANGE).and_then(|v| v.to_str().ok()),
        entry.size,
    );
    let resp = match range {
        RangeRequest::Unsatisfiable => {
            return Err(AppError::RangeNotSatisfiable { size: entry.size });
        }
        RangeRequest::Full => {
            let stream = storage.read(path, None).await?;
            builder
                .status(StatusCode::OK)
                .header(header::CONTENT_LENGTH, entry.size)
                .body(Body::from_stream(stream))
        }
        RangeRequest::Partial(r) => {
            let stream = storage.read(path, Some(r.clone())).await?;
            builder
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_LENGTH, r.end - r.start)
                .header(
                    header::CONTENT_RANGE,
                    format!("bytes {}-{}/{}", r.start, r.end - 1, entry.size),
                )
                .body(Body::from_stream(stream))
        }
    };
    resp.map_err(|e| AppError::Internal(e.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        use RangeRequest::*;
        assert_eq!(parse_range(None, 10), Full);
        assert_eq!(parse_range(Some("bytes=0-4"), 10), Partial(0..5));
        assert_eq!(parse_range(Some("bytes=5-"), 10), Partial(5..10));
        assert_eq!(parse_range(Some("bytes=-3"), 10), Partial(7..10));
        assert_eq!(parse_range(Some("bytes=-30"), 10), Partial(0..10));
        assert_eq!(parse_range(Some("bytes=8-100"), 10), Partial(8..10));
        assert_eq!(parse_range(Some("bytes=10-"), 10), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=-0"), 10), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=0-"), 0), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=5-2"), 10), Full);
        assert_eq!(parse_range(Some("bytes=0-1,4-5"), 10), Full);
        assert_eq!(parse_range(Some("items=0-1"), 10), Full);
        assert_eq!(parse_range(Some("bytes=x-1"), 10), Full);
    }

    #[test]
    fn disposition() {
        let v = content_disposition("résumé \"final\".pdf", false);
        let s = v.to_str().unwrap();
        assert!(s.starts_with("attachment; filename=\"r_sum_ _final_.pdf\""));
        assert!(s.contains("filename*=UTF-8''r%C3%A9sum%C3%A9%20%22final%22.pdf"));
        assert!(
            content_disposition("a.txt", true)
                .to_str()
                .unwrap()
                .starts_with("inline")
        );
    }
}
