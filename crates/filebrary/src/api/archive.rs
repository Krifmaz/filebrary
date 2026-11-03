//! Browsing inside archives without extracting them.
//!
//! | Method and path                                  | What it does |
//! |--------------------------------------------------|--------------|
//! | `GET /api/sources/{source}/archive?path=&dir=`   | List an archive: everything, or with `dir` only the direct children of that folder inside it |
//! | `GET /api/sources/{source}/archive/entry?path=&entry=` | Download one file from inside an archive |
//!
//! Supported: zip (and `.jar`, `.cbz`), tar, tar.gz/tgz and 7z. The archive
//! is read through the storage backend with ranged reads, so for zip and 7z
//! only the index and the requested entry are transferred. Both routes need
//! the `download` permission on the archive. Entry names are cleaned and
//! never touch the file system; entries with `..` in their name are hidden.
//! Limits from `[jobs]` (`max_archive_entries`, `max_archive_entry_mib`)
//! stop hostile archives.

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};
use bytes::Bytes;
use filebrary_archive::{ArchiveEntry, ArchiveError, ArchiveKind, ArchiveLimits};
use serde::{Deserialize, Serialize};

use crate::{
    auth::{AuthUser, Perms},
    error::{AppError, AppResult},
    jobs::stage::{self, StorageReader},
    state::AppState,
};

/// The archive routes (merged into `/api`).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sources/{source}/archive", get(list))
        .route("/sources/{source}/archive/entry", get(entry))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    path: String,
    dir: Option<String>,
}

#[derive(Debug, Serialize)]
struct Listing {
    kind: ArchiveKind,
    entries: Vec<ArchiveEntry>,
}

fn limits(state: &AppState) -> ArchiveLimits {
    ArchiveLimits {
        max_entries: state.config.jobs.max_archive_entries,
        max_entry_bytes: state
            .config
            .jobs
            .max_archive_entry_mib
            .saturating_mul(1024 * 1024),
    }
}

fn map_err(e: ArchiveError) -> AppError {
    match e {
        ArchiveError::NotFound => AppError::NotFound,
        ArchiveError::IsDirectory => AppError::bad_request("that entry is a folder"),
        ArchiveError::Unsupported => {
            AppError::bad_request("not a supported archive (zip, tar, tar.gz, 7z)")
        }
        ArchiveError::Corrupt(m) => AppError::bad_request(format!("cannot read this archive: {m}")),
        ArchiveError::TooLarge(m) => {
            AppError::bad_request(format!("archive too large to browse: {m}"))
        }
        ArchiveError::Cancelled => AppError::bad_request("cancelled"),
        ArchiveError::Io(e) => AppError::from(e),
    }
}

/// Opens the archive at the user-visible `path` and works out its kind
/// (by content first, then by name).
async fn open(
    state: &AppState,
    user: &crate::auth::User,
    source: &str,
    path: &str,
) -> AppResult<(ArchiveKind, StorageReader)> {
    let (storage, vp) = stage::resolve(state, user, source, path, Perms::DOWNLOAD).await?;
    let meta = storage.stat(&vp).await?;
    if meta.is_dir() {
        return Err(AppError::bad_request("that is a folder, not an archive"));
    }
    let mut reader = StorageReader::new(storage, vp, meta.size);
    let name = meta.name.clone();
    let (kind, reader) = tokio::task::spawn_blocking(move || {
        let k = ArchiveKind::sniff(&mut reader).ok().flatten();
        (k, reader)
    })
    .await
    .map_err(|e| anyhow::anyhow!("archive task: {e}"))?;
    let kind = kind
        .or_else(|| ArchiveKind::from_name(&name))
        .ok_or_else(|| map_err(ArchiveError::Unsupported))?;
    // A gzip file that is not a tarball is not browsable either; the tar
    // reader reports that as a corrupt archive.
    Ok((kind, reader))
}

async fn list(
    State(state): State<AppState>,
    AuthUser { user, .. }: AuthUser,
    Path(source): Path<String>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Listing>> {
    let (kind, reader) = open(&state, &user, &source, &q.path).await?;
    let limits = limits(&state);
    let all = tokio::task::spawn_blocking(move || filebrary_archive::list(kind, reader, limits))
        .await
        .map_err(|e| anyhow::anyhow!("archive task: {e}"))?
        .map_err(map_err)?;
    let entries = match q.dir.as_deref() {
        Some(d) => filebrary_archive::children(&all, d),
        None => all,
    };
    Ok(Json(Listing { kind, entries }))
}

#[derive(Debug, Deserialize)]
struct EntryQuery {
    path: String,
    entry: String,
}

enum Msg {
    Data(Bytes),
    Done,
    Err(ArchiveError),
}

/// A `Write` that forwards chunks to the response body.
struct ChannelWriter(tokio::sync::mpsc::Sender<Msg>);

impl std::io::Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .blocking_send(Msg::Data(Bytes::copy_from_slice(buf)))
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "client went away"))?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn entry(
    State(state): State<AppState>,
    AuthUser { user, .. }: AuthUser,
    Path(source): Path<String>,
    Query(q): Query<EntryQuery>,
) -> AppResult<Response> {
    let (kind, reader) = open(&state, &user, &source, &q.path).await?;
    let limits = limits(&state);
    let inner = filebrary_archive::clean_entry_path(&q.entry).ok_or(AppError::NotFound)?;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Msg>(8);
    {
        let inner = inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut w = std::io::BufWriter::with_capacity(64 * 1024, ChannelWriter(tx.clone()));
            let r = filebrary_archive::read_entry(kind, reader, &inner, limits, &mut w)
                .and_then(|_| std::io::Write::flush(&mut w).map_err(ArchiveError::Io));
            drop(w);
            let _ = tx.blocking_send(match r {
                Ok(_) => Msg::Done,
                Err(e) => Msg::Err(e),
            });
        });
    }
    // Wait for the first message so a missing entry is a proper 404.
    let first = match rx.recv().await {
        Some(Msg::Err(e)) => return Err(map_err(e)),
        None => {
            return Err(AppError::Internal(anyhow::anyhow!(
                "archive reader stopped"
            )));
        }
        Some(m) => m,
    };
    let stream = futures::stream::unfold((Some(first), rx), |(pending, mut rx)| async move {
        let msg = match pending {
            Some(m) => m,
            None => rx.recv().await?,
        };
        match msg {
            Msg::Data(b) => Some((Ok::<_, std::io::Error>(b), (None, rx))),
            Msg::Done => None,
            Msg::Err(e) => {
                tracing::warn!(error = %e, "archive entry stream failed");
                Some((Err(std::io::Error::other(e.to_string())), (None, rx)))
            }
        }
    });

    let file_name = inner.rsplit('/').next().unwrap_or("file").to_owned();
    let mime = crate::storage::guess_mime(&file_name);
    let mut resp = Body::from_stream(stream).into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&mime)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    // Content from an archive is untrusted: always a download, and sandboxed
    // in case a browser renders it anyway.
    h.insert(header::CONTENT_DISPOSITION, attachment(&file_name));
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox; default-src 'none'"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    Ok(resp)
}

/// `attachment; filename="..."; filename*=UTF-8''...` with a safe ASCII fallback.
fn attachment(name: &str) -> HeaderValue {
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
    let encoded =
        percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC).to_string();
    HeaderValue::from_str(&format!(
        "attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}"
    ))
    .unwrap_or(HeaderValue::from_static("attachment"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_header_is_safe() {
        let v = attachment("a\"b\r\nc.txt");
        let s = v.to_str().unwrap();
        assert!(s.starts_with("attachment; filename=\"a_b__c.txt\""), "{s}");
        assert!(!s.contains('\n'));
        let v = attachment("фото.jpg");
        assert!(v.to_str().unwrap().contains("filename*=UTF-8''%D1%84"));
    }
}
