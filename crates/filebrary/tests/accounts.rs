//! Account foundations: first-run setup, "remember me", roles, home
//! folders, password resets and preferences, driven through the full router.

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use filebrary::{
    AppState,
    config::{Config, SourceConfig},
    db,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

struct App {
    router: Router,
    state: AppState,
    dir: tempfile::TempDir,
}

#[derive(Clone, Default)]
struct Client {
    cookie: Option<String>,
    csrf: Option<String>,
}

struct Resp {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Value,
}

/// An app with no users; `admin_password` set or not.
async fn app(admin_password: Option<&str>) -> (App, Option<String>) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("files")).unwrap();
    let mut config = Config {
        data_dir: dir.path().join("data"),
        admin_password: admin_password.map(str::to_owned),
        login_max_attempts: 3,
        ..Config::default()
    };
    config
        .sources
        .push(SourceConfig::local("files", dir.path().join("files")));
    let pool = db::open_memory().await.unwrap();
    let state = AppState::new(config, pool, [7u8; 32]).unwrap();
    filebrary::identity::install(&state);
    let token = filebrary::auth::bootstrap_admin(&state).await.unwrap();
    (
        App {
            router: filebrary::app(state.clone()),
            state,
            dir,
        },
        token,
    )
}

impl App {
    async fn call(&self, c: &Client, method: Method, uri: &str, body: Option<Value>) -> Resp {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(ck) = &c.cookie {
            req = req.header(header::COOKIE, ck);
        }
        if let Some(t) = &c.csrf {
            req = req.header("x-csrf-token", t);
        }
        let req = match body {
            Some(b) => req
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let resp = self.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        Resp {
            status,
            headers,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }

    /// Signs in (or runs setup) and returns the client plus the Set-Cookie.
    async fn session(&self, uri: &str, body: Value) -> (Client, String) {
        let r = self
            .call(&Client::default(), Method::POST, uri, Some(body))
            .await;
        assert_eq!(r.status, StatusCode::OK, "{uri}: {}", r.body);
        let set = r
            .headers
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        (
            Client {
                cookie: Some(set.split(';').next().unwrap().to_owned()),
                csrf: Some(r.body["csrf_token"].as_str().unwrap().to_owned()),
            },
            set,
        )
    }

    async fn login(&self, user: &str, pw: &str) -> Client {
        self.session(
            "/api/auth/login",
            json!({ "username": user, "password": pw }),
        )
        .await
        .0
    }
}

#[tokio::test]
async fn first_run_setup_creates_the_admin_once() {
    let (a, token) = app(None).await;
    let token = token.expect("setup mode without a bootstrap password");
    let anon = Client::default();
    // Nobody exists and there is no default password.
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&a.state.db)
        .await
        .unwrap();
    assert_eq!(users, 0);
    let r = a
        .call(
            &anon,
            Method::POST,
            "/api/auth/login",
            Some(json!({"username": "admin", "password": "admin"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    let r = a.call(&anon, Method::GET, "/api/setup", None).await;
    assert_eq!(r.body["needed"], true);

    // A wrong token is refused (and counted by the rate limiter).
    let r = a
        .call(
            &anon,
            Method::POST,
            "/api/setup",
            Some(json!({"token": "wrong", "username": "azule", "password": "a-long-password"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // A weak password is refused without using up the token.
    let r = a
        .call(
            &anon,
            Method::POST,
            "/api/setup",
            Some(json!({"token": token, "username": "azule", "password": "x"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    let (c, _) = a
        .session(
            "/api/setup",
            json!({"token": token, "username": "azule", "password": "a-long-password"}),
        )
        .await;
    let me = a.call(&c, Method::GET, "/api/me", None).await;
    assert_eq!(me.body["user"]["username"], "azule");
    assert_eq!(me.body["user"]["role"], "admin");

    // Done: the token no longer works and setup is not offered.
    let r = a.call(&anon, Method::GET, "/api/setup", None).await;
    assert_eq!(r.body["needed"], false);
    let r = a
        .call(
            &anon,
            Method::POST,
            "/api/setup",
            Some(json!({"token": token, "username": "eve", "password": "another-password"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn setup_tokens_are_rate_limited() {
    let (a, _) = app(None).await;
    let anon = Client::default();
    let mut last = StatusCode::OK;
    for _ in 0..5 {
        last = a
            .call(
                &anon,
                Method::POST,
                "/api/setup",
                Some(json!({"token": "guess", "username": "x", "password": "a-long-password"})),
            )
            .await
            .status;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn bootstrap_password_skips_setup() {
    let (a, token) = app(Some("bootstrap-pass-1")).await;
    assert!(token.is_none());
    let r = a
        .call(&Client::default(), Method::GET, "/api/setup", None)
        .await;
    assert_eq!(r.body["needed"], false);
    let c = a.login("admin", "bootstrap-pass-1").await;
    // The admin changes the bootstrap password from the settings page.
    let r = a
        .call(
            &c,
            Method::POST,
            "/api/me/password",
            Some(json!({"current": "bootstrap-pass-1", "new": "my-own-password"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    a.login("admin", "my-own-password").await;
}

#[tokio::test]
async fn remember_me_controls_the_cookie_lifetime() {
    let (a, _) = app(Some("bootstrap-pass-1")).await;
    let (_, set) = a
        .session(
            "/api/auth/login",
            json!({"username": "admin", "password": "bootstrap-pass-1", "remember": true}),
        )
        .await;
    assert!(set.contains("Max-Age="), "{set}");
    assert!(
        set.contains("HttpOnly") && set.contains("SameSite=Lax"),
        "{set}"
    );
    let (c, set) = a
        .session(
            "/api/auth/login",
            json!({"username": "admin", "password": "bootstrap-pass-1", "remember": false}),
        )
        .await;
    assert!(
        !set.contains("Max-Age") && !set.contains("Expires"),
        "{set}"
    );
    // The short session works and is stored as non-persistent with a short expiry.
    assert_eq!(
        a.call(&c, Method::GET, "/api/me", None).await.status,
        StatusCode::OK
    );
    let (persistent, ttl): (bool, i64) = sqlx::query_as(
        "SELECT persistent, expires_at - created_at FROM sessions ORDER BY created_at DESC, rowid DESC LIMIT 1",
    )
    .fetch_one(&a.state.db)
    .await
    .unwrap();
    assert!(!persistent);
    assert!(ttl <= 12 * 3600, "{ttl}");
}

#[tokio::test]
async fn roles_home_folders_and_password_resets() {
    let (a, _) = app(Some("bootstrap-pass-1")).await;
    let admin = a.login("admin", "bootstrap-pass-1").await;

    // A read-only user with a home folder.
    let r = a
        .call(
            &admin,
            Method::POST,
            "/api/users",
            Some(json!({
                "username": "reader", "password": "reader-password",
                "role": "read_only", "home": {"source": "files"}
            })),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(r.body["role"], "read_only");
    let reader_id = r.body["id"].as_i64().unwrap();
    let home = a.dir.path().join("files/home/reader");
    assert!(home.is_dir());
    std::fs::write(home.join("note.txt"), "hi").unwrap();

    let reader = a.login("reader", "reader-password").await;
    let me = a.call(&reader, Method::GET, "/api/me", None).await;
    assert_eq!(me.body["user"]["role"], "read_only");
    assert_eq!(me.body["sources"]["files"]["scope"], "/home/reader");
    // Sees their home as the root, can download, cannot write.
    let r = a
        .call(&reader, Method::GET, "/api/sources/files/list?path=/", None)
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["entries"][0]["name"], "note.txt");
    let r = a
        .call(
            &reader,
            Method::POST,
            "/api/sources/files/mkdir",
            Some(json!({"path": "/x"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // An API key cannot widen a read-only account.
    let r = a
        .call(
            &reader,
            Method::POST,
            "/api/me/api-keys",
            Some(json!({"name": "k", "scopes": ["files:read", "files:write", "files:delete"]})),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    {
        let key = r.body["token"].as_str().unwrap().to_owned();
        let req = Request::post("/api/sources/files/mkdir")
            .header(header::AUTHORIZATION, format!("Bearer {key}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"path":"/x"}"#))
            .unwrap();
        let s = a.router.clone().oneshot(req).await.unwrap().status();
        assert_eq!(s, StatusCode::FORBIDDEN);
    }

    // Promote to a regular user: now they can write in their home.
    let r = a
        .call(
            &admin,
            Method::PUT,
            &format!("/api/users/{reader_id}"),
            Some(json!({"role": "user"})),
        )
        .await;
    assert_eq!(r.body["role"], "user");
    let r = a
        .call(
            &reader,
            Method::POST,
            "/api/sources/files/mkdir",
            Some(json!({"path": "/x"})),
        )
        .await;
    assert!(r.status.is_success(), "{}", r.body);
    assert!(home.join("x").is_dir());
    // role and is_admin together are ambiguous.
    let r = a
        .call(
            &admin,
            Method::PUT,
            &format!("/api/users/{reader_id}"),
            Some(json!({"role": "user", "is_admin": true})),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // Reset the password: their sessions end, the new password works.
    let r = a
        .call(
            &admin,
            Method::PUT,
            &format!("/api/users/{reader_id}"),
            Some(json!({"password": "reset-password-1"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        a.call(&reader, Method::GET, "/api/me", None).await.status,
        StatusCode::UNAUTHORIZED
    );
    a.login("reader", "reset-password-1").await;

    // A home folder for an existing user, at a chosen path.
    let r = a
        .call(
            &admin,
            Method::PUT,
            &format!("/api/users/{reader_id}/home"),
            Some(json!({"source": "files", "path": "/people/reader"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(a.dir.path().join("files/people/reader").is_dir());
    // Not the root, not outside the source, not an unknown source.
    for body in [
        json!({"source": "files", "path": "/"}),
        json!({"source": "files", "path": "/../etc"}),
        json!({"source": "nope"}),
    ] {
        let r = a
            .call(
                &admin,
                Method::PUT,
                &format!("/api/users/{reader_id}/home"),
                Some(body.clone()),
            )
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}");
    }
    // A failed home folder does not leave the account behind.
    let r = a
        .call(
            &admin,
            Method::POST,
            "/api/users",
            Some(json!({"username": "ghost", "password": "ghost-password", "home": {"source": "files", "path": "/"}})),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let list = a.call(&admin, Method::GET, "/api/users", None).await;
    assert!(
        !list
            .body
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["username"] == "ghost")
    );
}

#[tokio::test]
async fn preferences_round_trip() {
    let (a, _) = app(Some("bootstrap-pass-1")).await;
    let c = a.login("admin", "bootstrap-pass-1").await;
    let r = a.call(&c, Method::GET, "/api/me/preferences", None).await;
    assert_eq!(r.body, json!({"view_mode": "details"}));
    let r = a
        .call(
            &c,
            Method::PUT,
            "/api/me/preferences",
            Some(json!({"view_mode": "large"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = a.call(&c, Method::GET, "/api/me/preferences", None).await;
    assert_eq!(r.body, json!({"view_mode": "large"}));
    let r = a
        .call(
            &c,
            Method::PUT,
            "/api/me/preferences",
            Some(json!({"view_mode": "huge"})),
        )
        .await;
    assert!(r.status.is_client_error());
}
