//! Integration tests for 2FA, API keys, quotas and OpenID Connect sign-in,
//! driving the full application in-process.

mod common;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    Form, Json, Router,
    body::Body,
    http::{Request, StatusCode, header},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use common::{TestApp, json};
use filebrary::{
    AppState,
    auth::{Perms, users},
    config::{Config, SourceConfig},
    db,
};
use filebrary_identity::{base32, totp};
use hmac::{Hmac, Mac};
use serde_json::{Value, json as j};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

fn login_req(user: &str, pw: &str, code: Option<&str>) -> Request<Body> {
    let mut b = j!({ "username": user, "password": pw });
    if let Some(c) = code {
        b["totp"] = c.into();
    }
    Request::post("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(b.to_string()))
        .unwrap()
}

/// Logs in with a password and returns a cookie client.
async fn login(t: &TestApp, user: &str, pw: &str) -> common::Client {
    let resp = t.send(login_req(user, pw, None)).await;
    let cookie = resp.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let (_, body) = json(resp).await;
    common::Client {
        cookie,
        csrf: body["csrf_token"].as_str().unwrap().to_owned(),
    }
}

/// Parses the query string of an absolute URL (values are percent-decoded).
fn query_of(url: &str) -> HashMap<String, String> {
    let q = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    q.split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_owned(), pct_decode(v)))
        .collect()
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(if b[i] == b'+' { b' ' } else { b[i] });
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

fn now() -> u64 {
    db::now() as u64
}

#[tokio::test]
async fn totp_two_factor_login() {
    let t = TestApp::new().await;
    assert!(filebrary::identity::install(&t.state));
    users::create(&t.state.db, "dana", "correct horse battery", false)
        .await
        .unwrap();
    let c = login(&t, "dana", "correct horse battery").await;

    let (st, setup) = json(t.send(c.post_json("/api/me/totp/setup", j!({}))).await).await;
    assert_eq!(st, StatusCode::OK, "{setup}");
    let secret = base32::decode(setup["secret"].as_str().unwrap()).unwrap();
    assert!(
        setup["uri"]
            .as_str()
            .unwrap()
            .starts_with("otpauth://totp/Filebrary:dana")
    );

    let prev = totp::code_at(&secret, now() - totp::STEP_SECS);
    let (st, conf) = json(
        t.send(c.post_json("/api/me/totp/confirm", j!({ "code": prev })))
            .await,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{conf}");
    let recovery = conf["recovery_codes"][0].as_str().unwrap().to_owned();

    // Password alone now asks for a code.
    let (st, body) = json(
        t.send(login_req("dana", "correct horse battery", None))
            .await,
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "totp_required");
    // A wrong code is an ordinary failed login.
    let (st, body) = json(
        t.send(login_req("dana", "correct horse battery", Some("000000")))
            .await,
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "invalid_credentials");
    // The right code logs in.
    let cur = totp::code_at(&secret, now());
    let resp = t
        .send(login_req("dana", "correct horse battery", Some(&cur)))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get(header::SET_COOKIE).is_some());
    // A recovery code works too, once.
    let resp = t
        .send(login_req("dana", "correct horse battery", Some(&recovery)))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let (_, status) = json(t.send(c.get("/api/me/totp")).await).await;
    assert_eq!(status, j!({ "enabled": true, "recovery_codes_left": 9 }));

    // An administrator can reset a lost device.
    let admin = t.user("root", true, Perms::ALL, "/").await;
    let uid = users::list(&t.state.db)
        .await
        .unwrap()
        .into_iter()
        .find(|u| u.username == "dana")
        .unwrap()
        .id;
    let resp = t
        .send(
            admin
                .req("DELETE", &format!("/api/users/{uid}/totp"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = t
        .send(login_req("dana", "correct horse battery", None))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

fn bearer(token: &str, method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

#[tokio::test]
async fn api_keys() {
    let t = TestApp::new().await;
    assert!(filebrary::identity::install(&t.state));
    let c = t.user("erin", false, Perms::ALL, "/").await;

    let (st, key) = json(
        t.send(c.post_json(
            "/api/me/api-keys",
            j!({ "name": "sync", "scopes": ["files:read"], "expires_in_days": 30 }),
        ))
        .await,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{key}");
    let token = key["token"].as_str().unwrap().to_owned();
    assert_eq!(key["scopes"], j!(["files:read"]));

    // The key authenticates without cookies or CSRF.
    let (st, me) = json(t.send(bearer(&token, "GET", "/api/me", None)).await).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me["user"]["username"], "erin");

    // The key's scopes cap what it can do with files.
    std::fs::write(t.root().join("a.txt"), b"hello").unwrap();
    let resp = t
        .send(bearer(
            &token,
            "GET",
            "/api/sources/files/download?path=/a.txt",
            None,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = t
        .send(bearer(
            &token,
            "POST",
            "/api/sources/files/uploads",
            Some(j!({ "path": "/b.txt", "size": 1 })),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let (_, del) = json(
        t.send(bearer(
            &token,
            "POST",
            "/api/sources/files/delete",
            Some(j!({ "paths": ["/a.txt"] })),
        ))
        .await,
    )
    .await;
    assert!(
        del["deleted"].as_array().is_none_or(|d| d.is_empty()),
        "{del}"
    );
    assert!(t.root().join("a.txt").exists());

    // Keys cannot manage keys or 2FA.
    let resp = t
        .send(bearer(
            &token,
            "POST",
            "/api/me/api-keys",
            Some(j!({ "name": "x", "scopes": ["files:read"] })),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let resp = t
        .send(bearer(&token, "POST", "/api/me/totp/setup", Some(j!({}))))
        .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Non-admins cannot mint admin keys; unknown scopes are refused.
    for scopes in [j!(["admin"]), j!(["root"]), j!([])] {
        let resp = t
            .send(c.post_json("/api/me/api-keys", j!({ "name": "x", "scopes": scopes })))
            .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // Listing never shows the secret.
    let (_, list) = json(t.send(c.get("/api/me/api-keys")).await).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert!(!list.to_string().contains(&token[20..]));

    // A garbage or forged token is a 401.
    let forged = format!("{}x", &token[..token.len() - 1]);
    for bad in [forged.as_str(), "nonsense"] {
        let resp = t.send(bearer(bad, "GET", "/api/me", None)).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // Revoked keys stop working.
    let id = key["id"].as_str().unwrap();
    let resp = t
        .send(
            c.req("DELETE", &format!("/api/me/api-keys/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = t.send(bearer(&token, "GET", "/api/me", None)).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn quotas_limit_uploads() {
    let t = TestApp::new().await;
    assert!(filebrary::identity::install(&t.state));
    let admin = t.user("root", true, Perms::ALL, "/").await;
    let c = t.user("fred", false, Perms::ALL, "/home/fred").await;
    std::fs::create_dir_all(t.root().join("home/fred")).unwrap();
    std::fs::write(t.root().join("home/fred/old.bin"), vec![0u8; 40]).unwrap();
    let uid = users::list(&t.state.db)
        .await
        .unwrap()
        .into_iter()
        .find(|u| u.username == "fred")
        .unwrap()
        .id;

    let req = admin
        .req("PUT", &format!("/api/users/{uid}/quota"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(j!({ "limit": 100 }).to_string()))
        .unwrap();
    let (st, q) = json(t.send(req).await).await;
    assert_eq!(st, StatusCode::OK, "{q}");
    // A rescan counts what is already in the home folder.
    let req = admin
        .req("POST", &format!("/api/users/{uid}/quota/rescan"))
        .body(Body::empty())
        .unwrap();
    let (_, q) = json(t.send(req).await).await;
    assert_eq!(q, j!({ "limit": 100, "used": 40, "free": 60 }));

    let upload = |name: &str, size: u64| {
        c.post_json(
            "/api/sources/files/uploads",
            j!({ "path": name, "size": size }),
        )
    };
    let mut events = t.state.events.subscribe();
    let (st, body) = json(t.send(upload("big.bin", 61)).await).await;
    assert_eq!(st, StatusCode::INSUFFICIENT_STORAGE, "{body}");
    assert_eq!(body["error"], "quota_exceeded");
    // 40 + 15 = 55%: below the 90% warning threshold, no event.
    let resp = t.send(upload("ok.bin", 15)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(events.try_recv().is_err());
    // 55 + 45 = 100%: crosses 90% once, so one QuotaNearFull event.
    let resp = t.send(upload("ok2.bin", 45)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let notice = events.try_recv().unwrap();
    assert_eq!(
        notice.event,
        filebrary_events::Event::QuotaNearFull {
            user: "fred".into(),
            used_bytes: 100,
            quota_bytes: 100,
            threshold_percent: 90,
        }
    );
    let (_, q) = json(t.send(c.get("/api/me/quota")).await).await;
    assert_eq!(q["used"], 100);

    // Size strings work, and null means unlimited.
    let req = admin
        .req("PUT", &format!("/api/users/{uid}/quota"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(j!({ "limit": null }).to_string()))
        .unwrap();
    let (_, q) = json(t.send(req).await).await;
    assert_eq!(q["limit"], Value::Null);
    let resp = t.send(upload("huge.bin", 1 << 30)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    // Users cannot change their own quota.
    let req = c
        .req("PUT", &format!("/api/users/{uid}/quota"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(j!({ "limit": "1T" }).to_string()))
        .unwrap();
    assert_eq!(t.send(req).await.status(), StatusCode::FORBIDDEN);
}

// ------------------------------------------------------------ OIDC

const SECRET: &str = "a-client-secret-long-enough-for-hs256";

#[derive(Default)]
struct Mock {
    issuer: String,
    codes: HashMap<String, (String, String)>,
    sub: String,
    username: String,
    groups: Vec<String>,
}
type Shared = Arc<Mutex<Mock>>;

fn sign(claims: &Value) -> String {
    let header = B64.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let body = B64.encode(serde_json::to_vec(claims).unwrap());
    let mut mac = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(format!("{header}.{body}").as_bytes());
    format!(
        "{header}.{body}.{}",
        B64.encode(mac.finalize().into_bytes())
    )
}

async fn start_mock() -> Shared {
    let shared: Shared = Arc::default();
    let s1 = shared.clone();
    let s2 = shared.clone();
    let app =
        Router::new()
            .route(
                "/.well-known/openid-configuration",
                get(move || {
                    let iss = s1.lock().unwrap().issuer.clone();
                    async move {
                        Json(j!({
                            "issuer": iss,
                            "authorization_endpoint": format!("{iss}/authorize"),
                            "token_endpoint": format!("{iss}/token"),
                            "jwks_uri": format!("{iss}/jwks"),
                            "response_types_supported": ["code"],
                            "subject_types_supported": ["public"],
                            "id_token_signing_alg_values_supported": ["HS256"],
                        }))
                    }
                }),
            )
            .route("/jwks", get(|| async { Json(j!({ "keys": [] })) }))
            .route(
                "/token",
                post(move |Form(f): Form<HashMap<String, String>>| {
                    let mut m = s2.lock().unwrap();
                    let (nonce, challenge) = m.codes.remove(&f["code"]).unwrap();
                    assert_eq!(
                        B64.encode(Sha256::digest(f["code_verifier"].as_bytes())),
                        challenge
                    );
                    let t = db::now();
                    let id = sign(&j!({
                        "iss": m.issuer, "sub": m.sub, "aud": "filebrary", "iat": t, "exp": t + 300,
                        "nonce": nonce, "preferred_username": m.username, "groups": m.groups,
                    }));
                    async move {
                        Json(j!({ "access_token": "at", "token_type": "Bearer", "id_token": id }))
                    }
                }),
            );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    shared.lock().unwrap().issuer = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    shared
}

async fn sso_app(issuer: &str, extra: Value) -> (tempfile::TempDir, AppState, Router) {
    let dir = tempfile::tempdir().unwrap();
    let files = dir.path().join("files");
    std::fs::create_dir_all(&files).unwrap();
    let mut oidc = j!({
        "issuer_url": issuer, "client_id": "filebrary", "client_secret": SECRET,
        "admin_group": "admins"
    });
    for (k, v) in extra.as_object().unwrap() {
        oidc[k] = v.clone();
    }
    let config = Config {
        data_dir: dir.path().join("data"),
        public_url: Some("http://files.test".into()),
        sources: vec![SourceConfig::local("files", files)],
        oidc: Some(serde_json::from_value(oidc).unwrap()),
        ..Config::default()
    };
    config.validate().unwrap();
    std::fs::create_dir_all(&config.data_dir).unwrap();
    let state = AppState::new(config, db::open_memory().await.unwrap(), [7u8; 32]).unwrap();
    assert!(filebrary::identity::install(&state));
    let app = filebrary::app(state.clone());
    (dir, state, app)
}

/// Runs the browser side of a login; returns the callback response.
async fn sso_login(app: &Router, mock: &Shared, return_to: &str) -> axum::response::Response {
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/api/auth/oidc/login?return_to={return_to}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    let q = query_of(&loc);
    assert_eq!(
        q["redirect_uri"],
        "http://files.test/api/auth/oidc/callback"
    );
    let code = format!("c{}", q["state"]);
    mock.lock().unwrap().codes.insert(
        code.clone(),
        (q["nonce"].clone(), q["code_challenge"].clone()),
    );
    app.clone()
        .oneshot(
            Request::get(format!(
                "/api/auth/oidc/callback?code={code}&state={}",
                q["state"]
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn oidc_sign_in_creates_links_and_syncs() {
    let mock = start_mock().await;
    {
        let mut m = mock.lock().unwrap();
        m.sub = "sub-123".into();
        m.username = "gina".into();
        m.groups = vec!["admins".into(), "editors".into()];
    }
    let iss = mock.lock().unwrap().issuer.clone();
    let (_d, state, app) = sso_app(&iss, j!({})).await;
    let editors = filebrary::auth::groups::create(&state.db, "editors")
        .await
        .unwrap();
    filebrary::auth::groups::create(&state.db, "viewers")
        .await
        .unwrap();

    let (_, methods) = json(
        app.clone()
            .oneshot(
                Request::get("/api/auth/methods")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(methods["oidc"]["label"], "Sign in with SSO");
    assert_eq!(methods["password"], true);

    let resp = sso_login(&app, &mock, "/files/docs").await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()[header::LOCATION], "/files/docs");
    let cookie = resp.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let (st, me) = json(
        app.clone()
            .oneshot(
                Request::get("/api/me")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me["user"]["username"], "gina");
    assert_eq!(me["user"]["auth_source"], "oidc");
    assert_eq!(me["user"]["is_admin"], true);
    let groups: Vec<_> = me["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["name"].clone())
        .collect();
    assert_eq!(groups, [j!("editors")]);

    // Second login: same user (by sub), admin and groups follow the provider.
    mock.lock().unwrap().groups = vec![];
    mock.lock().unwrap().username = "renamed".into();
    let resp = sso_login(&app, &mock, "/").await;
    assert_eq!(resp.headers()[header::LOCATION], "/");
    let all = users::list(&state.db).await.unwrap();
    assert_eq!(all.iter().filter(|u| u.auth_source == "oidc").count(), 1);
    // No groups claim: memberships kept (only an explicit list is synced).
    assert_eq!(
        filebrary::auth::groups::members(&state.db, editors.id)
            .await
            .unwrap()
            .len(),
        1
    );

    // A local account with the provider's username is never taken over.
    users::create(&state.db, "hal", "a long enough password", false)
        .await
        .unwrap();
    {
        let mut m = mock.lock().unwrap();
        m.sub = "sub-hal".into();
        m.username = "hal".into();
    }
    let resp = sso_login(&app, &mock, "/").await;
    assert_eq!(resp.headers()[header::LOCATION], "/login?error=sso_failed");

    // An open redirect target is dropped.
    mock.lock().unwrap().sub = "sub-123".into();
    let resp = sso_login(&app, &mock, "//evil.example").await;
    assert_eq!(resp.headers()[header::LOCATION], "/");
}

#[tokio::test]
async fn oidc_without_auto_create_and_password_off() {
    let mock = start_mock().await;
    {
        let mut m = mock.lock().unwrap();
        m.sub = "sub-ivy".into();
        m.username = "ivy".into();
    }
    let iss = mock.lock().unwrap().issuer.clone();
    let (_d, state, app) = sso_app(
        &iss,
        j!({ "auto_create": false, "disable_password_login": true }),
    )
    .await;

    // Unknown users are refused when auto_create is off...
    let resp = sso_login(&app, &mock, "/").await;
    assert_eq!(resp.headers()[header::LOCATION], "/login?error=sso_failed");
    // ...until an administrator creates the account.
    users::create_external(&state.db, "ivy", "oidc", false)
        .await
        .unwrap();
    let resp = sso_login(&app, &mock, "/").await;
    assert_eq!(resp.headers()[header::LOCATION], "/");

    // Password login is off for non-admins.
    users::create(&state.db, "jo", "a long enough password", false)
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(login_req("jo", "a long enough password", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    users::create(&state.db, "boss", "a long enough password", true)
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(login_req("boss", "a long enough password", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
