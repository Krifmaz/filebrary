//! End-to-end OpenID Connect flow against a small in-process mock provider
//! that signs ID tokens with HS256 (the client secret), checks PKCE, and
//! serves groups only from its userinfo endpoint.

use std::{
    collections::HashMap,
    str::FromStr,
    sync::{Arc, Mutex},
};

use axum::{
    Form, Json, Router,
    extract::State,
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use filebrary_identity::{
    Error,
    oidc::{OidcConfig, OidcProvider},
};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");
const SECRET: &str = "a-client-secret-long-enough-for-hs256";

#[derive(Default)]
struct Mock {
    issuer: String,
    /// code -> (nonce, code_challenge)
    codes: HashMap<String, (String, String)>,
    tamper_nonce: bool,
}

type Shared = Arc<Mutex<Mock>>;

fn sign(claims: &Value) -> String {
    let header = B64.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let body = B64.encode(serde_json::to_vec(claims).unwrap());
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(format!("{header}.{body}").as_bytes());
    let sig = B64.encode(mac.finalize().into_bytes());
    format!("{header}.{body}.{sig}")
}

async fn discovery(State(m): State<Shared>) -> Json<Value> {
    let iss = m.lock().unwrap().issuer.clone();
    Json(json!({
        "issuer": iss,
        "authorization_endpoint": format!("{iss}/authorize"),
        "token_endpoint": format!("{iss}/token"),
        "userinfo_endpoint": format!("{iss}/userinfo"),
        "jwks_uri": format!("{iss}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["HS256"],
    }))
}

async fn token(
    State(m): State<Shared>,
    Form(f): Form<HashMap<String, String>>,
) -> Result<Json<Value>, axum::http::StatusCode> {
    let mut m = m.lock().unwrap();
    let (nonce, challenge) = m
        .codes
        .remove(&f["code"])
        .ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    let verifier = f
        .get("code_verifier")
        .ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    if B64.encode(Sha256::digest(verifier.as_bytes())) != challenge {
        return Err(axum::http::StatusCode::BAD_REQUEST);
    }
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let id = sign(&json!({
        "iss": m.issuer, "sub": "user-1", "aud": "filebrary",
        "iat": now, "exp": now + 300,
        "nonce": if m.tamper_nonce { "wrong".to_owned() } else { nonce },
        "preferred_username": "carol", "email": "carol@example.com",
        "email_verified": true, "name": "Carol",
    }));
    Ok(Json(json!({
        "access_token": "at-1", "token_type": "Bearer", "expires_in": 300, "id_token": id
    })))
}

async fn userinfo() -> Json<Value> {
    Json(json!({"sub": "user-1", "groups": ["staff", "admins"]}))
}

async fn start_mock() -> Shared {
    let shared: Shared = Arc::default();
    let app = Router::new()
        .route("/.well-known/openid-configuration", get(discovery))
        .route("/jwks", get(|| async { Json(json!({"keys": []})) }))
        .route("/token", post(token))
        .route("/userinfo", get(userinfo))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    shared.lock().unwrap().issuer = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    shared
}

/// Plays the provider's login page: reads the authorize URL, issues a code.
fn authorize(m: &Shared, url: &str) -> (String, String) {
    let u = openidconnect::url::Url::parse(url).unwrap();
    let q: HashMap<_, _> = u.query_pairs().into_owned().collect();
    assert_eq!(q["code_challenge_method"], "S256");
    assert!(q["scope"].split(' ').any(|s| s == "openid"));
    let code = format!("code-{}", q["state"].len() + m.lock().unwrap().codes.len());
    m.lock().unwrap().codes.insert(
        code.clone(),
        (q["nonce"].clone(), q["code_challenge"].clone()),
    );
    (code, q["state"].clone())
}

#[tokio::test]
async fn full_login_flow() {
    let mock = start_mock().await;
    let issuer = mock.lock().unwrap().issuer.clone();
    let db = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::from_str("sqlite::memory:").unwrap())
        .await
        .unwrap();
    MIGRATOR.run(&db).await.unwrap();

    let cfg: OidcConfig = serde_json::from_value(json!({
        "issuer_url": issuer, "client_id": "filebrary", "client_secret": SECRET,
        "admin_group": "admins", "allowed_groups": ["staff"]
    }))
    .unwrap();
    let p = OidcProvider::discover(cfg, "http://localhost:8080/api/auth/oidc/callback")
        .await
        .unwrap();

    let url = p.begin(&db, Some("/files/docs")).await.unwrap();
    let (code, state) = authorize(&mock, &url);
    let id = p.finish(&db, &code, &state).await.unwrap();
    assert_eq!(id.subject, "user-1");
    assert_eq!(id.issuer, issuer);
    assert_eq!(id.username, "carol");
    assert_eq!(id.email.as_deref(), Some("carol@example.com"));
    assert_eq!(id.groups, ["staff", "admins"]);
    assert!(id.is_admin);
    assert_eq!(id.return_to.as_deref(), Some("/files/docs"));

    // The state is single use.
    assert!(matches!(
        p.finish(&db, &code, &state).await,
        Err(Error::InvalidCredentials)
    ));
    // An open redirect target is dropped.
    let url = p.begin(&db, Some("//evil.example")).await.unwrap();
    let (code, state) = authorize(&mock, &url);
    assert_eq!(p.finish(&db, &code, &state).await.unwrap().return_to, None);

    // A token carrying the wrong nonce is rejected.
    mock.lock().unwrap().tamper_nonce = true;
    let url = p.begin(&db, None).await.unwrap();
    let (code, state) = authorize(&mock, &url);
    assert!(matches!(
        p.finish(&db, &code, &state).await,
        Err(Error::Oidc(_))
    ));
}
