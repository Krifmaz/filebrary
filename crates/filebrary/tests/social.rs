//! Integration tests for comments, collections, casting and branding, run
//! against the full router with a temporary directory as the source.

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use filebrary::{AppState, app, auth::session, config::Config, config::SourceConfig, db};
use serde_json::{Value, json};
use tower::ServiceExt;

struct Ctx {
    app: Router,
    state: AppState,
    _dir: tempfile::TempDir,
}

/// A logged-in client: cookie and CSRF token.
struct Client {
    cookie: String,
    csrf: String,
}

async fn setup() -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let files = dir.path().join("files");
    std::fs::create_dir_all(files.join("films")).unwrap();
    std::fs::create_dir_all(files.join("home/bob")).unwrap();
    std::fs::write(files.join("films/a.mp4"), b"0123456789").unwrap();
    std::fs::write(files.join("notes.txt"), b"hello").unwrap();
    std::fs::write(files.join("home/bob/b.txt"), b"bob").unwrap();
    let config = Config {
        data_dir: dir.path().join("data"),
        sources: vec![SourceConfig {
            name: "files".into(),
            kind: Default::default(),
            path: files,
            content_search: false,
        }],
        ..Config::default()
    };
    let pool = db::open_memory().await.unwrap();
    let state = AppState::new(config, pool, [7; 32]).unwrap();
    Ctx {
        app: app(state.clone()),
        state,
        _dir: dir,
    }
}

impl Ctx {
    /// Creates a user. `grant` is `(perm bits, scope)` on source `files`.
    async fn user(&self, name: &str, admin: bool, grant: Option<(i64, &str)>) -> (i64, Client) {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at)
             VALUES (?, 'x', ?, 0, 0) RETURNING id",
        )
        .bind(name)
        .bind(admin)
        .fetch_one(&self.state.db)
        .await
        .unwrap();
        if let Some((perms, scope)) = grant {
            let scope = filebrary::storage::VPath::parse(scope).unwrap();
            filebrary::auth::permissions::set(
                &self.state.db,
                "files",
                &scope,
                filebrary::auth::Subject::User(id),
                filebrary::auth::Perms::from_bits(perms),
            )
            .await
            .unwrap();
            filebrary::auth::users::set_scope(&self.state.db, id, "files", &scope)
                .await
                .unwrap();
        }
        let s = session::create(&self.state, id, None, None).await.unwrap();
        (
            id,
            Client {
                cookie: format!("{}={}", session::SESSION_COOKIE, s.token),
                csrf: s.csrf_token,
            },
        )
    }

    async fn call(
        &self,
        c: Option<&Client>,
        method: Method,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(c) = c {
            req = req
                .header(header::COOKIE, &c.cookie)
                .header("x-csrf-token", &c.csrf);
        }
        let req = match body {
            Some(b) => req
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, v)
    }
}

const ALL: i64 = 0b11_1111;
const VIEW: i64 = 1;
const VIEW_DL: i64 = 0b11;

#[tokio::test]
async fn comments_flow_and_permissions() {
    let ctx = setup().await;
    let (_, alice) = ctx.user("alice", false, Some((VIEW_DL, "/"))).await;
    let (_, viewer) = ctx.user("viewer", false, Some((VIEW, "/"))).await;
    let (_, bob) = ctx.user("bob", false, Some((VIEW_DL, "/home/bob"))).await;
    let (_, admin) = ctx.user("root", true, None).await;

    let post = |path: &str, body: &str| json!({"source": "files", "path": path, "body": body});
    let (s, c) = ctx
        .call(
            Some(&alice),
            Method::POST,
            "/api/comments",
            Some(post("/notes.txt", " hi ")),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(c["body"], "hi");
    let id = c["id"].as_i64().unwrap();

    // Missing files, empty bodies, view-only users and anonymous callers are refused.
    let (s, _) = ctx
        .call(
            Some(&alice),
            Method::POST,
            "/api/comments",
            Some(post("/nope", "x")),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = ctx
        .call(
            Some(&alice),
            Method::POST,
            "/api/comments",
            Some(post("/notes.txt", "  ")),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = ctx
        .call(
            Some(&viewer),
            Method::POST,
            "/api/comments",
            Some(post("/notes.txt", "x")),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = ctx
        .call(
            None,
            Method::GET,
            "/api/comments?source=files&path=/notes.txt",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = ctx
        .call(
            Some(&alice),
            Method::POST,
            "/api/comments",
            Some(post("/../etc/passwd", "x")),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // The viewer reads the thread but may not edit or delete.
    let (s, list) = ctx
        .call(
            Some(&viewer),
            Method::GET,
            "/api/comments?source=files&path=/notes.txt",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list[0]["username"], "alice");
    assert_eq!(list[0]["can_edit"], false);
    let (s, _) = ctx
        .call(
            Some(&viewer),
            Method::DELETE,
            &format!("/api/comments/{id}"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Bob's scope does not include /notes.txt: the comment does not exist for him.
    let (s, _) = ctx
        .call(
            Some(&bob),
            Method::DELETE,
            &format!("/api/comments/{id}"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Scoped users see the same thread under their own path.
    let (s, _) = ctx
        .call(
            Some(&bob),
            Method::POST,
            "/api/comments",
            Some(post("/b.txt", "mine")),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, l) = ctx
        .call(
            Some(&alice),
            Method::GET,
            "/api/comments?source=files&path=/home/bob/b.txt",
            None,
        )
        .await;
    assert_eq!(l[0]["body"], "mine");
    let (_, counts) = ctx
        .call(
            Some(&alice),
            Method::GET,
            "/api/comments/counts?source=files&dir=/",
            None,
        )
        .await;
    assert_eq!(counts, json!({"notes.txt": 1}));
    let (_, counts) = ctx
        .call(
            Some(&bob),
            Method::GET,
            "/api/comments/counts?source=files&dir=/",
            None,
        )
        .await;
    assert_eq!(counts, json!({"b.txt": 1}));

    // Author edits; admin moderates.
    let (s, _) = ctx
        .call(
            Some(&alice),
            Method::PATCH,
            &format!("/api/comments/{id}"),
            Some(json!({"body": "edited"})),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = ctx
        .call(
            Some(&admin),
            Method::PATCH,
            &format!("/api/comments/{id}"),
            Some(json!({"body": "x"})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = ctx
        .call(
            Some(&admin),
            Method::DELETE,
            &format!("/api/comments/{id}"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn csrf_is_required_for_comments() {
    let ctx = setup().await;
    let (_, mut alice) = ctx.user("alice", false, Some((VIEW_DL, "/"))).await;
    alice.csrf = "wrong".into();
    let (s, _) = ctx
        .call(
            Some(&alice),
            Method::POST,
            "/api/comments",
            Some(json!({"source": "files", "path": "/notes.txt", "body": "x"})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn collections_flow() {
    let ctx = setup().await;
    let (_, alice) = ctx.user("alice", false, Some((ALL, "/"))).await;
    let (_, bob) = ctx.user("bob", false, Some((VIEW_DL, "/home/bob"))).await;

    let (s, c) = ctx
        .call(
            Some(&alice),
            Method::POST,
            "/api/collections",
            Some(json!({"name": "Trip", "items": [{"source": "files", "path": "/films/a.mp4"}]})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let id = c["id"].as_i64().unwrap();
    assert_eq!(c["items"][0]["name"], "a.mp4");
    assert_eq!(c["items"][0]["kind"], "file");

    // Adding twice is a no-op; folders work; the source root and missing paths do not.
    let add = |p: &str| json!({"items": [{"source": "files", "path": p}]});
    let uri = format!("/api/collections/{id}/items");
    let (s, c) = ctx
        .call(Some(&alice), Method::POST, &uri, Some(add("/films/a.mp4")))
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(c["items"].as_array().unwrap().len(), 1);
    let (_, c) = ctx
        .call(Some(&alice), Method::POST, &uri, Some(add("/home")))
        .await;
    assert_eq!(c["items"][1]["kind"], "dir");
    let (s, _) = ctx
        .call(Some(&alice), Method::POST, &uri, Some(add("/")))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = ctx
        .call(Some(&alice), Method::POST, &uri, Some(add("/missing")))
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Collections are private to their owner.
    let (s, _) = ctx
        .call(
            Some(&bob),
            Method::GET,
            &format!("/api/collections/{id}"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (_, l) = ctx
        .call(Some(&bob), Method::GET, "/api/collections", None)
        .await;
    assert_eq!(l, json!([]));

    // Share resolution: alice may share both; bob (no share perm) may share nothing.
    let alice_user = filebrary::auth::load_user(&ctx.state, 1)
        .await
        .unwrap()
        .unwrap();
    filebrary::collections::check_shareable(&ctx.state, &alice_user, id)
        .await
        .unwrap();
    let items = filebrary::collections::share_items(&ctx.state, &alice_user, id)
        .await
        .unwrap();
    assert_eq!(
        items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
        ["a.mp4", "home"]
    );
    // Losing the share permission drops the items from the link.
    filebrary::auth::permissions::set(
        &ctx.state.db,
        "files",
        &filebrary::storage::VPath::root(),
        filebrary::auth::Subject::User(1),
        filebrary::auth::Perms::from_bits(3),
    )
    .await
    .unwrap();
    let alice_user = filebrary::auth::load_user(&ctx.state, 1)
        .await
        .unwrap()
        .unwrap();
    assert!(
        filebrary::collections::share_items(&ctx.state, &alice_user, id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        filebrary::collections::check_shareable(&ctx.state, &alice_user, id)
            .await
            .is_err()
    );

    // Listing, renaming, removing.
    let (_, l) = ctx
        .call(Some(&alice), Method::GET, "/api/collections", None)
        .await;
    assert_eq!(l[0]["item_count"], 2);
    let (s, _) = ctx
        .call(
            Some(&alice),
            Method::PATCH,
            &format!("/api/collections/{id}"),
            Some(json!({"name": "Holiday"})),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let item = c["items"][0]["id"].as_i64().unwrap();
    let (s, _) = ctx
        .call(
            Some(&alice),
            Method::DELETE,
            &format!("/api/collections/{id}/items/{item}"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = ctx
        .call(
            Some(&alice),
            Method::DELETE,
            &format!("/api/collections/{id}"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = ctx
        .call(
            Some(&alice),
            Method::GET,
            &format!("/api/collections/{id}"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cast_urls_serve_ranges_without_cookies() {
    let ctx = setup().await;
    let (alice_id, alice) = ctx.user("alice", false, Some((VIEW_DL, "/"))).await;
    let (_, viewer) = ctx.user("viewer", false, Some((VIEW, "/"))).await;
    let body = |p: &str| json!({"source": "files", "path": p});

    let (s, _) = ctx
        .call(
            Some(&viewer),
            Method::POST,
            "/api/cast",
            Some(body("/films/a.mp4")),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = ctx
        .call(
            Some(&alice),
            Method::POST,
            "/api/cast",
            Some(body("/notes.txt")),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, v) = ctx
        .call(
            Some(&alice),
            Method::POST,
            "/api/cast",
            Some(body("/films/a.mp4")),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let url = v["url"].as_str().unwrap().to_owned();
    assert!(url.starts_with("/api/public/cast/") && url.ends_with("/a%2Emp4"));

    let get = |range: Option<&str>| {
        let mut r = Request::get(&url);
        if let Some(range) = range {
            r = r.header(header::RANGE, range);
        }
        ctx.app.clone().oneshot(r.body(Body::empty()).unwrap())
    };
    let resp = get(None).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()[header::CONTENT_TYPE], "video/mp4");
    assert_eq!(
        to_bytes(resp.into_body(), 100).await.unwrap(),
        &b"0123456789"[..]
    );
    let resp = get(Some("bytes=2-4")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(resp.headers()[header::CONTENT_RANGE], "bytes 2-4/10");
    assert_eq!(to_bytes(resp.into_body(), 100).await.unwrap(), &b"234"[..]);
    let resp = get(Some("bytes=50-")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);

    // A tampered token is not found; a disabled account stops the cast.
    let tampered = url.replacen("/cast/", "/cast/x", 1);
    let resp = ctx
        .app
        .clone()
        .oneshot(Request::get(&tampered).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    sqlx::query("UPDATE users SET disabled = 1 WHERE id = ?")
        .bind(alice_id)
        .execute(&ctx.state.db)
        .await
        .unwrap();
    assert_eq!(get(None).await.unwrap().status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn branding_and_themes() {
    let ctx = setup().await;
    let (_, alice) = ctx.user("alice", false, Some((VIEW, "/"))).await;
    let (_, admin) = ctx.user("root", true, None).await;

    let (s, b) = ctx.call(None, Method::GET, "/api/branding", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["name"], "Filebrary");
    assert_eq!(b["logo_url"], Value::Null);

    let settings = json!({
        "name": "Acme Files", "default_theme": "dark",
        "light": {"--accent": "#336699"}, "dark": {}, "custom": {"--bg": "#101010"}
    });
    let (s, _) = ctx
        .call(
            Some(&alice),
            Method::PUT,
            "/api/admin/branding",
            Some(settings.clone()),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = ctx
        .call(
            Some(&admin),
            Method::PUT,
            "/api/admin/branding",
            Some(json!({"light": {"--accent": "red; background:url(x)"}})),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, b) = ctx
        .call(
            Some(&admin),
            Method::PUT,
            "/api/admin/branding",
            Some(settings),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["name"], "Acme Files");

    // Users inherit the default theme until they pick one.
    let (_, t) = ctx
        .call(Some(&alice), Method::GET, "/api/me/theme", None)
        .await;
    assert_eq!(t["theme"], "dark");
    let (s, _) = ctx
        .call(
            Some(&alice),
            Method::PUT,
            "/api/me/theme",
            Some(json!({"theme": "custom", "colours": {"--text": "#eee"}})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let (_, t) = ctx
        .call(Some(&alice), Method::GET, "/api/me/theme", None)
        .await;
    assert_eq!(t, json!({"theme": "custom", "colours": {"--text": "#eee"}}));

    // Logo: only real images, served with a sandboxing CSP.
    let upload = |bytes: &'static [u8]| {
        Request::put("/api/admin/branding/logo")
            .header(header::COOKIE, &admin.cookie)
            .header("x-csrf-token", &admin.csrf)
            .body(Body::from(bytes))
            .unwrap()
    };
    let r = ctx
        .app
        .clone()
        .oneshot(upload(b"<html><script>alert(1)</script>"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    let r = ctx
        .app
        .clone()
        .oneshot(upload(b"<svg xmlns='http://www.w3.org/2000/svg'/>"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NO_CONTENT);
    let (_, b) = ctx.call(None, Method::GET, "/api/branding", None).await;
    let logo = b["logo_url"].as_str().unwrap().to_owned();
    let r = ctx
        .app
        .clone()
        .oneshot(Request::get(&logo).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()[header::CONTENT_TYPE], "image/svg+xml");
    assert!(
        r.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );

    // The PWA manifest carries the branding.
    let (s, m) = ctx
        .call(None, Method::GET, "/manifest.webmanifest", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(m["name"], "Acme Files");
    assert_eq!(m["theme_color"], "#336699");
    assert_eq!(m["icons"][0]["type"], "image/svg+xml");
}

#[tokio::test]
async fn comments_follow_moves_and_deletes() {
    let ctx = setup().await;
    let (_, alice) = ctx.user("alice", false, Some((ALL, "/"))).await;
    let post = json!({"source": "files", "path": "/films/a.mp4", "body": "great"});
    let (s, _) = ctx
        .call(Some(&alice), Method::POST, "/api/comments", Some(post))
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, _) = ctx
        .call(
            Some(&alice),
            Method::POST,
            "/api/sources/files/move",
            Some(json!({"from": "/films", "to": "/movies"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let (_, l) = ctx
        .call(
            Some(&alice),
            Method::GET,
            "/api/comments?source=files&path=/movies/a.mp4",
            None,
        )
        .await;
    assert_eq!(l[0]["body"], "great");
    let (s, _) = ctx
        .call(
            Some(&alice),
            Method::POST,
            "/api/sources/files/delete",
            Some(json!({"paths": ["/movies"]})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    // Deleting moves the file to the trash, and its thread goes with it.
    let (_, l) = ctx
        .call(
            Some(&alice),
            Method::GET,
            "/api/comments?source=files&path=/movies/a.mp4",
            None,
        )
        .await;
    assert_eq!(l, json!([]));
    let left: Vec<String> = sqlx::query_scalar("SELECT path FROM comments")
        .fetch_all(&ctx.state.db)
        .await
        .unwrap();
    assert!(left.iter().all(|p| !p.starts_with("/movies")), "{left:?}");
}
