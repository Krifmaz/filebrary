//! WebDAV through the full application: real users, argon2 passwords, grants,
//! scopes and login rate limiting.

mod common;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use base64::Engine as _;
use common::{TestApp, body_bytes};
use filebrary::{
    auth::{Perms, Subject, permissions, users},
    storage::VPath,
};

async fn add_user(
    app: &TestApp,
    name: &str,
    pw: &str,
    admin: bool,
    perms: Perms,
    scope: &str,
) -> i64 {
    let rec = users::create(&app.state.db, name, pw, admin).await.unwrap();
    if !admin {
        let scope = VPath::parse(scope).unwrap();
        permissions::set(&app.state.db, "files", &scope, Subject::User(rec.id), perms)
            .await
            .unwrap();
        users::set_scope(&app.state.db, rec.id, "files", &scope)
            .await
            .unwrap();
    }
    rec.id
}

fn dav(method: &str, uri: &str, user: &str, pw: &str, body: &str) -> Request<Body> {
    let token = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pw}"));
    let mut r = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Basic {token}"))
        .header("depth", "1")
        .body(Body::from(body.to_owned()))
        .unwrap();
    r.extensions_mut()
        .insert(ConnectInfo(std::net::SocketAddr::from((
            [10, 0, 0, 1],
            5555,
        ))));
    r
}

#[tokio::test]
async fn webdav_uses_real_users_and_grants() {
    let app = TestApp::new().await;
    add_user(&app, "admin", "s3cret-password", true, Perms::ALL, "/").await;
    add_user(
        &app,
        "ro",
        "read-only-password",
        false,
        Perms::VIEW | Perms::DOWNLOAD,
        "/",
    )
    .await;
    add_user(&app, "jail", "jailed-password", false, Perms::ALL, "/jail").await;
    std::fs::create_dir(app.root().join("jail")).unwrap();

    // Admin writes a file.
    let r = app
        .send(dav(
            "PUT",
            "/dav/files/hello.txt",
            "admin",
            "s3cret-password",
            "hi",
        ))
        .await;
    assert_eq!(r.status(), StatusCode::CREATED);
    assert_eq!(
        std::fs::read_to_string(app.root().join("hello.txt")).unwrap(),
        "hi"
    );

    // The second request hits the credential cache and still works.
    let r = app
        .send(dav(
            "GET",
            "/dav/files/hello.txt",
            "admin",
            "s3cret-password",
            "",
        ))
        .await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(body_bytes(r).await, "hi");

    // Read-only user: may read, may not write.
    let r = app
        .send(dav(
            "GET",
            "/dav/files/hello.txt",
            "ro",
            "read-only-password",
            "",
        ))
        .await;
    assert_eq!(r.status(), StatusCode::OK);
    let r = app
        .send(dav("PUT", "/dav/files/x", "ro", "read-only-password", "x"))
        .await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    // Scoped user sees only their folder.
    let r = app
        .send(dav(
            "PUT",
            "/dav/files/mine",
            "jail",
            "jailed-password",
            "m",
        ))
        .await;
    assert_eq!(r.status(), StatusCode::CREATED);
    assert!(app.root().join("jail/mine").exists());
    let r = app
        .send(dav(
            "GET",
            "/dav/files/hello.txt",
            "jail",
            "jailed-password",
            "",
        ))
        .await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);

    // The mount root lists the source.
    let r = app
        .send(dav("PROPFIND", "/dav/", "ro", "read-only-password", ""))
        .await;
    assert_eq!(r.status(), StatusCode::MULTI_STATUS);
    let body = String::from_utf8(body_bytes(r).await.to_vec()).unwrap();
    assert!(body.contains("<D:href>/dav/files/</D:href>"));

    // A disabled user is refused even with cached credentials.
    sqlx::query("UPDATE users SET disabled = 1 WHERE username = 'admin'")
        .execute(&app.state.db)
        .await
        .unwrap();
    let r = app
        .send(dav(
            "GET",
            "/dav/files/hello.txt",
            "admin",
            "s3cret-password",
            "",
        ))
        .await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn webdav_never_accepts_the_session_cookie() {
    let app = TestApp::new().await;
    let client = app.user("cookie", true, Perms::ALL, "/").await;
    let r = app
        .send(
            client
                .req("DELETE", "/dav/files/anything")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    assert!(
        r.headers()["www-authenticate"]
            .to_str()
            .unwrap()
            .starts_with("Basic")
    );
}

#[tokio::test]
async fn webdav_logins_are_rate_limited() {
    let app = TestApp::new().await;
    add_user(&app, "u", "right-password", true, Perms::ALL, "/").await;
    let max = app.state.config.login_max_attempts;
    for _ in 0..max {
        let r = app
            .send(dav("PROPFIND", "/dav/", "u", "wrong-password", ""))
            .await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }
    // Even the right password is refused until the window passes.
    let r = app
        .send(dav("PROPFIND", "/dav/", "u", "right-password", ""))
        .await;
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(r.headers().contains_key("retry-after"));
    // OPTIONS still answers, unauthenticated.
    let r = app
        .send(dav("OPTIONS", "/dav/", "u", "wrong-password", ""))
        .await;
    assert_eq!(r.status(), StatusCode::OK);
}

#[tokio::test]
async fn webdav_can_be_turned_off() {
    let dir = tempfile::tempdir().unwrap();
    let config = filebrary::config::Config {
        data_dir: dir.path().join("data"),
        webdav: false,
        sources: vec![filebrary::config::SourceConfig {
            name: "files".into(),
            kind: filebrary::config::SourceKind::Local,
            path: dir.path().join("files"),
            ..Default::default()
        }],
        ..Default::default()
    };
    std::fs::create_dir_all(&config.data_dir).unwrap();
    let pool = filebrary::db::open_memory().await.unwrap();
    let state = filebrary::AppState::new(config, pool, [1; 32]).unwrap();
    let app = filebrary::app(state);
    use tower::ServiceExt;
    let r = app
        .oneshot(dav("PROPFIND", "/dav/", "u", "p", ""))
        .await
        .unwrap();
    assert_ne!(r.status(), StatusCode::MULTI_STATUS);
    assert_ne!(r.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn webdav_deletes_go_to_the_trash_and_are_logged() {
    let app = TestApp::new().await;
    add_user(&app, "admin", "admin-password", true, Perms::ALL, "/").await;
    let send = |m: &'static str, p: &'static str, body: &'static str| {
        app.send(dav(m, p, "admin", "admin-password", body))
    };
    assert_eq!(
        send("PUT", "/dav/files/doc.txt", "x").await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        send("DELETE", "/dav/files/doc.txt", "").await.status(),
        StatusCode::NO_CONTENT
    );
    assert!(!app.root().join("doc.txt").exists());
    // It is in the trash, and the trash folder is not visible over WebDAV.
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trash WHERE original_path = '/doc.txt'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(n, 1);
    let r = send("PROPFIND", "/dav/files/", "").await;
    let body = String::from_utf8(body_bytes(r).await.to_vec()).unwrap();
    assert!(!body.contains(".filebrary-trash"), "{body}");
    assert_eq!(
        send("PROPFIND", "/dav/files/.filebrary-trash/", "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    // Both changes are in the activity log, with the client address.
    let rows: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT action, path, ip FROM activity ORDER BY id")
            .fetch_all(&app.state.db)
            .await
            .unwrap();
    let actions: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(actions, ["upload", "trash"]);
    assert_eq!(rows[1].1, "/doc.txt");
    assert_eq!(rows[1].2.as_deref(), Some("10.0.0.1"));
}

#[tokio::test]
async fn webdav_applies_per_folder_rules() {
    let app = TestApp::new().await;
    let id = add_user(&app, "u", "user-password", false, Perms::ALL, "/").await;
    std::fs::create_dir(app.root().join("secret")).unwrap();
    std::fs::write(app.root().join("secret/plan.txt"), "plan").unwrap();
    std::fs::create_dir(app.root().join("open")).unwrap();
    // A deeper rule takes everything away in /secret.
    permissions::set(
        &app.state.db,
        "files",
        &VPath::parse("secret").unwrap(),
        Subject::User(id),
        Perms::NONE,
    )
    .await
    .unwrap();
    let send = |m: &'static str, p: &'static str| app.send(dav(m, p, "u", "user-password", ""));
    assert!(
        !send("GET", "/dav/files/secret/plan.txt")
            .await
            .status()
            .is_success()
    );
    assert!(
        !send("PUT", "/dav/files/secret/new.txt")
            .await
            .status()
            .is_success()
    );
    // Deleting the parent would delete what the rule protects.
    assert_eq!(
        send("DELETE", "/dav/files/").await.status(),
        StatusCode::FORBIDDEN
    );
    assert!(app.root().join("secret/plan.txt").exists());
    #[cfg(unix)]
    {
        // A link in an open folder cannot lead into the denied one.
        std::os::unix::fs::symlink(app.root().join("secret"), app.root().join("open/link"))
            .unwrap();
        let r = send("GET", "/dav/files/open/link/plan.txt").await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn webdav_enforces_and_reports_storage_quotas() {
    let app = TestApp::new().await;
    assert!(filebrary::identity::install(&app.state));
    let id = add_user(&app, "q", "quota-password", false, Perms::ALL, "/").await;
    filebrary_identity::quota::set_limit(&app.state.db, id, Some(10))
        .await
        .unwrap();
    let send = |m: &'static str, p: &'static str, body: &'static str| {
        app.send(dav(m, p, "q", "quota-password", body))
    };
    assert_eq!(
        send("PUT", "/dav/files/small.txt", "1234").await.status(),
        StatusCode::CREATED
    );
    // 4 of 10 bytes used; 20 more do not fit.
    let r = send("PUT", "/dav/files/big.txt", "01234567890123456789").await;
    assert_eq!(r.status(), StatusCode::INSUFFICIENT_STORAGE);
    assert!(!app.root().join("big.txt").exists());
    // RFC 4331 properties show the limit.
    let r = send(
        "PROPFIND",
        "/dav/files/",
        r#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:prop><D:quota-available-bytes/><D:quota-used-bytes/></D:prop></D:propfind>"#,
    )
    .await;
    assert_eq!(r.status(), StatusCode::MULTI_STATUS);
    let body = String::from_utf8(body_bytes(r).await.to_vec()).unwrap();
    assert!(
        body.contains("<D:quota-available-bytes>6</D:quota-available-bytes>"),
        "{body}"
    );
}
