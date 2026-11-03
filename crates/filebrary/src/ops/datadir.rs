//! Start-up check that the data directory is usable.
//!
//! The most common first-run failure in Docker is a bind-mounted data folder
//! that the host created as `root`, while the container runs as another user.
//! SQLite then reports only "unable to open database file". [`ensure_writable`]
//! catches this before the database is opened and explains the fix: the path,
//! the user the process runs as, who owns the folder, and the `chown` command.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::Path;

/// Files in the data directory that must be writable if they already exist.
const KNOWN_FILES: &[&str] = &[
    "filebrary.db",
    "filebrary.db-wal",
    "filebrary.db-shm",
    "secret.key",
];

/// Creates `data_dir` if needed and checks that this process can write to it
/// and to the database files already inside it.
///
/// # Errors
///
/// Returns an error whose message names the path, the current user and group
/// ids, the owner of the path, and the `chown` command that fixes it.
pub fn ensure_writable(data_dir: &Path) -> anyhow::Result<()> {
    if let Err(e) = fs::create_dir_all(data_dir) {
        return Err(explain(data_dir, data_dir, "create", &e));
    }
    let probe = data_dir.join(format!(".filebrary-write-test-{}", std::process::id()));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(f) => {
            drop(f);
            let _ = fs::remove_file(&probe);
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&probe);
        }
        Err(e) => return Err(explain(data_dir, data_dir, "write to", &e)),
    }
    for name in KNOWN_FILES {
        let file = data_dir.join(name);
        match OpenOptions::new().read(true).write(true).open(&file) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(explain(data_dir, &file, "write to", &e)),
        }
    }
    Ok(())
}

/// Builds the error message for a data directory this process cannot use.
fn explain(data_dir: &Path, path: &Path, action: &str, err: &io::Error) -> anyhow::Error {
    let (uid, gid) = process_ids();
    let ids = match (uid, gid) {
        (Some(u), Some(g)) => format!("uid {u}, gid {g}"),
        _ => "the current user".to_owned(),
    };
    let owner = owner_of(path)
        .or_else(|| owner_of(data_dir))
        .map(|(u, g)| format!(" It is owned by {u}:{g}."))
        .unwrap_or_default();
    let who = match (uid, gid) {
        (Some(u), Some(g)) => format!("{u}:{g}"),
        _ => "<uid>:<gid>".to_owned(),
    };
    anyhow::anyhow!(
        "cannot {action} {path} ({err}). Filebrary runs as {ids} and needs to write \
         to its data directory {dir}.{owner}\n\
         Fix: make the folder writable by that user, for example\n\
         \n    sudo chown -R {who} <folder>\n\n\
         In Docker, <folder> is the host folder mounted at {dir} \
         (Docker creates a missing bind-mount folder as root), \
         or run the container with `user:` set to the folder's owner.\n\
         See https://krifmaz.github.io/filebrary/install/docker.html#troubleshooting",
        path = path.display(),
        dir = data_dir.display(),
    )
}

/// The real user and group ids of this process, where the platform exposes
/// them without extra dependencies (Linux, through `/proc/self/status`).
fn process_ids() -> (Option<u32>, Option<u32>) {
    let Ok(status) = fs::read_to_string("/proc/self/status") else {
        return (None, None);
    };
    let field = |key: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(key))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|v| v.parse().ok())
    };
    (field("Uid:"), field("Gid:"))
}

/// The numeric owner of `path`, on Unix.
#[cfg(unix)]
fn owner_of(path: &Path) -> Option<(u32, u32)> {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).ok().map(|m| (m.uid(), m.gid()))
}

/// Ownership is not reported on this platform.
#[cfg(not(unix))]
fn owner_of(_path: &Path) -> Option<(u32, u32)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_and_accepts_a_writable_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("data");
        ensure_writable(&dir).unwrap();
        assert!(dir.is_dir());
        // The probe file is cleaned up.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test]
    fn message_names_path_ids_and_fix() {
        let err = explain(
            Path::new("/data"),
            Path::new("/data"),
            "write to",
            &io::Error::from(io::ErrorKind::PermissionDenied),
        )
        .to_string();
        assert!(err.contains("cannot write to /data"), "{err}");
        assert!(err.contains("sudo chown -R"), "{err}");
        assert!(err.contains("mounted at /data"), "{err}");
        if cfg!(target_os = "linux") {
            assert!(err.contains("uid "), "{err}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_read_only_dir() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("data");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        // root ignores permission bits; nothing to check then.
        let writable_anyway = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join("x"))
            .is_ok();
        if !writable_anyway {
            let err = ensure_writable(&dir).unwrap_err().to_string();
            assert!(err.contains("cannot write to"), "{err}");
            assert!(err.contains("chown"), "{err}");
        }
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_read_only_database_file() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("filebrary.db");
        fs::write(&db, b"").unwrap();
        fs::set_permissions(&db, fs::Permissions::from_mode(0o444)).unwrap();
        if OpenOptions::new().write(true).open(&db).is_err() {
            let err = ensure_writable(tmp.path()).unwrap_err().to_string();
            assert!(err.contains("filebrary.db"), "{err}");
        }
    }
}
