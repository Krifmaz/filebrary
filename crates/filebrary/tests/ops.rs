//! Integration tests for operations features: readiness, metrics, request
//! ids, the OpenAPI document and docs page, notification admin endpoints,
//! upload events and the FTP server wired to real users.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    body::{Body, Bytes},
    http::{HeaderMap, Request, StatusCode, header},
    routing::post,
};
use filebrary::{
    AppState,
    auth::{Perms, session},
    config::{Config, SourceConfig, SourceKind},
    db,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

struct App {
    dir: tempfile::TempDir,
    state: AppState,
    app: Router,
}

async fn app_with(tweak: impl FnOnce(&mut Config)) -> App {
    let dir = tempfile::tempdir().unwrap();
    let files = dir.path().join("files");
    std::fs::create_dir_all(&files).unwrap();
    let mut config = Config {
        data_dir: dir.path().join("data"),
        sources: vec![SourceConfig {
            name: "files".into(),
            kind: SourceKind::Local,
            path: files,
        }],
        ..Config::default()
    };
    tweak(&mut config);
    config.validate().unwrap();
    std::fs::create_dir_all(&config.data_dir).unwrap();
    let pool = db::open_memory().await.unwrap();
    let state = AppState::new(config, pool, [7u8; 32]).unwrap();
    let app = filebrary::app(state.clone());
    App { dir, state, app }
}

impl App {
    async fn send(&self, req: Request<Body>) -> axum::response::Response {
        self.app.clone().oneshot(req).await.unwrap()
    }

    /// Creates a user with a real password and returns (cookie, csrf).
    async fn user(
        &self,
        name: &str,
        pass: &str,
        admin: bool,
        perms: Perms,
        scope: &str,
    ) -> (String, String) {
        let id = filebrary::auth::users::create(&self.state.db, name, pass, admin)
            .await
            .unwrap()
            .id;
        if !admin {
            let scope = filebrary::storage::VPath::parse(scope).unwrap();
            filebrary::auth::permissions::set(
                &self.state.db,
                "files",
                &scope,
                filebrary::auth::Subject::User(id),
                perms,
            )
            .await
            .unwrap();
            filebrary::auth::users::set_scope(&self.state.db, id, "files", &scope)
                .await
                .unwrap();
        }
        let s = session::create(&self.state, id, None, None).await.unwrap();
        (
            format!("{}={}", session::SESSION_COOKIE, s.token),
            s.csrf_token,
        )
    }
}

async fn body(resp: axum::response::Response) -> (StatusCode, HeaderMap, Bytes) {
    let status = resp.status();
    let headers = resp.headers().clone();
    (
        status,
        headers,
        resp.into_body().collect().await.unwrap().to_bytes(),
    )
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn readiness_reports_components() {
    let t = app_with(|_| {}).await;
    let (s, _, b) = body(t.send(get("/readyz")).await).await;
    assert_eq!(s, StatusCode::OK);
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["status"], "ok");
    assert_eq!(v["checks"]["database"], "ok");
    assert_eq!(v["checks"]["source:files"], "ok");

    // A source whose directory disappeared makes the instance not ready.
    std::fs::remove_dir_all(t.dir.path().join("files")).unwrap();
    let (s, _, b) = body(t.send(get("/readyz")).await).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["checks"]["source:files"], "fail");
    // Liveness is unaffected.
    assert_eq!(t.send(get("/healthz")).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn metrics_endpoint_and_token() {
    let t = app_with(|c| c.metrics.token = Some("0123456789abcdef".into())).await;
    t.send(get("/api/health")).await;
    t.send(get("/api/public/shares/secret-token-value")).await;
    assert_eq!(
        t.send(get("/metrics")).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let req = Request::get("/metrics")
        .header(header::AUTHORIZATION, "Bearer wrong")
        .body(Body::empty())
        .unwrap();
    assert_eq!(t.send(req).await.status(), StatusCode::UNAUTHORIZED);
    let req = Request::get("/metrics")
        .header(header::AUTHORIZATION, "Bearer 0123456789abcdef")
        .body(Body::empty())
        .unwrap();
    let (s, h, b) = body(t.send(req).await).await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        h[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/plain")
    );
    let text = String::from_utf8(b.to_vec()).unwrap();
    assert!(text.contains("filebrary_http_requests_total"), "{text}");
    assert!(text.contains("route=\"/api/health\""), "{text}");
    assert!(text.contains("filebrary_build_info"));
    assert!(text.contains("filebrary_uptime_seconds"));
    // Raw paths never become labels.
    assert!(!text.contains("secret-token-value"));
}

#[tokio::test]
async fn metrics_can_be_disabled() {
    let t = app_with(|c| c.metrics.enabled = false).await;
    assert_eq!(
        t.send(get("/metrics")).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn request_ids_are_echoed_or_generated() {
    let t = app_with(|_| {}).await;
    let req = Request::get("/healthz")
        .header("x-request-id", "trace-42")
        .body(Body::empty())
        .unwrap();
    assert_eq!(t.send(req).await.headers()["x-request-id"], "trace-42");
    let generated = t.send(get("/healthz")).await;
    let id = generated.headers()["x-request-id"].to_str().unwrap();
    assert_eq!(id.len(), 36, "uuid expected, got {id}");
    let req = Request::get("/healthz")
        .header("x-request-id", "x".repeat(200))
        .body(Body::empty())
        .unwrap();
    assert_eq!(t.send(req).await.headers()["x-request-id"].len(), 36);
}

#[tokio::test]
async fn openapi_and_docs_page() {
    let t = app_with(|_| {}).await;
    let (s, h, b) = body(t.send(get("/api/openapi.json")).await).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h[header::CONTENT_TYPE], "application/json");
    let doc: Value = serde_json::from_slice(&b).unwrap();
    assert!(doc["openapi"].as_str().unwrap().starts_with("3."));
    assert_eq!(doc["info"]["version"], env!("CARGO_PKG_VERSION"));

    let (s, h, b) = body(t.send(get("/api/docs")).await).await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        h[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let html = String::from_utf8(b.to_vec()).unwrap();
    // Scripts are external so the strict CSP applies.
    assert!(html.contains("src=\"/api/docs/docs.js\"") && !html.contains("<script>"));
    for asset in ["/api/docs/docs.js", "/api/docs/docs.css"] {
        assert_eq!(t.send(get(asset)).await.status(), StatusCode::OK, "{asset}");
    }
}

/// Every path in the document is routed: requesting it never falls through
/// to the "unknown API path" 404 (it may still be 401, 404 for a missing
/// item, and so on).
#[tokio::test]
async fn every_documented_path_is_routed() {
    let t = app_with(|_| {}).await;
    let doc = filebrary::ops::openapi::document();
    let mut checked = 0;
    for (path, item) in &doc.paths.paths {
        let concrete = path
            .replace("{id}", "files")
            .replace("{uid}", "00000000000000000000000000000000")
            .replace("{name}", "nope");
        let ops = [
            ("GET", item.get.is_some()),
            ("POST", item.post.is_some()),
            ("PATCH", item.patch.is_some()),
            ("DELETE", item.delete.is_some()),
            ("PUT", item.put.is_some()),
        ];
        for (method, present) in ops {
            if !present {
                continue;
            }
            let req = Request::builder()
                .method(method)
                .uri(&concrete)
                .body(Body::empty())
                .unwrap();
            let (s, _, b) = body(t.send(req).await).await;
            let unknown =
                s == StatusCode::METHOD_NOT_ALLOWED || (s == StatusCode::NOT_FOUND && b.is_empty());
            assert!(
                !unknown,
                "{method} {path} is documented but not routed ({s})"
            );
            checked += 1;
        }
    }
    assert!(checked >= 15, "only {checked} operations checked");
}

type Seen = Arc<Mutex<Vec<Value>>>;

async fn webhook_receiver() -> (String, Seen) {
    let seen: Seen = Arc::default();
    let s = seen.clone();
    let app = Router::new().route(
        "/hook",
        post(move |b: Bytes| {
            let s = s.clone();
            async move {
                s.lock().unwrap().push(serde_json::from_slice(&b).unwrap());
                StatusCode::OK
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("http://{addr}/hook"), seen)
}

async fn wait_for(seen: &Seen, n: usize) {
    for _ in 0..400 {
        if seen.lock().unwrap().len() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("expected {n} webhook calls, got {:?}", seen.lock().unwrap());
}

#[tokio::test]
async fn notification_admin_and_upload_event() {
    let (url, seen) = webhook_receiver().await;
    let t = app_with(|c| {
        c.notifications.instance = "test-box".into();
        c.notifications
            .webhooks
            .push(filebrary_events::WebhookConfig {
                name: "hook".into(),
                url,
                secret: Some("shh".into()),
                headers: Default::default(),
                events: vec![],
            });
    })
    .await;
    let (admin, csrf) = t.user("root", "pw-root-1", true, Perms::ALL, "/").await;
    let (user, user_csrf) = t.user("bob", "pw-bob-1", false, Perms::ALL, "/").await;

    // Listing: admins only, no secrets.
    let list = |c: &str| {
        Request::get("/api/admin/notifications")
            .header(header::COOKIE, c)
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(t.send(list(&user)).await.status(), StatusCode::FORBIDDEN);
    let (s, _, b) = body(t.send(list(&admin)).await).await;
    assert_eq!(s, StatusCode::OK);
    let text = String::from_utf8(b.to_vec()).unwrap();
    assert!(!text.contains("shh"));
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["targets"][0]["name"], "hook");
    assert_eq!(v["targets"][0]["kind"], "webhook");

    // Test send.
    let test = |c: &str, csrf: &str, name: &str| {
        Request::post(format!("/api/admin/notifications/{name}/test"))
            .header(header::COOKIE, c)
            .header("x-csrf-token", csrf)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"message": "ping"}).to_string()))
            .unwrap()
    };
    assert_eq!(
        t.send(test(&user, &user_csrf, "hook")).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        t.send(test(&admin, &csrf, "nope")).await.status(),
        StatusCode::NOT_FOUND
    );
    let (s, _, b) = body(t.send(test(&admin, &csrf, "hook")).await).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&b).unwrap()["delivered"],
        true
    );
    wait_for(&seen, 1).await;
    assert_eq!(seen.lock().unwrap()[0]["type"], "test");
    assert_eq!(seen.lock().unwrap()[0]["message"], "ping");
    assert_eq!(seen.lock().unwrap()[0]["instance"], "test-box");

    // A finished upload publishes an event.
    let req = Request::post("/api/sources/files/uploads")
        .header(header::COOKIE, &user)
        .header("x-csrf-token", &user_csrf)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"path": "/up/a.txt", "size": 5}).to_string(),
        ))
        .unwrap();
    let (s, _, b) = body(t.send(req).await).await;
    assert_eq!(s, StatusCode::OK, "{b:?}");
    let uid = serde_json::from_slice::<Value>(&b).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let req = Request::patch(format!("/api/uploads/{uid}"))
        .header(header::COOKIE, &user)
        .header("x-csrf-token", &user_csrf)
        .header("upload-offset", "0")
        .body(Body::from("hello"))
        .unwrap();
    assert_eq!(t.send(req).await.status(), StatusCode::OK);
    wait_for(&seen, 2).await;
    let e = seen.lock().unwrap()[1].clone();
    assert_eq!(e["type"], "upload");
    assert_eq!(e["user"], "bob");
    assert_eq!(e["source"], "files");
    assert_eq!(e["path"], "/up/a.txt");
    assert_eq!(e["size"], 5);
}

async fn fetch(
    c: &mut suppaftp::tokio::AsyncFtpStream,
    path: &str,
) -> Result<String, suppaftp::FtpError> {
    use tokio::io::AsyncReadExt;
    let mut s = c.retr_as_stream(path).await?;
    let mut got = String::new();
    let _ = s.read_to_string(&mut got).await;
    c.finalize_retr_stream(s).await?;
    Ok(got)
}

#[tokio::test]
async fn ftp_uses_real_users_and_scopes() {
    use suppaftp::tokio::AsyncFtpStream;

    let (url, seen) = webhook_receiver().await;
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let pasv = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let t = app_with(|c| {
        c.ftp.enabled = true;
        c.ftp.listen = SocketAddr::from(([127, 0, 0, 1], port));
        c.ftp.passive_ports = [pasv, pasv.saturating_add(20)];
        c.notifications
            .webhooks
            .push(filebrary_events::WebhookConfig {
                name: "hook".into(),
                url,
                secret: None,
                headers: Default::default(),
                events: vec![filebrary_events::EventKind::Upload],
            });
    })
    .await;
    std::fs::create_dir_all(t.dir.path().join("files/home/carol")).unwrap();
    std::fs::create_dir_all(t.dir.path().join("files/home/dave")).unwrap();
    std::fs::write(t.dir.path().join("files/home/dave/private.txt"), "dave").unwrap();
    t.user(
        "carol",
        "carol-pass",
        false,
        Perms::VIEW | Perms::DOWNLOAD | Perms::UPLOAD,
        "/home/carol",
    )
    .await;
    let (_, _) = t.user("eve", "eve-pass", false, Perms::ALL, "/").await;
    sqlx::query("UPDATE users SET disabled = 1 WHERE username = 'eve'")
        .execute(&t.state.db)
        .await
        .unwrap();
    filebrary::ops::ftp::spawn(&t.state, std::future::pending())
        .unwrap()
        .unwrap();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Wrong password, unknown user and disabled user are all refused.
    for (u, p) in [("carol", "nope"), ("mallory", "x"), ("eve", "eve-pass")] {
        let mut c = AsyncFtpStream::connect(addr).await.unwrap();
        assert!(c.login(u, p).await.is_err(), "{u} logged in");
    }

    let mut c = AsyncFtpStream::connect(addr).await.unwrap();
    c.login("carol", "carol-pass").await.unwrap();
    assert_eq!(c.nlst(Some("/")).await.unwrap(), ["files"]);
    let mut data: &[u8] = b"from ftp";
    c.put_file("/files/note.txt", &mut data).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(t.dir.path().join("files/home/carol/note.txt")).unwrap(),
        "from ftp"
    );
    assert_eq!(fetch(&mut c, "/files/note.txt").await.unwrap(), "from ftp");
    // No edit permission: cannot overwrite or rename; no delete permission.
    let mut again: &[u8] = b"x";
    assert!(c.put_file("/files/note.txt", &mut again).await.is_err());
    assert!(c.rename("/files/note.txt", "/files/n2.txt").await.is_err());
    assert!(c.rm("/files/note.txt").await.is_err());
    // Out of scope.
    for p in [
        "/files/../files/../../home/dave/private.txt",
        "/../home/dave/private.txt",
    ] {
        assert!(fetch(&mut c, p).await.is_err(), "{p}");
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            t.dir.path().join("files/home/dave"),
            t.dir.path().join("files/home/carol/dave"),
        )
        .unwrap();
        assert!(fetch(&mut c, "/files/dave/private.txt").await.is_err());
    }

    wait_for(&seen, 1).await;
    let e = seen.lock().unwrap()[0].clone();
    assert_eq!(e["type"], "upload");
    assert_eq!(e["user"], "carol");
    assert_eq!(e["path"], "/home/carol/note.txt");
}

#[tokio::test]
async fn ftp_deletes_go_to_the_trash_and_are_logged() {
    use suppaftp::tokio::AsyncFtpStream;

    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let pasv = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let t = app_with(|c| {
        c.ftp.enabled = true;
        c.ftp.listen = SocketAddr::from(([127, 0, 0, 1], port));
        c.ftp.passive_ports = [pasv, pasv.saturating_add(20)];
    })
    .await;
    t.user("root", "root-pass", true, Perms::ALL, "/").await;
    std::fs::write(t.dir.path().join("files/old.txt"), "old").unwrap();
    filebrary::ops::ftp::spawn(&t.state, std::future::pending())
        .unwrap()
        .unwrap();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut c = AsyncFtpStream::connect(addr).await.unwrap();
    c.login("root", "root-pass").await.unwrap();
    c.rm("/files/old.txt").await.unwrap();
    assert!(!t.dir.path().join("files/old.txt").exists());
    let trashed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trash")
        .fetch_one(&t.state.db)
        .await
        .unwrap();
    assert_eq!(trashed, 1);
    // The trash folder exists on disk but is never listed or reachable.
    assert!(t.dir.path().join("files/.filebrary-trash").exists());
    assert!(c.nlst(Some("/files")).await.unwrap().is_empty());
    assert!(c.cwd("/files/.filebrary-trash").await.is_err());
    c.mkdir("/files/new").await.unwrap();
    let actions: Vec<String> = sqlx::query_scalar("SELECT action FROM activity ORDER BY id")
        .fetch_all(&t.state.db)
        .await
        .unwrap();
    assert_eq!(actions, ["trash", "mkdir"]);
}

/// Hooks standing in for two-factor login and quotas.
struct StrictHooks;

impl filebrary::auth::hooks::AuthHooks for StrictHooks {
    fn after_password<'a>(
        &'a self,
        _state: &'a AppState,
        user: &'a filebrary::auth::users::UserRecord,
        totp: Option<&'a str>,
    ) -> filebrary::auth::hooks::BoxFuture<'a, filebrary::error::AppResult<()>> {
        Box::pin(async move {
            if user.username == "twofa" && totp.is_none() {
                return Err(filebrary::error::AppError::TotpRequired);
            }
            Ok(())
        })
    }

    fn before_write<'a>(
        &'a self,
        _state: &'a AppState,
        _user: &'a filebrary::auth::User,
        _source: &'a str,
        path: &'a filebrary::storage::VPath,
        _bytes: Option<u64>,
    ) -> filebrary::auth::hooks::BoxFuture<'a, filebrary::error::AppResult<()>> {
        Box::pin(async move {
            if path.to_string().ends_with("big.bin") {
                return Err(filebrary::error::AppError::PayloadTooLarge);
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn ftp_respects_two_factor_and_write_hooks() {
    use suppaftp::tokio::AsyncFtpStream;

    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let pasv = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let t = app_with(|c| {
        c.ftp.enabled = true;
        c.ftp.listen = SocketAddr::from(([127, 0, 0, 1], port));
        c.ftp.passive_ports = [pasv, pasv.saturating_add(20)];
    })
    .await;
    assert!(t.state.auth.set_hooks(Arc::new(StrictHooks)).is_ok());
    t.user("root", "root-pass", true, Perms::ALL, "/").await;
    t.user("twofa", "twofa-pass", true, Perms::ALL, "/").await;
    filebrary::ops::ftp::spawn(&t.state, std::future::pending())
        .unwrap()
        .unwrap();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // An account that needs a second factor cannot use FTP.
    let mut c = AsyncFtpStream::connect(addr).await.unwrap();
    assert!(c.login("twofa", "twofa-pass").await.is_err());

    // The write hook (quotas) refuses before any bytes land on disk.
    let mut c = AsyncFtpStream::connect(addr).await.unwrap();
    c.login("root", "root-pass").await.unwrap();
    c.put_file("/files/ok.bin", &mut &b"fine"[..])
        .await
        .unwrap();
    assert!(
        c.put_file("/files/big.bin", &mut &b"too much"[..])
            .await
            .is_err()
    );
    assert!(t.dir.path().join("files/ok.bin").exists());
    assert!(!t.dir.path().join("files/big.bin").exists());
}
