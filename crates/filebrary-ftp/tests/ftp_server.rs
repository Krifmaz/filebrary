//! End-to-end FTP tests: a real server on a temporary directory, driven by
//! a real FTP client.

use std::{
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use filebrary_ftp::{Change, Denied, FtpAccess, FtpAccount, FtpConfig, FtpServer, Op};
use filebrary_storage::{Storage, VPath, local::LocalStorage};
use suppaftp::tokio::AsyncFtpStream;
use tokio::io::AsyncReadExt;

/// One user, two sources. `alice` may do everything on `files` inside
/// `/home/alice`, and only view/download `media`.
struct Account {
    files: Arc<dyn Storage>,
    media: Arc<dyn Storage>,
    writes: Arc<Mutex<Vec<(String, String, u64)>>>,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Account")
    }
}

#[async_trait]
impl FtpAccount for Account {
    fn username(&self) -> &str {
        "alice"
    }
    fn sources(&self) -> Vec<String> {
        vec!["files".into(), "media".into()]
    }
    async fn resolve(
        &self,
        source: &str,
        op: Op,
        _path: &VPath,
    ) -> Result<Arc<dyn Storage>, Denied> {
        match source {
            "files" => Ok(self
                .files
                .sub(&VPath::parse("home/alice").unwrap())
                .await
                .unwrap()),
            "media" if matches!(op, Op::View | Op::Download) => Ok(self.media.clone()),
            "media" => Err(Denied::Forbidden),
            _ => Err(Denied::NotFound),
        }
    }
    fn hide(&self, _source: &str, dir: &VPath, name: &str) -> bool {
        dir.is_root() && name == ".hidden"
    }
    async fn after_change(&self, source: &str, change: Change<'_>) {
        if let Change::Upload { path, size, .. } = change {
            self.writes
                .lock()
                .unwrap()
                .push((source.into(), path.to_string(), size));
        }
    }
}

#[derive(Debug)]
struct Access(Arc<Account>);

#[async_trait]
impl FtpAccess for Access {
    async fn login(&self, u: &str, p: &str, _ip: IpAddr) -> Option<Arc<dyn FtpAccount>> {
        (u == "alice" && p == "secret").then(|| self.0.clone() as Arc<dyn FtpAccount>)
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    addr: SocketAddr,
    writes: Arc<Mutex<Vec<(String, String, u64)>>>,
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn start(max_login_attempts: u32) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join("files/home/alice")).unwrap();
    std::fs::create_dir_all(root.join("files/home/bob")).unwrap();
    std::fs::write(root.join("files/home/bob/secret.txt"), "bob's").unwrap();
    std::fs::create_dir_all(root.join("media")).unwrap();
    std::fs::write(root.join("media/song.txt"), "la la la").unwrap();
    let writes = Arc::new(Mutex::new(Vec::new()));
    let account = Arc::new(Account {
        files: Arc::new(LocalStorage::new(root.join("files")).unwrap()),
        media: Arc::new(LocalStorage::new(root.join("media")).unwrap()),
        writes: writes.clone(),
    });
    let port = free_port();
    let pasv = free_port();
    let cfg = FtpConfig {
        enabled: true,
        listen: SocketAddr::from(([127, 0, 0, 1], port)),
        passive_ports: [pasv, pasv.saturating_add(20)],
        max_login_attempts,
        ..FtpConfig::default()
    };
    let server = FtpServer::new(cfg, Arc::new(Access(account))).unwrap();
    tokio::spawn(server.run(std::future::pending()));
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Fixture {
        _dir: dir,
        root,
        addr,
        writes,
    }
}

async fn login(f: &Fixture) -> AsyncFtpStream {
    let mut c = AsyncFtpStream::connect(f.addr).await.unwrap();
    c.login("alice", "secret").await.unwrap();
    c
}

async fn get(c: &mut AsyncFtpStream, path: &str) -> Result<String, suppaftp::FtpError> {
    let mut s = c.retr_as_stream(path).await?;
    let mut buf = String::new();
    s.read_to_string(&mut buf).await.unwrap();
    c.finalize_retr_stream(s).await?;
    Ok(buf)
}

#[tokio::test]
async fn root_lists_sources() {
    let f = start(5).await;
    let mut c = login(&f).await;
    let mut names = c.nlst(None).await.unwrap();
    names.sort();
    assert_eq!(names, ["files", "media"]);
    c.cwd("/media").await.unwrap();
    assert_eq!(c.nlst(None).await.unwrap(), ["song.txt"]);
    assert_eq!(get(&mut c, "song.txt").await.unwrap(), "la la la");
    assert_eq!(c.size("/media/song.txt").await.unwrap(), 8);
}

#[tokio::test]
async fn upload_rename_delete_inside_scope() {
    let f = start(5).await;
    std::fs::create_dir_all(f.root.join("files/home/alice/.hidden")).unwrap();
    let mut c = login(&f).await;
    c.cwd("/files").await.unwrap();
    assert!(
        c.nlst(None).await.unwrap().is_empty(),
        "hidden entries are listed"
    );
    c.mkdir("docs").await.unwrap();
    let mut data: &[u8] = b"hello ftp";
    c.put_file("docs/a.txt", &mut data).await.unwrap();
    // The user's scope is applied: the file lands in home/alice.
    assert_eq!(
        std::fs::read_to_string(f.root.join("files/home/alice/docs/a.txt")).unwrap(),
        "hello ftp"
    );
    assert_eq!(
        *f.writes.lock().unwrap(),
        [("files".to_owned(), "/docs/a.txt".to_owned(), 9)]
    );
    c.rename("docs/a.txt", "docs/b.txt").await.unwrap();
    assert_eq!(get(&mut c, "/files/docs/b.txt").await.unwrap(), "hello ftp");
    // RMD refuses a folder that is not empty.
    assert!(c.rmdir("docs").await.is_err());
    c.rm("docs/b.txt").await.unwrap();
    c.rmdir("docs").await.unwrap();
    assert!(!f.root.join("files/home/alice/docs").exists());
}

#[tokio::test]
async fn resumed_upload_appends() {
    let f = start(5).await;
    let mut c = login(&f).await;
    let mut first: &[u8] = b"hello ";
    c.put_file("/files/r.txt", &mut first).await.unwrap();
    c.resume_transfer(6).await.unwrap();
    let mut rest: &[u8] = b"world";
    c.put_file("/files/r.txt", &mut rest).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(f.root.join("files/home/alice/r.txt")).unwrap(),
        "hello world"
    );
    c.resume_transfer(6).await.unwrap();
    assert_eq!(get(&mut c, "/files/r.txt").await.unwrap(), "world");
}

#[tokio::test]
async fn permissions_are_enforced() {
    let f = start(5).await;
    let mut c = login(&f).await;
    // media is read-only for alice.
    let mut data: &[u8] = b"x";
    assert!(c.put_file("/media/new.txt", &mut data).await.is_err());
    assert!(c.rm("/media/song.txt").await.is_err());
    assert!(c.mkdir("/media/d").await.is_err());
    assert!(f.root.join("media/song.txt").exists());
    // Nothing can be created at the virtual root or replace a source.
    assert!(c.mkdir("/newsource").await.is_err());
    assert!(c.rmdir("/files").await.is_err());
    // Moving between sources is refused.
    assert!(
        c.rename("/media/song.txt", "/files/song.txt")
            .await
            .is_err()
    );
    // Hidden sources do not exist.
    assert!(c.cwd("/other").await.is_err());
}

#[tokio::test]
async fn traversal_cannot_escape_scope() {
    let f = start(5).await;
    let mut c = login(&f).await;
    for p in [
        "/files/../files/home/bob/secret.txt",
        "/files/../../home/bob/secret.txt",
        "../home/bob/secret.txt",
        "/files/..%2Fbob/secret.txt",
        "/files/..\\..\\bob\\secret.txt",
        "/../../../../etc/passwd",
    ] {
        assert!(get(&mut c, p).await.is_err(), "read {p}");
    }
    c.cwd("/files").await.unwrap();
    // `..` past the source only reaches the virtual root.
    c.cwd("../../..").await.unwrap();
    let mut names = c.nlst(None).await.unwrap();
    names.sort();
    assert_eq!(names, ["files", "media"]);
}

#[tokio::test]
async fn symlinks_out_of_the_source_are_refused() {
    let f = start(5).await;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(f.root.join("media"), f.root.join("files/home/alice/link"))
            .unwrap();
        // A link to another user's folder in the same source is refused too.
        std::os::unix::fs::symlink(
            f.root.join("files/home/bob"),
            f.root.join("files/home/alice/bob"),
        )
        .unwrap();
        let mut c = login(&f).await;
        assert!(get(&mut c, "/files/link/song.txt").await.is_err());
        assert!(get(&mut c, "/files/bob/secret.txt").await.is_err());
        let mut data: &[u8] = b"x";
        assert!(
            c.put_file("/files/bob/planted.txt", &mut data)
                .await
                .is_err()
        );
        assert!(!f.root.join("files/home/bob/planted.txt").exists());
        assert!(
            c.nlst(Some("/files/link"))
                .await
                .map_or(true, |v| v.is_empty())
        );
    }
    #[cfg(not(unix))]
    let _ = f;
}

#[tokio::test]
async fn bad_passwords_are_rejected_and_rate_limited() {
    let f = start(2).await;
    for _ in 0..2 {
        let mut c = AsyncFtpStream::connect(f.addr).await.unwrap();
        assert!(c.login("alice", "wrong").await.is_err());
    }
    // Blocked now, even with the right password.
    let mut c = AsyncFtpStream::connect(f.addr).await.unwrap();
    assert!(c.login("alice", "secret").await.is_err());
    let mut c = AsyncFtpStream::connect(f.addr).await.unwrap();
    assert!(c.login("anonymous", "").await.is_err());
}

#[tokio::test]
async fn ftps_required_mode() {
    use filebrary_ftp::{FtpsConfig, FtpsMode};
    use suppaftp::tokio::{AsyncRustlsConnector, AsyncRustlsFtpStream};
    use tokio_rustls::rustls::{self, pki_types::CertificateDer};

    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("files/home/alice")).unwrap();
    std::fs::create_dir_all(dir.path().join("media")).unwrap();
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, cert.cert.pem()).unwrap();
    std::fs::write(&key_path, cert.signing_key.serialize_pem()).unwrap();

    let account = Arc::new(Account {
        files: Arc::new(LocalStorage::new(dir.path().join("files")).unwrap()),
        media: Arc::new(LocalStorage::new(dir.path().join("media")).unwrap()),
        writes: Arc::default(),
    });
    let port = free_port();
    let pasv = free_port();
    let cfg = FtpConfig {
        enabled: true,
        listen: SocketAddr::from(([127, 0, 0, 1], port)),
        passive_ports: [pasv, pasv.saturating_add(20)],
        tls: Some(FtpsConfig {
            cert: cert_path,
            key: key_path,
            mode: FtpsMode::Required,
        }),
        ..FtpConfig::default()
    };
    tokio::spawn(
        FtpServer::new(cfg, Arc::new(Access(account)))
            .unwrap()
            .run(std::future::pending()),
    );
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Plain-text login is refused.
    let mut plain = AsyncFtpStream::connect(addr).await.unwrap();
    assert!(plain.login("alice", "secret").await.is_err());

    // After AUTH TLS everything works.
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(cert.cert.der().to_vec()))
        .unwrap();
    let client = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = AsyncRustlsConnector::from(tokio_rustls::TlsConnector::from(Arc::new(client)));
    let mut c = AsyncRustlsFtpStream::connect(addr)
        .await
        .unwrap()
        .into_secure(connector, "localhost")
        .await
        .unwrap();
    c.login("alice", "secret").await.unwrap();
    let mut data: &[u8] = b"over tls";
    c.put_file("/files/t.txt", &mut data).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("files/home/alice/t.txt")).unwrap(),
        "over tls"
    );
}
