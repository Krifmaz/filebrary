//! Runs the backend conformance suite against every backend.
//!
//! The local backend always runs, against a temporary directory. The remote
//! backends run when their test server is configured through environment
//! variables, and are skipped (with a note on stderr) otherwise:
//!
//! | Backend | Variables |
//! |---|---|
//! | S3 (MinIO) | `FILEBRARY_TEST_S3_ENDPOINT`, `FILEBRARY_TEST_S3_BUCKET`, `FILEBRARY_TEST_S3_ACCESS_KEY`, `FILEBRARY_TEST_S3_SECRET_KEY` |
//! | SFTP | `FILEBRARY_TEST_SFTP_ADDR` (`host:port`), `FILEBRARY_TEST_SFTP_USER`, `FILEBRARY_TEST_SFTP_PASSWORD`, `FILEBRARY_TEST_SFTP_ROOT` (a writable directory) |
//! | SMB | `FILEBRARY_TEST_SMB_ADDR` (`host:port`), `FILEBRARY_TEST_SMB_SHARE`, `FILEBRARY_TEST_SMB_USER`, `FILEBRARY_TEST_SMB_PASSWORD` |
//!
//! Each run works in a fresh, randomly named folder or prefix and removes it
//! afterwards. Set `FILEBRARY_TEST_REQUIRE_REMOTE=1` (as CI does) to fail
//! instead of skipping when a variable is missing. `scripts/test-backends.sh`
//! starts the three servers in Docker and runs these tests.

// Without remote backends only the local test remains.
#![cfg_attr(
    not(any(feature = "s3", feature = "sftp", feature = "smb")),
    allow(unused)
)]

use filebrary_storage::{Storage, VPath, conformance, local::LocalStorage};

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Returns the variables, or `None` to skip (panics if remote tests are required).
fn require(backend: &str, names: &[&str]) -> Option<Vec<String>> {
    let vals: Vec<Option<String>> = names.iter().map(|n| env(n)).collect();
    if vals.iter().all(Option::is_some) {
        return Some(vals.into_iter().flatten().collect());
    }
    let missing: Vec<&str> = names
        .iter()
        .zip(&vals)
        .filter(|(_, v)| v.is_none())
        .map(|(n, _)| *n)
        .collect();
    assert!(
        env("FILEBRARY_TEST_REQUIRE_REMOTE").is_none(),
        "{backend} test server not configured: missing {missing:?}"
    );
    eprintln!("skipping {backend} backend tests: {missing:?} not set");
    None
}

fn run_id() -> String {
    format!("fbtest-{}", uuid::Uuid::new_v4().simple())
}

#[tokio::test]
async fn local_conforms() {
    let dir = tempfile::tempdir().unwrap();
    let s = LocalStorage::new(dir.path()).unwrap();
    conformance::run(&s).await;
}

#[cfg(feature = "s3")]
#[tokio::test]
async fn s3_conforms() {
    use filebrary_storage::s3::{S3Config, S3Storage};
    let Some(v) = require(
        "S3",
        &[
            "FILEBRARY_TEST_S3_ENDPOINT",
            "FILEBRARY_TEST_S3_BUCKET",
            "FILEBRARY_TEST_S3_ACCESS_KEY",
            "FILEBRARY_TEST_S3_SECRET_KEY",
        ],
    ) else {
        return;
    };
    let cfg = S3Config {
        endpoint: Some(v[0].clone()),
        bucket: v[1].clone(),
        access_key_id: Some(v[2].clone()),
        secret_access_key: Some(v[3].clone()),
        prefix: run_id(),
        ..Default::default()
    };
    let s = S3Storage::new(&cfg).unwrap();
    conformance::run(&s).await;

    // Folder objects made by other tools (`key/`) count as folders, and the
    // prefix confines the source.
    let outside = S3Storage::new(&S3Config {
        prefix: String::new(),
        ..cfg.clone()
    })
    .unwrap();
    let probe = VPath::parse(&format!("{}-sibling/secret", cfg.prefix)).unwrap();
    outside.mkdir(&probe.parent().unwrap()).await.unwrap();
    outside
        .write(&probe, Box::pin(&b"x"[..]), false)
        .await
        .unwrap();
    assert!(s.list(&VPath::root()).await.unwrap().is_empty());
    outside.delete(&probe.parent().unwrap()).await.unwrap();
}

#[cfg(feature = "sftp")]
#[tokio::test]
async fn sftp_conforms() {
    use filebrary_storage::sftp::{SftpConfig, SftpStorage};
    let Some(v) = require(
        "SFTP",
        &[
            "FILEBRARY_TEST_SFTP_ADDR",
            "FILEBRARY_TEST_SFTP_USER",
            "FILEBRARY_TEST_SFTP_PASSWORD",
            "FILEBRARY_TEST_SFTP_ROOT",
        ],
    ) else {
        return;
    };
    let (host, port) = v[0].rsplit_once(':').expect("host:port");
    let base = SftpConfig {
        host: host.into(),
        port: Some(port.parse().unwrap()),
        username: v[1].clone(),
        password: Some(v[2].clone()),
        root: v[3].clone(),
        insecure_accept_any_host_key: true,
        ..Default::default()
    };
    // Host-key pinning: a wrong fingerprint is refused.
    let wrong = SftpStorage::new(&SftpConfig {
        insecure_accept_any_host_key: false,
        host_key_fingerprint: Some("SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into()),
        ..base.clone()
    })
    .unwrap();
    let err = wrong.list(&VPath::root()).await.unwrap_err().to_string();
    assert!(err.contains("host key"), "{err}");

    let top = SftpStorage::new(&base).unwrap();
    let id = run_id();
    let dir = VPath::parse(&id).unwrap();
    top.mkdir(&dir).await.unwrap();
    let s = SftpStorage::new(&SftpConfig {
        root: format!("{}/{id}", v[3].trim_end_matches('/')),
        ..base.clone()
    })
    .unwrap();
    conformance::run(&s).await;
    symlink_escape(&top, &s, &id).await;
    top.delete(&dir).await.unwrap();
}

/// Symlinks that point outside the root are hidden and refused.
#[cfg(feature = "sftp")]
async fn symlink_escape(
    top: &filebrary_storage::sftp::SftpStorage,
    s: &filebrary_storage::sftp::SftpStorage,
    id: &str,
) {
    use filebrary_storage::StorageError;
    let outside = VPath::parse(&format!("{id}-outside")).unwrap();
    top.mkdir(&outside).await.unwrap();
    top.write(
        &outside.join("secret.txt").unwrap(),
        Box::pin(&b"secret"[..]),
        false,
    )
    .await
    .unwrap();
    s.mkdir(&VPath::parse("in").unwrap()).await.unwrap();
    s.write(
        &VPath::parse("in/ok.txt").unwrap(),
        Box::pin(&b"ok"[..]),
        false,
    )
    .await
    .unwrap();
    // Relative targets resolve next to the link on the server.
    s.create_test_symlink("evil_dir", &format!("../{id}-outside"))
        .await
        .unwrap();
    s.create_test_symlink("evil_file", &format!("../{id}-outside/secret.txt"))
        .await
        .unwrap();
    s.create_test_symlink("good_dir", "in").await.unwrap();

    let names: Vec<String> = s
        .list(&VPath::root())
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(names.contains(&"good_dir".to_owned()), "{names:?}");
    assert!(!names.contains(&"evil_dir".to_owned()), "{names:?}");
    assert!(!names.contains(&"evil_file".to_owned()), "{names:?}");
    for p in ["evil_dir/secret.txt", "evil_file"] {
        let p = VPath::parse(p).unwrap();
        assert!(matches!(
            s.read(&p, None).await,
            Err(StorageError::OutsideRoot)
        ));
        assert!(matches!(s.stat(&p).await, Err(StorageError::OutsideRoot)));
    }
    assert!(matches!(
        s.list(&VPath::parse("evil_dir").unwrap()).await,
        Err(StorageError::OutsideRoot)
    ));
    assert!(matches!(
        s.write(
            &VPath::parse("evil_dir/new").unwrap(),
            Box::pin(&b"x"[..]),
            false
        )
        .await,
        Err(StorageError::OutsideRoot)
    ));
    // A link inside the root works.
    let ok = conformance::read_all(s, "good_dir/ok.txt", None)
        .await
        .unwrap();
    assert_eq!(ok, b"ok");
    // Permission checks see where a path really lands.
    assert_eq!(
        s.resolve(&VPath::parse("good_dir/ok.txt").unwrap())
            .await
            .unwrap(),
        VPath::parse("in/ok.txt").unwrap()
    );
    assert!(matches!(
        s.resolve(&VPath::parse("evil_dir/x").unwrap()).await,
        Err(StorageError::OutsideRoot)
    ));
    // A scope cannot be entered through a link that leaves it.
    let scoped = s.sub(&VPath::root()).await.unwrap();
    assert!(matches!(
        scoped.stat(&VPath::parse("evil_dir").unwrap()).await,
        Err(StorageError::OutsideRoot)
    ));
    // Deleting a link removes the link, never the target.
    s.delete(&VPath::parse("evil_dir").unwrap()).await.unwrap();
    s.delete(&VPath::parse("evil_file").unwrap()).await.unwrap();
    assert!(top.stat(&outside.join("secret.txt").unwrap()).await.is_ok());
    top.delete(&outside).await.unwrap();
    s.delete(&VPath::parse("good_dir").unwrap()).await.unwrap();
    assert!(s.stat(&VPath::parse("in/ok.txt").unwrap()).await.is_ok());
    s.delete(&VPath::parse("in").unwrap()).await.unwrap();
}

#[cfg(feature = "smb")]
#[tokio::test]
async fn smb_conforms() {
    use filebrary_storage::smb::{SmbConfig, SmbStorage};
    let Some(v) = require(
        "SMB",
        &[
            "FILEBRARY_TEST_SMB_ADDR",
            "FILEBRARY_TEST_SMB_SHARE",
            "FILEBRARY_TEST_SMB_USER",
            "FILEBRARY_TEST_SMB_PASSWORD",
        ],
    ) else {
        return;
    };
    let (host, port) = v[0].rsplit_once(':').expect("host:port");
    let base = SmbConfig {
        host: host.into(),
        port: Some(port.parse().unwrap()),
        share: v[1].clone(),
        username: v[2].clone(),
        password: Some(v[3].clone()),
        ..Default::default()
    };
    let top = SmbStorage::new(&base).unwrap();
    let id = run_id();
    let dir = VPath::parse(&id).unwrap();
    top.mkdir(&dir).await.unwrap();
    let s = SmbStorage::new(&SmbConfig {
        root: id.clone(),
        ..base.clone()
    })
    .unwrap();
    conformance::run(&s).await;
    top.delete(&dir).await.unwrap();
}

/// With `FILEBRARY_TEST_SMB_LINKS` naming a folder on the share that holds
/// `real/f.txt`, `inside -> real`, `outside -> /etc` and `pw -> /etc/passwd`
/// (made by `scripts/test-backends.sh`), links that leave the share are not
/// served. Windows junctions and symlinks (reparse points) are hidden and
/// refused by the backend itself; Samba follows Unix links on its side.
#[cfg(feature = "smb")]
#[tokio::test]
async fn smb_links_do_not_escape() {
    use filebrary_storage::smb::{SmbConfig, SmbStorage};
    let (Some(dir), Some(addr)) = (
        env("FILEBRARY_TEST_SMB_LINKS"),
        env("FILEBRARY_TEST_SMB_ADDR"),
    ) else {
        eprintln!("skipping SMB link tests: FILEBRARY_TEST_SMB_LINKS not set");
        return;
    };
    let (host, port) = addr.rsplit_once(':').expect("host:port");
    let s = SmbStorage::new(&SmbConfig {
        host: host.into(),
        port: Some(port.parse().unwrap()),
        share: env("FILEBRARY_TEST_SMB_SHARE").unwrap(),
        username: env("FILEBRARY_TEST_SMB_USER").unwrap(),
        password: env("FILEBRARY_TEST_SMB_PASSWORD"),
        root: dir,
        ..Default::default()
    })
    .unwrap();
    let listed = s.list(&VPath::root()).await;
    eprintln!("listing: {listed:?}");
    for p in ["outside/passwd", "pw"] {
        let r = conformance::read_all(&s, p, None).await;
        eprintln!("{p}: {:?}", r.as_ref().map(|b| b.len()));
        assert!(r.is_err(), "{p} must not be readable");
    }
    // Samba resolves Unix symlinks itself and shows them as ordinary entries;
    // with `wide links = no` (its default) it refuses the ones that leave the
    // share, which is what the reads above check. Links inside the share work.
    let names: Vec<String> = listed.unwrap().into_iter().map(|e| e.name).collect();
    assert!(names.contains(&"real".to_owned()), "{names:?}");
    let inside = conformance::read_all(&s, "inside/f.txt", None)
        .await
        .unwrap();
    assert_eq!(inside, b"hi\n");
}
