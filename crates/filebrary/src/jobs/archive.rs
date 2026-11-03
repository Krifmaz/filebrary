//! The `archive.create` job: packs a selection of files and folders into a
//! new zip or tar.gz file in a chosen folder.
//!
//! Parameters:
//!
//! ```json
//! { "source": "files", "paths": ["/photos/2024", "/notes.txt"],
//!   "dest": "/", "name": "backup", "format": "zip" }
//! ```
//!
//! `name` defaults to the single selected item's name, or `archive`. The
//! output never overwrites anything: a free name such as `backup (1).zip` is
//! chosen instead. The user needs `download` on every input and `upload` on
//! the destination folder.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use async_trait::async_trait;
use filebrary_archive::{ArchiveBuilder, CreateFormat};
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use tokio_util::io::{StreamReader, SyncIoBridge};

use super::{JobContext, JobError, JobHandler, JobResult, Prepared, stage};
use crate::{
    auth::{Perms, User},
    error::{AppError, AppResult},
    state::AppState,
    storage::{EntryKind, VPath},
};

/// Handler for `archive.create`.
pub struct CreateArchive;

#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Params {
    source: String,
    paths: Vec<String>,
    dest: String,
    #[serde(default)]
    name: Option<String>,
    format: CreateFormat,
}

/// Most items that can be selected in one request.
const MAX_SELECTION: usize = 10_000;

#[async_trait]
impl JobHandler for CreateArchive {
    fn describe(&self) -> Value {
        json!({ "title": "Create archive", "formats": ["zip", "tar.gz"] })
    }

    async fn prepare(&self, state: &AppState, user: &User, params: Value) -> AppResult<Prepared> {
        let mut p: Params = serde_json::from_value(params)
            .map_err(|e| AppError::bad_request(format!("invalid parameters: {e}")))?;
        if p.paths.is_empty() {
            return Err(AppError::bad_request("select at least one file or folder"));
        }
        if p.paths.len() > MAX_SELECTION {
            return Err(AppError::bad_request("too many items selected"));
        }
        let mut seen = std::collections::HashSet::new();
        let mut first_name = None;
        for path in &p.paths {
            let vp = VPath::parse(path)?;
            if vp.is_root() {
                return Err(AppError::bad_request(
                    "cannot archive the whole source; select folders",
                ));
            }
            if !seen.insert(vp.clone()) {
                return Err(AppError::bad_request("an item is selected twice"));
            }
            first_name.get_or_insert_with(|| vp.name().unwrap_or("archive").to_owned());
            let (storage, vp) =
                stage::resolve_tree(state, user, &p.source, path, Perms::DOWNLOAD).await?;
            storage.stat(&vp).await?;
        }
        let (storage, dest) =
            stage::resolve(state, user, &p.source, &p.dest, Perms::UPLOAD).await?;
        if !storage.stat(&dest).await?.is_dir() {
            return Err(AppError::bad_request("the destination is not a folder"));
        }
        let name = match p
            .name
            .take()
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty())
        {
            Some(n) => n,
            None if p.paths.len() == 1 => first_name.unwrap_or_else(|| "archive".into()),
            None => "archive".into(),
        };
        let name = strip_archive_ext(&name).to_owned();
        // Validates the name as a single path component.
        VPath::root()
            .join(&format!("{name}.{}", p.format.extension()))
            .map_err(|e| AppError::bad_request(format!("invalid archive name: {e}")))?;
        let title = format!(
            "Create {name}.{} ({} item{})",
            p.format.extension(),
            p.paths.len(),
            if p.paths.len() == 1 { "" } else { "s" }
        );
        p.name = Some(name);
        Ok(Prepared {
            title,
            params: serde_json::to_value(&p).map_err(anyhow::Error::from)?,
        })
    }

    async fn run(&self, ctx: &JobContext, params: &Value) -> JobResult<Value> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| JobError::failed(format!("invalid parameters: {e}")))?;
        let user = ctx.user().await?;
        let limit = ctx.state.config.jobs.max_archive_files;

        // Collect (archive name, entry) for every file and folder.
        let mut items: Vec<(String, crate::storage::Entry)> = Vec::new();
        let mut storage = None;
        for path in &p.paths {
            let (st, vp) =
                stage::resolve_tree(&ctx.state, &user, &p.source, path, Perms::DOWNLOAD).await?;
            let base = vp.parent().unwrap_or_default();
            for e in stage::walk(st.as_ref(), &vp, limit, &ctx.cancel).await? {
                let rel = e
                    .path
                    .strip_prefix(&base)
                    .map(|r| r.components().join("/"))
                    .unwrap_or_else(|| e.name.clone());
                items.push((rel, e));
                if items.len() > limit {
                    return Err(JobError::failed(format!(
                        "the selection has more than {limit} files and folders"
                    )));
                }
            }
            storage = Some(st);
        }
        let storage = storage.ok_or_else(|| JobError::failed("nothing selected"))?;
        let (out_storage, dest) =
            stage::resolve(&ctx.state, &user, &p.source, &p.dest, Perms::UPLOAD).await?;

        let total: u64 = items.iter().map(|(_, e)| e.size).sum();
        let done = Arc::new(AtomicU64::new(0));
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let out_path = ctx.tmp.join(format!("out.{}", p.format.extension()));

        let worker = {
            let (done, cancel_flag, out_path, storage) = (
                done.clone(),
                cancel_flag.clone(),
                out_path.clone(),
                storage.clone(),
            );
            let rt = tokio::runtime::Handle::current();
            let format = p.format;
            tokio::task::spawn_blocking(move || -> JobResult<u64> {
                let file = std::fs::File::create(&out_path)?;
                let mut b =
                    ArchiveBuilder::new(format, std::io::BufWriter::new(file), &cancel_flag);
                for (name, e) in &items {
                    let mtime = e.modified.map(OffsetDateTime::unix_timestamp);
                    match e.kind {
                        EntryKind::Dir => b.add_dir(name, mtime).map_err(archive_err)?,
                        EntryKind::File => {
                            let stream = rt.block_on(storage.read(&e.path, None))?;
                            let reader = SyncIoBridge::new_with_handle(
                                StreamReader::new(stream),
                                rt.clone(),
                            );
                            b.add_file(name, e.size, mtime, reader, &mut |n| {
                                done.fetch_add(n, Ordering::Relaxed);
                            })
                            .map_err(archive_err)?;
                        }
                    }
                }
                let files = items.len() as u64;
                use std::io::Write;
                b.finish().map_err(archive_err)?.flush()?;
                Ok(files)
            })
        };

        // Report progress and forward cancellation while the archive is built.
        let mut worker = worker;
        let count = loop {
            tokio::select! {
                r = &mut worker => break r.map_err(|e| JobError::failed(format!("archive task failed: {e}")))??,
                () = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                    if ctx.cancel.is_cancelled() {
                        cancel_flag.store(true, Ordering::Relaxed);
                    }
                    let d = done.load(Ordering::Relaxed);
                    let frac = if total == 0 { 0.0 } else { d as f64 / total as f64 * 0.95 };
                    ctx.progress(frac, Some("packing")).await;
                }
            }
        };
        ctx.check_cancel()?;
        ctx.progress(0.96, Some("saving")).await;
        let name = p.name.as_deref().unwrap_or("archive");
        let (stem, ext) = match p.format {
            CreateFormat::Zip => (name.to_owned(), "zip"),
            CreateFormat::TarGz => (format!("{name}.tar"), "gz"),
        };
        let written = stage::upload_new(
            &stage::Writer {
                state: &ctx.state,
                user: &user,
                source: &p.source,
            },
            out_storage.as_ref(),
            &dest,
            &stem,
            ext,
            &out_path,
        )
        .await?;
        ctx.record_write(
            &user,
            &p.source,
            &written,
            crate::activity::action::UPLOAD,
            "archive.create",
        )
        .await;
        let visible = written.to_string();
        let size = tokio::fs::metadata(&out_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        Ok(json!({ "source": p.source, "path": visible, "entries": count, "size": size }))
    }
}

fn strip_archive_ext(name: &str) -> &str {
    let lower = name.to_ascii_lowercase();
    for ext in [".tar.gz", ".tgz", ".zip"] {
        if lower.ends_with(ext) && name.len() > ext.len() {
            return &name[..name.len() - ext.len()];
        }
    }
    name
}

fn archive_err(e: filebrary_archive::ArchiveError) -> JobError {
    match e {
        filebrary_archive::ArchiveError::Cancelled => JobError::Cancelled,
        other => JobError::failed(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_extensions() {
        assert_eq!(strip_archive_ext("a.zip"), "a");
        assert_eq!(strip_archive_ext("a.TAR.GZ"), "a");
        assert_eq!(strip_archive_ext(".zip"), ".zip");
        assert_eq!(strip_archive_ext("a.txt"), "a.txt");
    }
}
