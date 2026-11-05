//! Filebrary's notice banners and Diagnostics checks for the kit.
//!
//! Notices (re-checked on their own, and on Check again):
//!
//! * `data`: the data directory can't be written (every minute).
//! * `sources`: one or more sources can't be reached (every five minutes).
//!
//! Diagnostics: the data directory, both databases, every source and the
//! preview tools.

use std::{path::Path, time::Duration};

use kit_server_core::Db;
use kit_server_diagnostics::{Check, Kind, Registry};
use kit_server_notices::{Notice, NoticeCheck, Notices, Severity};

use crate::{config::SourceKind, state::AppState, storage::VPath};

/// The id of the data directory notice.
pub const DATA_NOTICE: &str = "data";
/// The id of the unreachable sources notice.
pub const SOURCES_NOTICE: &str = "sources";

/// How long one source or database check may take.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether a file can be made in `dir`: `Ok`, or the reason in words.
pub fn writable(dir: &Path) -> Result<(), String> {
    let probe = dir.join(format!(".filebrary-write-test-{}", std::process::id()));
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err("missing".into()),
        Err(e) if e.raw_os_error() == Some(30) => Err("read-only".into()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            Err("not writable by the server's user".into())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Probes the data directory off the async threads.
async fn data_dir_state(dir: std::path::PathBuf) -> Result<(), String> {
    tokio::task::spawn_blocking(move || writable(&dir))
        .await
        .unwrap_or_else(|e| Err(e.to_string()))
}

/// Whether the source `name` answers at its root within [`CHECK_TIMEOUT`].
async fn source_state(st: &AppState, name: &str) -> Result<(), String> {
    let Some(src) = st.source(name) else {
        return Err("not configured".into());
    };
    match tokio::time::timeout(CHECK_TIMEOUT, src.storage.stat(&VPath::root())).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!(
            "no answer within {} seconds",
            CHECK_TIMEOUT.as_secs()
        )),
    }
}

/// The notice banners.
pub fn notices() -> Notices<AppState> {
    Notices::new()
        .register(
            NoticeCheck::kept(DATA_NOTICE, Severity::Warn, |st: AppState| async move {
                let dir = st.config.data_dir.clone();
                match data_dir_state(dir.clone()).await {
                    Ok(()) => Notice::fine(),
                    Err(why) => Notice::failing(format!(
                        "The data folder {} is {why}. Check the disk and the mount, and that the server's user may write to it.",
                        dir.display()
                    )),
                }
            })
            .every(Duration::from_secs(60)),
        )
        .register(
            NoticeCheck::kept(SOURCES_NOTICE, Severity::Warn, |st: AppState| async move {
                let mut down = Vec::new();
                for name in st.sources.keys() {
                    if let Err(why) = source_state(&st, name).await {
                        down.push(format!("{name} ({why})"));
                    }
                }
                if down.is_empty() {
                    Notice::fine()
                } else {
                    let what = if down.len() == 1 { "This source can't" } else { "These sources can't" };
                    Notice::failing(format!(
                        "{what} be reached: {}. Its files are hidden until it answers again; Diagnostics says more.",
                        down.join(", ")
                    ))
                }
            })
            .every(Duration::from_secs(300)),
        )
}

/// The Diagnostics page's checks.
pub fn diagnostics() -> Registry<AppState> {
    Registry::new()
        .check("data", |st: AppState| async move {
            let shown = st.config.data_dir.display().to_string();
            match data_dir_state(st.config.data_dir.clone()).await {
                Ok(()) => Check::new("Data folder", Kind::Path, true, format!("{shown} · writable"))
                    .request(shown),
                Err(why) => Check::new("Data folder", Kind::Path, false, format!("{shown} · {why}"))
                    .request(shown)
                    .fix("Check the disk and the mount, and that the server's user may write to it (chown the folder to that user)"),
            }
        })
        .check("database", |st: AppState| async move {
            let q = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users").fetch_one(&st.db);
            match tokio::time::timeout(CHECK_TIMEOUT, q).await {
                Ok(Ok(n)) => Check::new("Database", Kind::Path, true, format!("filebrary.db · {n} accounts")),
                Ok(Err(e)) => Check::new("Database", Kind::Path, false, format!("filebrary.db · {e}"))
                    .fix("Check the data folder's disk; restore filebrary.db from a backup if it is damaged"),
                Err(_) => Check::new("Database", Kind::Path, false, "filebrary.db · no answer within 5 seconds")
                    .fix("Check the data folder's disk and whether another process holds the database"),
            }
        })
        .check("kit-database", |st: AppState| async move {
            match st.kit.db.one("SELECT COUNT(*) AS n FROM kit_migrations", vec![]) {
                Ok(_) => Check::new("Settings database", Kind::Path, true, format!("{} · OK", super::DB_FILE)),
                Err(e) => Check::new("Settings database", Kind::Path, false, format!("{} · {}", super::DB_FILE, e.detail))
                    .fix("Check the data folder's disk; kit.db holds settings, notices and bug reports and is made again when missing"),
            }
        })
        .group("sources", |st: AppState| async move {
            let mut out = Vec::new();
            for sc in &st.config.sources {
                let (kind, kind_word) = match sc.kind {
                    SourceKind::Local => (Kind::Path, "folder"),
                    SourceKind::S3 => (Kind::Service, "S3"),
                    SourceKind::Sftp => (Kind::Service, "SFTP"),
                    SourceKind::Smb => (Kind::Service, "SMB"),
                };
                let name = format!("Source {}", sc.name);
                out.push(match source_state(&st, &sc.name).await {
                    Ok(()) => Check::new(name, kind, true, format!("{kind_word} · reachable")),
                    Err(why) => Check::new(name, kind, false, format!("{kind_word} · {why}")).fix(match sc.kind {
                        SourceKind::Local => "Check that the folder is mounted into the container and readable by the server's user",
                        _ => "Check the address, the credentials and that the server can reach it over the network",
                    }),
                });
            }
            Ok(out)
        })
        .group("tools", |st: AppState| async move {
            let Some(p) = st.previews.as_ref() else {
                return Ok(vec![]);
            };
            let t = p.tools();
            let tool = |name: &str, path: &Option<std::path::PathBuf>, what: &str| match path {
                Some(p) => Check::new(name, Kind::Tool, true, p.display().to_string()),
                None => Check::new(name, Kind::Tool, false, format!("not installed · {what} have no previews"))
                    .status(kit_server_diagnostics::Status::Warn)
                    .fix("Use the -full image, or install it on the server's PATH"),
            };
            Ok(vec![
                tool("ffmpeg", &t.ffmpeg, "videos"),
                tool("pdftoppm", &t.pdftoppm, "PDFs"),
                tool("LibreOffice", &t.soffice, "Office documents"),
            ])
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_writable_folder_passes() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(writable(dir.path()), Ok(()));
        assert_eq!(writable(&dir.path().join("nope")), Err("missing".into()));
    }
}
