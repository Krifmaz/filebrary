//! Response builders for shared content: single files (with `Range`) and
//! on-the-fly zip archives.
//!
//! These mirror what the authenticated download routes need. If the core
//! download module grows equivalent helpers, these should delegate to them.

use std::{
    io::{self, Write},
    ops::Range,
    sync::{Arc, Mutex},
};

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use bytes::Bytes;
use futures::StreamExt;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use tokio::sync::mpsc;
use zip::{CompressionMethod, DateTime, ZipWriter, write::SimpleFileOptions};

use crate::{
    error::{AppError, AppResult},
    storage::{Entry, Storage, StorageError, VPath, guess_mime},
};

/// Characters left unescaped in an RFC 5987 `filename*` value.
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

/// Most entries put into one archive, to bound the work a single link can cause.
const MAX_ZIP_ENTRIES: usize = 100_000;

/// Builds a `Content-Disposition` header value with an ASCII fallback and a
/// UTF-8 `filename*`.
pub fn content_disposition(kind: &str, name: &str) -> HeaderValue {
    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let v = format!(
        "{kind}; filename=\"{ascii}\"; filename*=UTF-8''{}",
        utf8_percent_encode(name, ATTR_CHAR)
    );
    HeaderValue::from_str(&v).unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

/// Whether a MIME type is safe to render inline on our origin. Anything that
/// could run script (HTML, SVG, XML, JavaScript...) is always downloaded.
fn inline_safe(mime: &str) -> bool {
    (mime.starts_with("image/") && mime != "image/svg+xml")
        || mime.starts_with("video/")
        || mime.starts_with("audio/")
        || mime == "application/pdf"
        || mime == "text/plain"
}

/// Parses a single `bytes=` range against `size`. Multi-range requests and
/// anything malformed are ignored (the whole file is sent), as RFC 9110 allows.
fn parse_range(headers: &HeaderMap, size: u64) -> Option<AppResult<Range<u64>>> {
    let v = headers.get(header::RANGE)?.to_str().ok()?;
    let spec = v.strip_prefix("bytes=")?.trim();
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let range = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        if n == 0 {
            return Some(Err(AppError::RangeNotSatisfiable { size }));
        }
        size.saturating_sub(n)..size
    } else {
        let start: u64 = a.parse().ok()?;
        let end = if b.is_empty() {
            size
        } else {
            b.parse::<u64>().ok()?.saturating_add(1).min(size)
        };
        start..end
    };
    if range.start >= range.end || range.start >= size {
        return Some(Err(AppError::RangeNotSatisfiable { size }));
    }
    Some(Ok(range))
}

/// Returns whether a request asks for a range that does not start at byte 0.
pub fn is_continuation(headers: &HeaderMap) -> bool {
    headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .is_some_and(|s| !s.trim_start().starts_with("0-"))
}

/// Streams one file, honouring a single `Range`. `inline` is a request to
/// display rather than download; it is only granted for harmless types.
pub async fn file_response(
    storage: &dyn Storage,
    entry: &Entry,
    headers: &HeaderMap,
    inline: bool,
) -> AppResult<Response> {
    if entry.is_dir() {
        return Err(StorageError::IsADirectory.into());
    }
    let size = entry.size;
    let mime = entry
        .mime
        .clone()
        .unwrap_or_else(|| guess_mime(&entry.name));
    let range = parse_range(headers, size).transpose()?;
    let stream = storage.read(&entry.path, range.clone()).await?;

    let mut resp = Response::new(Body::from_stream(stream));
    let h = resp.headers_mut();
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(v) = HeaderValue::from_str(&mime) {
        h.insert(header::CONTENT_TYPE, v);
    }
    let disposition = if inline && inline_safe(&mime) {
        "inline"
    } else {
        "attachment"
    };
    h.insert(
        header::CONTENT_DISPOSITION,
        content_disposition(disposition, &entry.name),
    );
    // Shared content is untrusted: never let it run in our origin. (Browsers
    // refuse to show PDFs in a sandbox, and PDF viewers do not run page script.)
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        if mime == "application/pdf" {
            HeaderValue::from_static(
                "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'",
            )
        } else {
            HeaderValue::from_static(
                "sandbox; default-src 'none'; img-src 'self'; media-src 'self'",
            )
        },
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    if let Some(m) = entry.modified
        && let Ok(v) = HeaderValue::from_str(&httpdate::fmt_http_date(m.into()))
    {
        h.insert(header::LAST_MODIFIED, v);
    }
    match range {
        Some(r) => {
            *resp.status_mut() = StatusCode::PARTIAL_CONTENT;
            let h = resp.headers_mut();
            h.insert(header::CONTENT_LENGTH, HeaderValue::from(r.end - r.start));
            if let Ok(v) = HeaderValue::from_str(&format!("bytes {}-{}/{size}", r.start, r.end - 1))
            {
                h.insert(header::CONTENT_RANGE, v);
            }
        }
        None => {
            resp.headers_mut()
                .insert(header::CONTENT_LENGTH, HeaderValue::from(size));
        }
    }
    Ok(resp)
}

/// A `Write` sink whose bytes are drained after each synchronous zip call
/// and forwarded to the response body.
#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl SharedBuf {
    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// One thing to put in an archive: where it lives and the name it gets.
pub struct ZipItem {
    /// Storage holding the item.
    pub storage: Arc<dyn Storage>,
    /// Path in that storage.
    pub path: VPath,
    /// Name of the item at the top of the archive.
    pub name: String,
}

fn zip_time(e: &Entry) -> DateTime {
    e.modified
        .and_then(|t| {
            DateTime::from_date_and_time(
                u16::try_from(t.year()).ok()?,
                t.month().into(),
                t.day(),
                t.hour(),
                t.minute(),
                t.second(),
            )
            .ok()
        })
        .unwrap_or_else(DateTime::default_for_write)
}

/// Streams a zip archive of `items` (files or folders, recursively). Entries
/// are stored without compression: it keeps CPU and memory flat and most
/// large files are already compressed.
pub fn zip_response(items: Vec<ZipItem>, archive_name: &str) -> Response {
    let (tx, rx) = mpsc::channel::<io::Result<Bytes>>(8);
    tokio::spawn(async move {
        if let Err(e) = write_zip(items, &tx).await {
            tracing::warn!(error = %e, "share archive aborted");
            let _ = tx.send(Err(io::Error::other(e.to_string()))).await;
        }
    });
    let body = Body::from_stream(tokio_stream_from(rx));
    let mut resp = Response::new(body);
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        content_disposition("attachment", archive_name),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

fn tokio_stream_from(
    mut rx: mpsc::Receiver<io::Result<Bytes>>,
) -> impl futures::Stream<Item = io::Result<Bytes>> {
    futures::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

async fn flush(buf: &SharedBuf, tx: &mpsc::Sender<io::Result<Bytes>>) -> io::Result<()> {
    let data = buf.take();
    if data.is_empty() {
        return Ok(());
    }
    tx.send(Ok(Bytes::from(data)))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "client went away"))
}

async fn write_zip(items: Vec<ZipItem>, tx: &mpsc::Sender<io::Result<Bytes>>) -> io::Result<()> {
    let buf = SharedBuf::default();
    let mut zw = ZipWriter::new_stream(buf.clone());
    let mut count = 0usize;
    for item in items {
        // Depth-first walk with an explicit stack: (path in storage, name in archive).
        let mut stack = vec![(item.path.clone(), item.name.clone())];
        while let Some((path, name)) = stack.pop() {
            count += 1;
            if count > MAX_ZIP_ENTRIES {
                return Err(io::Error::other("too many files for one archive"));
            }
            let entry = match item.storage.stat(&path).await {
                Ok(e) => e,
                // Vanished, or a symlink pointing outside: skip it silently.
                Err(StorageError::NotFound | StorageError::OutsideRoot) => continue,
                Err(e) => return Err(io::Error::other(e.to_string())),
            };
            let opts = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .last_modified_time(zip_time(&entry))
                .large_file(entry.size >= u64::from(u32::MAX));
            if entry.is_dir() {
                zw.add_directory(format!("{name}/"), opts)
                    .map_err(io::Error::other)?;
                let mut children = match item.storage.list(&path).await {
                    Ok(c) => c,
                    Err(StorageError::NotFound | StorageError::OutsideRoot) => continue,
                    Err(e) => return Err(io::Error::other(e.to_string())),
                };
                children.sort_by(|a, b| b.name.cmp(&a.name));
                for c in children {
                    stack.push((c.path, format!("{name}/{}", c.name)));
                }
            } else {
                zw.start_file(name, opts).map_err(io::Error::other)?;
                let mut stream = match item.storage.read(&path, None).await {
                    Ok(s) => s,
                    Err(StorageError::NotFound | StorageError::OutsideRoot) => continue,
                    Err(e) => return Err(io::Error::other(e.to_string())),
                };
                while let Some(chunk) = stream.next().await {
                    zw.write_all(&chunk?)?;
                    flush(&buf, tx).await?;
                }
            }
            flush(&buf, tx).await?;
        }
    }
    zw.finish().map_err(io::Error::other)?;
    flush(&buf, tx).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(range: &str) -> HeaderMap {
        let mut m = HeaderMap::new();
        m.insert(header::RANGE, HeaderValue::from_str(range).unwrap());
        m
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range(&h("bytes=0-9"), 100).unwrap().unwrap(), 0..10);
        assert_eq!(parse_range(&h("bytes=90-"), 100).unwrap().unwrap(), 90..100);
        assert_eq!(parse_range(&h("bytes=-10"), 100).unwrap().unwrap(), 90..100);
        assert_eq!(
            parse_range(&h("bytes=50-500"), 100).unwrap().unwrap(),
            50..100
        );
        assert!(parse_range(&h("bytes=100-"), 100).unwrap().is_err());
        assert!(parse_range(&h("bytes=0-1,5-6"), 100).is_none());
        assert!(parse_range(&HeaderMap::new(), 100).is_none());
        assert!(is_continuation(&h("bytes=5-")));
        assert!(!is_continuation(&h("bytes=0-")));
        assert!(!is_continuation(&HeaderMap::new()));
    }

    #[test]
    fn dispositions() {
        let v = content_disposition("attachment", "résumé \"x\".pdf");
        let s = v.to_str().unwrap();
        assert!(
            s.starts_with("attachment; filename=\"r_sum_ _x_.pdf\""),
            "{s}"
        );
        assert!(
            s.contains("filename*=UTF-8''r%C3%A9sum%C3%A9%20%22x%22.pdf"),
            "{s}"
        );
    }

    #[test]
    fn inline_rules() {
        assert!(inline_safe("image/png"));
        assert!(!inline_safe("image/svg+xml"));
        assert!(!inline_safe("text/html"));
        assert!(inline_safe("application/pdf"));
    }
}
