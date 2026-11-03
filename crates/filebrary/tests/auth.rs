//! Integration tests for login, sessions, CSRF, rate limiting, user and
//! group administration and permission rules, driven through the full router.

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use filebrary::{
    AppState,
    config::{Config, SourceConfig, SourceKind},
    db,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN_PW: &str = "admin-password-1";

struct App {
    router: Router,
    state: AppState,
    _dir: tempfile::TempDir,
}

async fn app() -> App {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config {
        data_dir: dir.path().join("data"),
        admin_password: Some(ADMIN_PW.into()),
        login_max_attempts: 3,
        ..Config::default()
    };
    config.sources.push(SourceConfig {
        name: "files".into(),
        kind: SourceKind::Local,
        path: dir.path().join("files"),
    });
    let pool = db::open_memory().await.unwrap();
    let state = AppState::new(config, pool, [7u8; 32]).unwrap();
    assert_eq!(
        filebrary::auth::bootstrap_admin(&state).await.unwrap(),
        None
    );
    App {
        router: filebrary::app(state.clone()),
        state,
        _dir: dir,
    }
}

/// A logged-in client: session cookie and CSRF token.
#[derive(Clone, Default)]
struct Client {
    cookie: Option<String>,
    csrf: Option<String>,
}

#[derive(Debug)]
struct Resp {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Value,
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
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Resp {
            status,
            headers,
            body,
        }
    }

    async fn login(&self, user: &str, pw: &str) -> Result<Client, Resp> {
        let r = self
            .call(
                &Client::default(),
                Method::POST,
                "/api/auth/login",
                Some(json!({ "username": user, "password": pw })),
            )
            .await;
        if r.status != StatusCode::OK {
            return Err(r);
        }
        let set = r.headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
        assert!(set.contains("HttpOnly"), "{set}");
        assert!(set.contains("SameSite=Lax"), "{set}");
        let cookie = set.split(';').next().unwrap().to_owned();
        Ok(Client {
            cookie: Some(cookie),
            csrf: Some(r.body["csrf_token"].as_str().unwrap().to_owned()),
        })
    }
}

#[tokio::test]
async fn login_me_csrf_logout() {
    let a = app().await;
    let anon = Client::default();
    assert_eq!(
        a.call(&anon, Method::GET, "/api/me", None).await.status,
        StatusCode::UNAUTHORIZED
    );

    let bad = a.login("admin", "wrong-password").await.err().unwrap();
    assert_eq!(bad.status, StatusCode::UNAUTHORIZED);
    assert_eq!(bad.body["error"], "invalid_credentials");
    let unknown = a.login("nobody", "wrong-password").await.err().unwrap();
    assert_eq!(unknown.body, bad.body);

    let c = a.login("ADMIN", ADMIN_PW).await.unwrap();
    let me = a.call(&c, Method::GET, "/api/me", None).await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.body["user"]["username"], "admin");
    assert_eq!(me.body["user"]["is_admin"], true);
    assert_eq!(me.body["csrf_token"], c.csrf.clone().unwrap().as_str());

    // Unsafe method without or with a wrong CSRF token.
    let mut no_csrf = c.clone();
    no_csrf.csrf = None;
    let r = a
        .call(&no_csrf, Method::POST, "/api/auth/logout", None)
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    no_csrf.csrf = Some("x".repeat(43));
    let r = a
        .call(&no_csrf, Method::POST, "/api/auth/logout", None)
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    let r = a.call(&c, Method::POST, "/api/auth/logout", None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(
        a.call(&c, Method::GET, "/api/me", None).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn forged_or_missing_cookie_is_rejected() {
    let a = app().await;
    let c = Client {
        cookie: Some("fb_session=forged".into()),
        csrf: None,
    };
    assert_eq!(
        a.call(&c, Method::GET, "/api/me", None).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn login_is_rate_limited() {
    let a = app().await;
    for _ in 0..3 {
        let r = a.login("admin", "wrong-password").await.err().unwrap();
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }
    // Even the right password is refused while locked.
    let r = a.login("admin", ADMIN_PW).await.err().unwrap();
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(r.body["error"], "too_many_requests");
    assert!(r.headers.get(header::RETRY_AFTER).is_some());
}

async fn create_user(a: &App, admin: &Client, name: &str) -> i64 {
    let r = a
        .call(
            admin,
            Method::POST,
            "/api/users",
            Some(json!({ "username": name, "password": "user-password-1" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    r.body["id"].as_i64().unwrap()
}

#[tokio::test]
async fn admin_manages_users_groups_and_rules() {
    let a = app().await;
    let admin = a.login("admin", ADMIN_PW).await.unwrap();
    let bob = create_user(&a, &admin, "bob").await;

    // Duplicate names (case-insensitive) conflict; weak passwords are refused.
    let r = a
        .call(
            &admin,
            Method::POST,
            "/api/users",
            Some(json!({ "username": "BOB", "password": "user-password-1" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let r = a
        .call(
            &admin,
            Method::POST,
            "/api/users",
            Some(json!({ "username": "eve", "password": "short" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    let b = a.login("bob", "user-password-1").await.unwrap();
    // Non-admins cannot administer.
    assert_eq!(
        a.call(&b, Method::GET, "/api/users", None).await.status,
        StatusCode::FORBIDDEN
    );
    // No rules yet: nothing.
    let r = a
        .call(
            &b,
            Method::GET,
            "/api/me/permissions?source=files&path=/docs",
            None,
        )
        .await;
    assert_eq!(r.body["perms"], json!([]));

    // A group with read access to the whole source, upload in /drop,
    // nothing in /private.
    let g = a
        .call(
            &admin,
            Method::POST,
            "/api/groups",
            Some(json!({ "name": "staff" })),
        )
        .await;
    assert_eq!(g.status, StatusCode::CREATED);
    let gid = g.body["id"].as_i64().unwrap();
    let r = a
        .call(
            &admin,
            Method::PUT,
            &format!("/api/groups/{gid}/members/{bob}"),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    for (path, perms) in [
        ("/", json!(["view", "download"])),
        ("/drop", json!(["upload"])),
        ("/private", json!([])),
    ] {
        let r = a
            .call(
                &admin,
                Method::PUT,
                "/api/permissions",
                Some(json!({
                    "source": "files", "path": path,
                    "subject": { "type": "group", "id": gid }, "perms": perms
                })),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    }
    let perms = |path: &'static str| {
        let a = &a;
        let b = b.clone();
        async move {
            a.call(
                &b,
                Method::GET,
                &format!("/api/me/permissions?source=files&path={path}"),
                None,
            )
            .await
            .body["perms"]
                .clone()
        }
    };
    assert_eq!(perms("/docs/a.txt").await, json!(["view", "download"]));
    assert_eq!(perms("/drop/x").await, json!(["view", "upload"]));
    assert_eq!(perms("/private/x").await, json!([]));

    // A user rule at the same folder overrides the group.
    let r = a
        .call(
            &admin,
            Method::PUT,
            "/api/permissions",
            Some(json!({
                "source": "files", "path": "/private",
                "subject": { "type": "user", "id": bob }, "perms": ["view"]
            })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(perms("/private/x").await, json!(["view"]));

    // Traversal in a rule path and unknown sources are refused.
    let r = a
        .call(
            &admin,
            Method::PUT,
            "/api/permissions",
            Some(json!({
                "source": "files", "path": "/a/../..",
                "subject": { "type": "user", "id": bob }, "perms": ["view"]
            })),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = a
        .call(
            &admin,
            Method::PUT,
            "/api/permissions",
            Some(json!({
                "source": "nope", "path": "/",
                "subject": { "type": "user", "id": bob }, "perms": ["view"]
            })),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // Leaving the group removes its rules' effect.
    let r = a
        .call(
            &admin,
            Method::DELETE,
            &format!("/api/groups/{gid}/members/{bob}"),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(perms("/docs/a.txt").await, json!([]));
}

#[tokio::test]
async fn disabling_a_user_ends_their_sessions() {
    let a = app().await;
    let admin = a.login("admin", ADMIN_PW).await.unwrap();
    let bob = create_user(&a, &admin, "bob").await;
    let b = a.login("bob", "user-password-1").await.unwrap();
    let r = a
        .call(
            &admin,
            Method::PUT,
            &format!("/api/users/{bob}"),
            Some(json!({ "disabled": true })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        a.call(&b, Method::GET, "/api/me", None).await.status,
        StatusCode::UNAUTHORIZED
    );
    let r = a.login("bob", "user-password-1").await.err().unwrap();
    assert_eq!(r.body["error"], "invalid_credentials");
}

#[tokio::test]
async fn last_admin_is_protected() {
    let a = app().await;
    let admin = a.login("admin", ADMIN_PW).await.unwrap();
    let me = a.call(&admin, Method::GET, "/api/me", None).await;
    let id = me.body["user"]["id"].as_i64().unwrap();
    for (m, body) in [
        (Method::PUT, Some(json!({ "is_admin": false }))),
        (Method::PUT, Some(json!({ "disabled": true }))),
        (Method::DELETE, None),
    ] {
        let r = a.call(&admin, m, &format!("/api/users/{id}"), body).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{:?}", r.body);
    }
}

#[tokio::test]
async fn password_change_keeps_this_session_and_ends_others() {
    let a = app().await;
    let admin = a.login("admin", ADMIN_PW).await.unwrap();
    create_user(&a, &admin, "bob").await;
    let first = a.login("bob", "user-password-1").await.unwrap();
    let second = a.login("bob", "user-password-1").await.unwrap();

    let r = a
        .call(
            &first,
            Method::POST,
            "/api/me/password",
            Some(json!({ "current": "wrong-password", "new": "new-password-2" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = a
        .call(
            &first,
            Method::POST,
            "/api/me/password",
            Some(json!({ "current": "user-password-1", "new": "new-password-2" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(
        a.call(&first, Method::GET, "/api/me", None).await.status,
        StatusCode::OK
    );
    assert_eq!(
        a.call(&second, Method::GET, "/api/me", None).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert!(a.login("bob", "user-password-1").await.is_err());
    assert!(a.login("bob", "new-password-2").await.is_ok());
}

#[tokio::test]
async fn scopes_confine_users() {
    let a = app().await;
    let admin = a.login("admin", ADMIN_PW).await.unwrap();
    let bob = create_user(&a, &admin, "bob").await;
    let r = a
        .call(
            &admin,
            Method::PUT,
            &format!("/api/users/{bob}/scopes"),
            Some(json!({ "source": "files", "scope": "/home/bob" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, json!({ "files": "/home/bob" }));
    a.call(
        &admin,
        Method::PUT,
        "/api/permissions",
        Some(json!({
            "source": "files", "path": "/home/bob",
            "subject": { "type": "user", "id": bob }, "perms": ["download", "upload"]
        })),
    )
    .await;
    let user = filebrary::auth::load_user(&a.state, bob)
        .await
        .unwrap()
        .unwrap();
    let vp = |s: &str| filebrary::storage::VPath::parse(s).unwrap();
    use filebrary::auth::Perms;
    assert_eq!(
        user.require("files", Perms::DOWNLOAD, &vp("a.txt"))
            .unwrap()
            .to_string(),
        "/home/bob/a.txt"
    );
    // ".." cannot even be expressed as a VPath, so the scope cannot be left.
    assert!(filebrary::storage::VPath::parse("../eve").is_err());
}

/// A symlink in a readable folder must not lead into a folder hidden by a
/// deny rule, nor out of the user's scope.
#[cfg(unix)]
#[tokio::test]
async fn symlinks_cannot_bypass_folder_rules() {
    use std::os::unix::fs::symlink;
    let a = app().await;
    let root = a._dir.path().join("files");
    std::fs::create_dir_all(root.join("public")).unwrap();
    std::fs::create_dir_all(root.join("private")).unwrap();
    std::fs::write(root.join("public/ok.txt"), b"ok").unwrap();
    std::fs::write(root.join("private/secret.txt"), b"secret").unwrap();
    symlink(
        root.join("private/secret.txt"),
        root.join("public/file-link"),
    )
    .unwrap();
    symlink(root.join("private"), root.join("public/dir-link")).unwrap();

    let admin = a.login("admin", ADMIN_PW).await.unwrap();
    let bob = create_user(&a, &admin, "bob").await;
    for (path, perms) in [("/", json!(["view", "download"])), ("/private", json!([]))] {
        let r = a
            .call(
                &admin,
                Method::PUT,
                "/api/permissions",
                Some(json!({
                    "source": "files", "path": path,
                    "subject": { "type": "user", "id": bob }, "perms": perms
                })),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK);
    }
    let b = a.login("bob", "user-password-1").await.unwrap();
    for (p, want) in [
        ("/public/ok.txt", StatusCode::OK),
        ("/private/secret.txt", StatusCode::NOT_FOUND),
        ("/public/file-link", StatusCode::FORBIDDEN),
        ("/public/dir-link/secret.txt", StatusCode::FORBIDDEN),
    ] {
        let uri = format!("/api/sources/files/download?path={p}");
        assert_eq!(
            a.call(&b, Method::GET, &uri, None).await.status,
            want,
            "{p}"
        );
    }
    // Listing through the link is refused too.
    let r = a
        .call(
            &b,
            Method::GET,
            "/api/sources/files/list?path=/public/dir-link",
            None,
        )
        .await;
    assert!(
        r.status == StatusCode::FORBIDDEN || r.status == StatusCode::NOT_FOUND,
        "{:?}",
        r.status
    );
}
