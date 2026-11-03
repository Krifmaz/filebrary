//! Integration tests for share links, driving the full router in-process
//! against a temporary directory and an in-memory database.

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use filebrary::{
    AppState, app,
    auth::{Perms, session},
    config::{Config, SourceConfig},
    db,
    storage::VPath,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

struct Env {
    _dir: TempDir,
    root: std::path::PathBuf,
    state: AppState,
    app: Router,
}

struct Login {
    cookie: String,
    csrf: String,
}

async fn setup() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("files");
    std::fs::create_dir_all(root.join("docs/sub")).unwrap();
    std::fs::create_dir_all(root.join("drop")).unwrap();
    std::fs::create_dir_all(root.join("private")).unwrap();
    std::fs::write(root.join("docs/a.txt"), b"hello world").unwrap();
    std::fs::write(root.join("docs/sub/b.txt"), b"bbb").unwrap();
    std::fs::write(root.join("photo.jpg"), b"jpegdata").unwrap();
    std::fs::write(root.join("private/secret.txt"), b"top secret").unwrap();

    let config = Config {
        data_dir: dir.path().join("data"),
        secure_cookies: Some(false),
        max_drop_upload_mib: 1,
        login_max_attempts: 3,
        sources: vec![SourceConfig::local("files", root.clone())],
        ..Config::default()
    };
    let pool = db::open_memory().await.unwrap();
    let state = AppState::new(config, pool, [7u8; 32]).unwrap();
    let app = app(state.clone());
    Env {
        _dir: dir,
        root,
        state,
        app,
    }
}

// Everything that touches the auth schema is in `user`, `rule`, `set_perms`
// and `disable`, so a schema change only needs those updated.
impl Env {
    /// Sets the permission rule of user `id` on a real folder of "files".
    /// An empty set hides the folder.
    async fn rule(&self, id: i64, path: &str, perms: Perms) {
        filebrary::auth::permissions::set(
            &self.state.db,
            "files",
            &VPath::parse(path).unwrap(),
            filebrary::auth::Subject::User(id),
            perms,
        )
        .await
        .unwrap();
    }

    /// Replaces a non-admin user's permissions at their scope.
    async fn set_perms(&self, id: i64, perms: Perms) {
        let scope = filebrary::auth::users::scopes(&self.state.db, id)
            .await
            .unwrap()
            .remove("files")
            .unwrap_or_default();
        self.rule(id, &scope.to_string(), perms).await;
    }

    /// Disables a user account.
    async fn disable(&self, id: i64) {
        sqlx::query("UPDATE users SET disabled = 1 WHERE id = ?")
            .bind(id)
            .execute(&self.state.db)
            .await
            .unwrap();
    }

    /// Adds a user. Non-admins get `perms` on `scope`, which also confines them.
    async fn user(&self, name: &str, admin: bool, perms: Perms, scope: &str) -> (i64, Login) {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at) \
             VALUES (?, 'x', ?, 0, 0) RETURNING id",
        )
        .bind(name)
        .bind(admin)
        .fetch_one(&self.state.db)
        .await
        .unwrap();
        if !admin {
            self.rule(id, scope, perms).await;
            filebrary::auth::users::set_scope(
                &self.state.db,
                id,
                "files",
                &VPath::parse(scope).unwrap(),
            )
            .await
            .unwrap();
        }
        let s = session::create(&self.state, id, None, None).await.unwrap();
        (
            id,
            Login {
                cookie: format!("{}={}", session::SESSION_COOKIE, s.token),
                csrf: s.csrf_token,
            },
        )
    }

    async fn send(&self, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, headers, body)
    }

    async fn api(
        &self,
        login: &Login,
        method: Method,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::COOKIE, &login.cookie)
            .header("x-csrf-token", &login.csrf);
        let body = match body {
            Some(v) => {
                b = b.header(header::CONTENT_TYPE, "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let (s, _, bytes) = self.send(b.body(body).unwrap()).await;
        (s, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn get(&self, uri: &str) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        self.send(Request::get(uri).body(Body::empty()).unwrap())
            .await
    }

    async fn get_with(
        &self,
        uri: &str,
        h: (&str, &str),
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        self.send(
            Request::get(uri)
                .header(h.0, h.1)
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    async fn create(&self, login: &Login, body: Value) -> Value {
        let (s, v) = self
            .api(login, Method::POST, "/api/shares", Some(body))
            .await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        v
    }
}

fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

#[tokio::test]
async fn folder_share_browse_and_download() {
    let env = setup().await;
    let (_, admin) = env.user("alice", true, Perms::ALL, "/").await;
    let share = env
        .create(&admin, json!({"source": "files", "paths": ["/docs"]}))
        .await;
    let token = share["token"].as_str().unwrap();
    assert_eq!(share["url"], format!("/s/{token}"));
    assert_eq!(share["paths"], json!(["/docs"]));
    assert_eq!(share["has_password"], false);

    let (s, _, b) = env.get(&format!("/api/public/shares/{token}")).await;
    assert_eq!(s, StatusCode::OK);
    let v = json(&b);
    assert_eq!(v["name"], "docs");
    assert_eq!(v["is_dir"], true);
    let names: Vec<_> = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].clone())
        .collect();
    assert_eq!(names, vec![json!("sub"), json!("a.txt")]);

    let (s, _, b) = env
        .get(&format!("/api/public/shares/{token}?path=sub"))
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(json(&b)["entries"][0]["path"], "sub/b.txt");

    let (s, h, b) = env
        .get(&format!("/api/public/shares/{token}/download?path=a.txt"))
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b, b"hello world");
    assert!(
        h[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("attachment")
    );

    // Range request.
    let (s, h, b) = env
        .get_with(
            &format!("/api/public/shares/{token}/download?path=a.txt"),
            ("range", "bytes=6-"),
        )
        .await;
    assert_eq!(s, StatusCode::PARTIAL_CONTENT);
    assert_eq!(b, b"world");
    assert_eq!(h[header::CONTENT_RANGE], "bytes 6-10/11");

    // Whole folder as zip.
    let (s, h, b) = env
        .get(&format!("/api/public/shares/{token}/download"))
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h[header::CONTENT_TYPE], "application/zip");
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(b)).unwrap();
    let mut names: Vec<_> = zip.file_names().map(|n| n.unwrap().into_owned()).collect();
    names.sort();
    // The folder's contents sit at the top of the archive.
    assert_eq!(names, ["a.txt", "sub/", "sub/b.txt"]);
    let mut f = zip.by_name("sub/b.txt").unwrap();
    let mut out = String::new();
    std::io::Read::read_to_string(&mut f, &mut out).unwrap();
    assert_eq!(out, "bbb");
}

#[tokio::test]
async fn traversal_is_refused() {
    let env = setup().await;
    let (_, admin) = env.user("alice", true, Perms::ALL, "/").await;
    let share = env
        .create(&admin, json!({"source": "files", "paths": ["/docs"]}))
        .await;
    let token = share["token"].as_str().unwrap();
    for p in [
        "..",
        "../private/secret.txt",
        "sub/../../private",
        "..%2fprivate",
        "%2e%2e/private/secret.txt",
        "..\\private",
    ] {
        let (s, _, b) = env
            .get(&format!("/api/public/shares/{token}/download?path={p}"))
            .await;
        assert!(s.is_client_error(), "{p}: {s}");
        assert!(!String::from_utf8_lossy(&b).contains("top secret"), "{p}");
    }
    // A symlink inside the shared folder pointing outside the source is refused.
    #[cfg(unix)]
    {
        let outside = env._dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), b"top secret").unwrap();
        std::os::unix::fs::symlink(&outside, env.root.join("docs/link")).unwrap();
        let (s, _, b) = env
            .get(&format!(
                "/api/public/shares/{token}/download?path=link/secret.txt"
            ))
            .await;
        assert!(s.is_client_error(), "{s}");
        assert!(!String::from_utf8_lossy(&b).contains("top secret"));
        let (_, _, b) = env
            .get(&format!("/api/public/shares/{token}/download"))
            .await;
        assert!(
            !b.windows(10).any(|w| w == b"top secret"),
            "zip must skip escaping links"
        );

        // So is a symlink to another folder of the same source: a share
        // exposes its folder and nothing else.
        std::os::unix::fs::symlink(env.root.join("private"), env.root.join("docs/peek")).unwrap();
        let (s, _, b) = env
            .get(&format!(
                "/api/public/shares/{token}/download?path=peek/secret.txt"
            ))
            .await;
        assert!(s.is_client_error(), "{s}");
        assert!(!String::from_utf8_lossy(&b).contains("top secret"));
        let (_, _, b) = env
            .get(&format!("/api/public/shares/{token}/download"))
            .await;
        assert!(!b.windows(10).any(|w| w == b"top secret"));
    }
}

#[tokio::test]
async fn single_file_share() {
    let env = setup().await;
    let (_, admin) = env.user("alice", true, Perms::ALL, "/").await;
    let share = env
        .create(&admin, json!({"source": "files", "paths": ["photo.jpg"]}))
        .await;
    let token = share["token"].as_str().unwrap();
    let (s, _, b) = env.get(&format!("/api/public/shares/{token}")).await;
    assert_eq!(s, StatusCode::OK);
    let v = json(&b);
    assert_eq!(v["name"], "photo.jpg");
    assert_eq!(v["is_dir"], false);
    assert_eq!(v["size"], 8);
    let (s, h, b) = env
        .get(&format!("/api/public/shares/{token}/download?inline=true"))
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b, b"jpegdata");
    assert!(
        h[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("inline")
    );
}

#[tokio::test]
async fn multi_item_share() {
    let env = setup().await;
    let (_, admin) = env.user("alice", true, Perms::ALL, "/").await;
    let share = env
        .create(
            &admin,
            json!({"source": "files", "paths": ["/docs/a.txt", "/photo.jpg"]}),
        )
        .await;
    let token = share["token"].as_str().unwrap();
    let (_, _, b) = env.get(&format!("/api/public/shares/{token}")).await;
    let v = json(&b);
    let names: Vec<_> = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].clone())
        .collect();
    assert_eq!(names, vec![json!("a.txt"), json!("photo.jpg")]);
    let (s, _, b) = env
        .get(&format!(
            "/api/public/shares/{token}/download?path=photo.jpg"
        ))
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b, b"jpegdata");
    let (s, _, _) = env
        .get(&format!(
            "/api/public/shares/{token}/download?path=secret.txt"
        ))
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _, b) = env
        .get(&format!("/api/public/shares/{token}/download"))
        .await;
    assert_eq!(s, StatusCode::OK);
    let zip = zip::ZipArchive::new(std::io::Cursor::new(b)).unwrap();
    assert_eq!(zip.len(), 2);

    // Duplicate names are rejected.
    let (s, _) = env
        .api(
            &admin,
            Method::POST,
            "/api/shares",
            Some(json!({"source": "files", "paths": ["/docs/a.txt", "/a.txt"]})),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "missing second path");
    std::fs::write(env.root.join("a.txt"), b"x").unwrap();
    let (s, _) = env
        .api(
            &admin,
            Method::POST,
            "/api/shares",
            Some(json!({"source": "files", "paths": ["/docs/a.txt", "/a.txt"]})),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn password_protection() {
    let env = setup().await;
    let (_, admin) = env.user("alice", true, Perms::ALL, "/").await;
    let share = env
        .create(
            &admin,
            json!({"source": "files", "paths": ["/docs"], "password": "hunter2"}),
        )
        .await;
    assert_eq!(share["has_password"], true);
    let token = share["token"].as_str().unwrap();
    let id = share["id"].as_i64().unwrap();

    let (s, _, b) = env.get(&format!("/api/public/shares/{token}")).await;
    assert_eq!(s, StatusCode::OK);
    let v = json(&b);
    assert_eq!(v["needs_password"], true);
    assert_eq!(v["name"], "");
    assert!(v.get("entries").is_none());

    let (s, _, b) = env
        .get(&format!("/api/public/shares/{token}/download?path=a.txt"))
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(json(&b)["error"], "password_required");

    let unlock = |pw: &str| {
        Request::post(format!("/api/public/shares/{token}/unlock"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"password": pw}).to_string()))
            .unwrap()
    };
    let (s, _, b) = env.send(unlock("wrong")).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(json(&b)["error"], "wrong_password");

    let (s, h, b) = env.send(unlock("hunter2")).await;
    assert_eq!(s, StatusCode::OK);
    let access = json(&b)["access"].as_str().unwrap().to_owned();
    let set_cookie = h[header::SET_COOKIE].to_str().unwrap();
    assert!(set_cookie.starts_with(&format!("fb_share_{token}=")));
    assert!(set_cookie.contains("HttpOnly"));

    let cookie = set_cookie.split(';').next().unwrap();
    let (s, _, b) = env
        .get_with(
            &format!("/api/public/shares/{token}/download?path=a.txt"),
            ("cookie", cookie),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b, b"hello world");
    let (s, _, _) = env
        .get(&format!(
            "/api/public/shares/{token}/download?path=a.txt&access={access}"
        ))
        .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = env
        .get_with(
            &format!("/api/public/shares/{token}"),
            ("x-share-access", &access),
        )
        .await;
    assert_eq!(s, StatusCode::OK);

    // Changing the password revokes existing grants.
    let (s, _) = env
        .api(
            &admin,
            Method::PATCH,
            &format!("/api/shares/{id}"),
            Some(json!({"password": "new-one"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = env
        .get(&format!(
            "/api/public/shares/{token}/download?path=a.txt&access={access}"
        ))
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Removing it opens the share.
    let (s, v) = env
        .api(
            &admin,
            Method::PATCH,
            &format!("/api/shares/{id}"),
            Some(json!({"password": null})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["has_password"], false);
    let (s, _, _) = env
        .get(&format!("/api/public/shares/{token}/download?path=a.txt"))
        .await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn wrong_passwords_are_rate_limited() {
    let env = setup().await;
    let (_, admin) = env.user("alice", true, Perms::ALL, "/").await;
    let share = env
        .create(
            &admin,
            json!({"source": "files", "paths": ["/docs"], "password": "pw"}),
        )
        .await;
    let token = share["token"].as_str().unwrap();
    let try_pw = |pw: &str| {
        Request::post(format!("/api/public/shares/{token}/unlock"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"password": pw}).to_string()))
            .unwrap()
    };
    for _ in 0..3 {
        assert_eq!(env.send(try_pw("nope")).await.0, StatusCode::UNAUTHORIZED);
    }
    let (s, h, _) = env.send(try_pw("pw")).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert!(h.contains_key(header::RETRY_AFTER));
}

#[tokio::test]
async fn expiry_and_download_limit() {
    let env = setup().await;
    let (_, admin) = env.user("alice", true, Perms::ALL, "/").await;

    let (s, _) = env
        .api(&admin, Method::POST, "/api/shares", Some(json!({"source": "files", "paths": ["photo.jpg"], "expires_at": "2001-01-01T00:00:00Z"})))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "past expiry");

    let share = env
        .create(&admin, json!({"source": "files", "paths": ["photo.jpg"], "expires_at": "2099-01-01T00:00:00Z", "max_downloads": 2}))
        .await;
    assert_eq!(share["expires_at"], "2099-01-01T00:00:00Z");
    let token = share["token"].as_str().unwrap();
    let id = share["id"].as_i64().unwrap();
    let url = format!("/api/public/shares/{token}/download");
    assert_eq!(env.get(&url).await.0, StatusCode::OK);
    // Range requests count too when a limit is set.
    assert_eq!(
        env.get_with(&url, ("range", "bytes=4-")).await.0,
        StatusCode::PARTIAL_CONTENT
    );
    let (s, _, b) = env.get(&url).await;
    assert_eq!(s, StatusCode::GONE);
    assert_eq!(json(&b)["error"], "download_limit_reached");

    let (_, v) = env
        .api(&admin, Method::GET, &format!("/api/shares/{id}"), None)
        .await;
    assert_eq!(v["downloads"], 2);
    assert_eq!(v["expired"], true);

    // Raising the limit revives it.
    env.api(
        &admin,
        Method::PATCH,
        &format!("/api/shares/{id}"),
        Some(json!({"max_downloads": null})),
    )
    .await;
    assert_eq!(env.get(&url).await.0, StatusCode::OK);

    // Expired shares are indistinguishable from unknown ones.
    sqlx::query("UPDATE shares SET expires_at = 1 WHERE id = ?")
        .bind(id)
        .execute(&env.state.db)
        .await
        .unwrap();
    assert_eq!(env.get(&url).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        env.get(&format!("/api/public/shares/{token}")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        env.get("/api/public/shares/AAAAAAAAAAAAAAAAAAAAAAAA")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        env.get("/api/public/shares/bogus").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn drop_link_uploads() {
    let env = setup().await;
    let (_, admin) = env.user("alice", true, Perms::ALL, "/").await;
    let (s, _) = env
        .api(
            &admin,
            Method::POST,
            "/api/shares",
            Some(json!({"source": "files", "paths": ["photo.jpg"], "upload_only": true})),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "drop link on a file");

    let share = env
        .create(
            &admin,
            json!({"source": "files", "paths": ["/drop"], "upload_only": true}),
        )
        .await;
    let token = share["token"].as_str().unwrap();

    // Visitors cannot list or download.
    let (_, _, b) = env.get(&format!("/api/public/shares/{token}")).await;
    let v = json(&b);
    assert_eq!(v["upload_only"], true);
    assert!(v.get("entries").is_none());
    assert_eq!(
        env.get(&format!("/api/public/shares/{token}/download"))
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    let up = |name: &str, data: &'static [u8]| {
        Request::post(format!("/api/public/shares/{token}/uploads?path={name}"))
            .body(Body::from(data))
            .unwrap()
    };
    let (s, _, b) = env.send(up("note.txt", b"one")).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(json(&b)["name"], "note.txt");
    // Same name again never overwrites.
    let (s, _, b) = env.send(up("note.txt", b"two")).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(json(&b)["name"], "note (1).txt");
    assert_eq!(
        std::fs::read(env.root.join("drop/note.txt")).unwrap(),
        b"one"
    );
    assert_eq!(
        std::fs::read(env.root.join("drop/note (1).txt")).unwrap(),
        b"two"
    );

    for bad in ["../x.txt", "sub/x.txt", "", ".."] {
        let (s, _, _) = env.send(up(bad, b"x")).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad:?}");
    }
    assert!(!env.root.join("x.txt").exists());

    // Size limit (1 MiB in the test config).
    static BIG: [u8; 1024 * 1024 + 1] = [0; 1024 * 1024 + 1];
    let (s, _, _) = env.send(up("big.bin", &BIG)).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(!env.root.join("drop/big.bin").exists());

    // Normal shares refuse uploads.
    let normal = env
        .create(&admin, json!({"source": "files", "paths": ["/docs"]}))
        .await;
    let t2 = normal["token"].as_str().unwrap();
    let (s, _, _) = env
        .send(
            Request::post(format!("/api/public/shares/{t2}/uploads?path=x.txt"))
                .body(Body::from("x"))
                .unwrap(),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn permissions_and_ownership() {
    let env = setup().await;
    let (_, admin) = env.user("alice", true, Perms::ALL, "/").await;
    // Bob may view and download in /docs only, and cannot share.
    let (bob_id, bob) = env
        .user("bob", false, Perms::VIEW | Perms::DOWNLOAD, "/docs")
        .await;
    let (s, _) = env
        .api(
            &bob,
            Method::POST,
            "/api/shares",
            Some(json!({"source": "files", "paths": ["a.txt"]})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    env.set_perms(bob_id, Perms::VIEW | Perms::DOWNLOAD | Perms::SHARE)
        .await;
    // Bob's paths are relative to his scope; he cannot reach outside it.
    let share = env
        .create(&bob, json!({"source": "files", "paths": ["a.txt"]}))
        .await;
    assert_eq!(share["paths"], json!(["/a.txt"]));
    let (s, _) = env
        .api(
            &bob,
            Method::POST,
            "/api/shares",
            Some(json!({"source": "files", "paths": ["../private"]})),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // No upload right means no drop link.
    let (s, _) = env
        .api(
            &bob,
            Method::POST,
            "/api/shares",
            Some(json!({"source": "files", "paths": ["sub"], "upload_only": true})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Unknown source.
    let (s, _) = env
        .api(
            &bob,
            Method::POST,
            "/api/shares",
            Some(json!({"source": "nope", "paths": ["a.txt"]})),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let token = share["token"].as_str().unwrap().to_owned();
    let id = share["id"].as_i64().unwrap();

    // Admin sees Bob's share with the real path; Bob sees only his own.
    let admin_share = env
        .create(&admin, json!({"source": "files", "paths": ["/photo.jpg"]}))
        .await;
    let (_, list) = env.api(&admin, Method::GET, "/api/shares", None).await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    let (_, list) = env.api(&bob, Method::GET, "/api/shares", None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["paths"], json!(["/a.txt"]));
    let admin_id = admin_share["id"].as_i64().unwrap();
    let (s, _) = env
        .api(
            &bob,
            Method::DELETE,
            &format!("/api/shares/{admin_id}"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = env
        .api(&bob, Method::GET, &format!("/api/shares/{admin_id}"), None)
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Losing the share right kills the link immediately.
    let url = format!("/api/public/shares/{token}/download");
    assert_eq!(env.get(&url).await.0, StatusCode::OK);
    env.set_perms(bob_id, Perms::VIEW | Perms::DOWNLOAD).await;
    assert_eq!(env.get(&url).await.0, StatusCode::NOT_FOUND);
    env.set_perms(bob_id, Perms::VIEW | Perms::DOWNLOAD | Perms::SHARE)
        .await;
    assert_eq!(env.get(&url).await.0, StatusCode::OK);
    // So does disabling the user.
    env.disable(bob_id).await;
    assert_eq!(env.get(&url).await.0, StatusCode::NOT_FOUND);

    // Admin can revoke anyone's share.
    let (s, _) = env
        .api(&admin, Method::DELETE, &format!("/api/shares/{id}"), None)
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = env
        .api(&admin, Method::GET, &format!("/api/shares/{id}"), None)
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn management_requires_login_and_csrf() {
    let env = setup().await;
    let (_, admin) = env.user("alice", true, Perms::ALL, "/").await;
    let (s, _, _) = env.get("/api/shares").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let req = Request::post("/api/shares")
        .header(header::COOKIE, &admin.cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"source": "files", "paths": ["/docs"]}).to_string(),
        ))
        .unwrap();
    assert_eq!(env.send(req).await.0, StatusCode::FORBIDDEN);
    let (s, _) = env
        .api(
            &admin,
            Method::POST,
            "/api/shares",
            Some(json!({"source": "files", "paths": []})),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = env
        .api(
            &admin,
            Method::POST,
            "/api/shares",
            Some(json!({"source": "files", "paths": ["/docs"], "password": ""})),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = env
        .api(
            &admin,
            Method::POST,
            "/api/shares",
            Some(json!({"source": "files", "paths": ["/docs"], "max_downloads": 0})),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn trash_is_never_shared() {
    let env = setup().await;
    let (_, admin) = env.user("alice", true, Perms::ALL, "/").await;
    std::fs::create_dir_all(env.root.join(".filebrary-trash/x")).unwrap();
    std::fs::write(env.root.join(".filebrary-trash/x/gone.txt"), b"deleted").unwrap();

    let (s, _) = env
        .api(
            &admin,
            Method::POST,
            "/api/shares",
            Some(json!({"source": "files", "paths": ["/.filebrary-trash/x"]})),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // A share of the whole source hides the trash folder.
    let share = env
        .create(&admin, json!({"source": "files", "paths": ["/"]}))
        .await;
    let token = share["token"].as_str().unwrap();
    let (_, _, b) = env.get(&format!("/api/public/shares/{token}")).await;
    let names: Vec<_> = json(&b)["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_owned())
        .collect();
    assert!(names.contains(&"docs".to_owned()));
    assert!(!names.contains(&".filebrary-trash".to_owned()), "{names:?}");
    for p in [".filebrary-trash", ".filebrary-trash/x/gone.txt"] {
        let (s, _, _) = env
            .get(&format!("/api/public/shares/{token}/download?path={p}"))
            .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{p}");
        let (s, _, _) = env
            .get(&format!("/api/public/shares/{token}?path={p}"))
            .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{p}");
    }
    let (s, _, b) = env
        .get(&format!("/api/public/shares/{token}/download"))
        .await;
    assert_eq!(s, StatusCode::OK);
    let zip = zip::ZipArchive::new(std::io::Cursor::new(b)).unwrap();
    assert!(
        zip.file_names()
            .all(|n| !n.unwrap().contains(".filebrary-trash")),
        "zip must not contain the trash"
    );
}

#[tokio::test]
async fn folders_hidden_from_the_creator_stay_hidden() {
    let env = setup().await;
    std::fs::write(env.root.join("docs/sub/hr.txt"), b"salaries").unwrap();
    // Bob may share everything, but a rule hides /docs/sub from him.
    let (bob_id, bob) = env
        .user(
            "bob",
            false,
            Perms::VIEW | Perms::DOWNLOAD | Perms::UPLOAD | Perms::SHARE,
            "/",
        )
        .await;
    let share = env
        .create(&bob, json!({"source": "files", "paths": ["/docs"]}))
        .await;
    let token = share["token"].as_str().unwrap().to_owned();
    #[cfg(unix)]
    std::os::unix::fs::symlink(env.root.join("docs/sub"), env.root.join("docs/alias")).unwrap();
    // Before the rule, everything is visible.
    let (s, _, _) = env
        .get(&format!(
            "/api/public/shares/{token}/download?path=sub/hr.txt"
        ))
        .await;
    assert_eq!(s, StatusCode::OK);

    env.rule(bob_id, "/docs/sub", Perms::NONE).await;

    let (_, _, b) = env.get(&format!("/api/public/shares/{token}")).await;
    let names: Vec<_> = json(&b)["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_owned())
        .collect();
    assert!(names.contains(&"a.txt".to_owned()), "{names:?}");
    assert!(!names.contains(&"sub".to_owned()), "{names:?}");
    let mut denied = vec!["sub", "sub/hr.txt"];
    if cfg!(unix) {
        denied.extend(["alias", "alias/hr.txt"]);
    }
    for p in denied {
        let (s, _, b) = env
            .get(&format!("/api/public/shares/{token}/download?path={p}"))
            .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "download {p}");
        assert!(!String::from_utf8_lossy(&b).contains("salaries"));
        let (s, _, _) = env
            .get(&format!("/api/public/shares/{token}?path={p}"))
            .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "list {p}");
    }
    let (s, _, b) = env
        .get(&format!("/api/public/shares/{token}/download"))
        .await;
    assert_eq!(s, StatusCode::OK);
    let zip = zip::ZipArchive::new(std::io::Cursor::new(b.clone())).unwrap();
    let names: Vec<String> = zip.file_names().map(|n| n.unwrap().into_owned()).collect();
    assert!(names.contains(&"a.txt".to_owned()), "{names:?}");
    assert!(!names.iter().any(|n| n.contains("hr.txt")), "{names:?}");
    assert!(!b.windows(8).any(|w| w == b"salaries"));

    // Drop links into a folder the creator can no longer upload to stop working.
    let drop = env
        .create(
            &bob,
            json!({"source": "files", "paths": ["/drop"], "upload_only": true}),
        )
        .await;
    let dt = drop["token"].as_str().unwrap();
    env.rule(bob_id, "/drop", Perms::VIEW).await;
    let (s, _, _) = env
        .send(
            Request::post(format!("/api/public/shares/{dt}/uploads?path=x.txt"))
                .body(Body::from("x"))
                .unwrap(),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert!(!env.root.join("drop/x.txt").exists());
}

#[tokio::test]
async fn opening_and_dropping_publish_events() {
    use filebrary_events::Event;

    let env = setup().await;
    let mut rx = env.state.events.subscribe();
    let (_, admin) = env.user("alice", true, Perms::ALL, "/").await;
    let share = env
        .create(&admin, json!({"source": "files", "paths": ["/docs"]}))
        .await;
    let token = share["token"].as_str().unwrap();
    // Opening the link is reported once; browsing deeper is not.
    env.get(&format!("/api/public/shares/{token}")).await;
    env.get(&format!("/api/public/shares/{token}?path=sub"))
        .await;
    let notice = rx.try_recv().unwrap();
    match notice.event {
        Event::ShareOpened {
            share_id,
            owner,
            source,
            path,
            ..
        } => {
            assert_eq!(share_id, share["id"].to_string());
            assert_eq!(owner, "alice");
            assert_eq!(source, "files");
            assert_eq!(path, "/docs");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(rx.try_recv().is_err());

    // A password-protected link is reported only once it is unlocked.
    let locked = env
        .create(
            &admin,
            json!({"source": "files", "paths": ["/docs"], "password": "hunter22hunter"}),
        )
        .await;
    let locked = locked["token"].as_str().unwrap();
    env.get(&format!("/api/public/shares/{locked}")).await;
    assert!(rx.try_recv().is_err());

    // Drop-link uploads are upload events of the link owner.
    let drop = env
        .create(
            &admin,
            json!({"source": "files", "paths": ["/drop"], "upload_only": true}),
        )
        .await;
    let drop = drop["token"].as_str().unwrap();
    let (s, _, _) = env
        .send(
            Request::post(format!("/api/public/shares/{drop}/uploads?path=n.txt"))
                .body(Body::from("hi"))
                .unwrap(),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let mut events = Vec::new();
    while let Ok(n) = rx.try_recv() {
        events.push(n.event);
    }
    assert!(events.iter().any(|e| matches!(
        e,
        Event::Upload { user, path, size: 2, .. } if user == "alice" && path == "/drop/n.txt"
    )));
}
