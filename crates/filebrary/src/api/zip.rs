//! On-the-fly zip archives of a selection of files and folders.
//!
//! The archive is produced while it is sent: nothing is staged on disk and
//! memory stays bounded (a small channel of 64 KiB chunks). The `zip` crate's
//! streaming writer runs on a blocking thread; reads from the [`Storage`]
//! backend are driven from that thread through the runtime handle.
//!
//! Already-compressed formats (media, archives, office files) are stored;
//! everything else is deflated at a fast level. ZIP64 is used for large files.
//! Symlinks are never followed by the local backend's listings beyond the
//! source root, so archives cannot pull in outside files.

use std::{io::Write, sync::Arc};

use axum::{
    body::Body,
    http::{StatusCode, header},
    response::Response,
};
use bytes::Bytes;
use futures::StreamExt;
use tokio::sync::mpsc;
use zip::{CompressionMethod, write::SimpleFileOptions};

use super::download::content_disposition;
use crate::{
    error::{AppError, AppResult},
    storage::{EntryKind, Storage, VPath},
};

const CHUNK: usize = 64 * 1024;

/// Extensions whose content is already compressed.
const STORED_EXT: &[&str] = &[
    "zip", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "jpg", "jpeg", "png", "gif", "webp",
    "heic", "avif", "mp3", "m4a", "aac", "ogg", "opus", "flac", "mp4", "m4v", "mkv", "webm", "mov",
    "avi", "docx", "xlsx", "pptx", "odt", "ods", "odp", "epub", "pdf", "jar", "apk",
];

/// A `Write` that forwards bytes to an async channel in chunks.
struct ChannelWriter {
    tx: mpsc::Sender<std::io::Result<Bytes>>,
    buf: Vec<u8>,
}

impl ChannelWriter {
    fn send_buf(&mut self) -> std::io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let chunk = Bytes::from(std::mem::replace(&mut self.buf, Vec::with_capacity(CHUNK)));
        self.tx
            .blocking_send(Ok(chunk))
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "client went away"))
    }
}

impl Write for ChannelWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        if self.buf.len() >= CHUNK {
            self.send_buf()?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.send_buf()
    }
}

fn options_for(name: &str, size: u64) -> SimpleFileOptions {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    let stored = ext.is_some_and(|e| STORED_EXT.contains(&e.as_str()));
    let opts = SimpleFileOptions::default().large_file(size >= u64::from(u32::MAX) - 1024);
    if stored {
        opts.compression_method(CompressionMethod::Stored)
    } else {
        opts.compression_method(CompressionMethod::Deflated)
            .compression_level(Some(1))
    }
}

/// Writes `items` (real paths) into the archive, recursing into folders.
fn write_archive(
    rt: &tokio::runtime::Handle,
    storage: &dyn Storage,
    items: &[VPath],
    out: ChannelWriter,
) -> anyhow::Result<()> {
    let mut zip = zip::ZipWriter::new_stream(out);
    // The root has no name; archive its children instead.
    let expanded;
    let items = if items.len() == 1 && items[0].is_root() {
        expanded = rt
            .block_on(storage.list(&VPath::root()))?
            .into_iter()
            // Never pack the trash folder when zipping a whole source.
            .filter(|e| e.name != crate::organize::trash::TRASH_DIR)
            .map(|e| e.path)
            .collect::<Vec<_>>();
        &expanded[..]
    } else {
        items
    };
    // (real path, name inside the archive)
    let mut stack: Vec<(VPath, String)> = items
        .iter()
        .rev()
        .filter_map(|p| p.name().map(|n| (p.clone(), n.to_owned())))
        .collect();
    while let Some((path, name)) = stack.pop() {
        let entry = rt.block_on(storage.stat(&path))?;
        match entry.kind {
            EntryKind::Dir => {
                zip.add_directory(format!("{name}/"), SimpleFileOptions::default())?;
                let mut children = rt.block_on(storage.list(&path))?;
                children.sort_by(|a, b| b.name.cmp(&a.name));
                for c in children {
                    stack.push((c.path.clone(), format!("{name}/{}", c.name)));
                }
            }
            EntryKind::File => {
                zip.start_file(name.as_str(), options_for(&name, entry.size))?;
                let mut stream = rt.block_on(storage.read(&path, None))?;
                while let Some(chunk) = rt.block_on(stream.next()) {
                    zip.write_all(&chunk?)?;
                }
            }
        }
    }
    let mut out = zip.finish()?.into_inner();
    out.flush()?;
    Ok(())
}

/// Returns a streaming zip response of `items` (real paths in `storage`).
///
/// `file_name` is the suggested download name (without `.zip`).
///
/// Every item is checked before streaming starts, so a missing or forbidden
/// item yields a proper error status instead of a broken download.
///
/// # Errors
///
/// Fails if the selection is empty or an item cannot be accessed.
pub async fn zip_response(
    storage: Arc<dyn Storage>,
    items: Vec<VPath>,
    file_name: &str,
) -> AppResult<Response> {
    if items.is_empty() || items.iter().any(VPath::is_root) && items.len() > 1 {
        return Err(AppError::bad_request(
            "select at least one item (and not the root with others)",
        ));
    }
    for item in &items {
        storage.stat(item).await?;
    }
    let (tx, rx) = mpsc::channel::<std::io::Result<Bytes>>(8);
    let rt = tokio::runtime::Handle::current();
    let err_tx = tx.clone();
    tokio::task::spawn_blocking(move || {
        let out = ChannelWriter {
            tx,
            buf: Vec::with_capacity(CHUNK),
        };
        if let Err(e) = write_archive(&rt, storage.as_ref(), &items, out) {
            tracing::warn!(error = %e, "zip stream aborted");
            // Make the client see a failed transfer instead of a truncated "success".
            let _ = err_tx.blocking_send(Err(std::io::Error::other("archive failed")));
        }
    });
    let stream =
        futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (v, rx)) });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(
            header::CONTENT_DISPOSITION,
            content_disposition(&format!("{file_name}.zip"), false),
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(stream))
        .map_err(|e| AppError::Internal(e.into()))
}
