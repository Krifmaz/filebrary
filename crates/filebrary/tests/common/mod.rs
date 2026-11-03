//! Helpers to drive the full application in-process against a temporary directory.
#![allow(dead_code)]

use axum::{
    Router,
    body::{Body, Bytes},
    http::{Request, Response, StatusCode, header},
};
use filebrary::{
    AppState,
    auth::{Perms, session},
    config::{Config, SourceConfig, SourceKind},
    db,
};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

/// A running test application.
pub struct TestApp {
    pub dir: tempfile::TempDir,
    pub state: AppState,
    pub app: Router,
}

/// A logged-in client.
pub struct Client {
    pub cookie: String,
    pub csrf: String,
}

impl TestApp {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let files = dir.path().join("files");
        std::fs::create_dir_all(&files).unwrap();
        let config = Config {
            data_dir: dir.path().join("data"),
            sources: vec![SourceConfig {
                name: "files".into(),
                kind: SourceKind::Local,
                path: files,
                content_search: false,
            }],
            max_chunk_mib: 1,
            ..Config::default()
        };
        std::fs::create_dir_all(&config.data_dir).unwrap();
        let pool = db::open_memory().await.unwrap();
        let state = AppState::new(config, pool, [7u8; 32]).unwrap();
        let app = filebrary::app(state.clone());
        Self { dir, state, app }
    }

    pub fn root(&self) -> std::path::PathBuf {
        self.dir.path().join("files")
    }

    /// Creates a user (admin, or with `perms` on "files" within `scope`) and logs in.
    pub async fn user(&self, name: &str, admin: bool, perms: Perms, scope: &str) -> Client {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, created_at) VALUES (?, 'x', ?, 0) RETURNING id",
        )
        .bind(name)
        .bind(admin)
        .fetch_one(&self.state.db)
        .await
        .unwrap();
        if !admin {
            sqlx::query("INSERT INTO user_sources (user_id, source, perms, scope) VALUES (?, 'files', ?, ?)")
                .bind(id)
                .bind(perms.bits())
                .bind(scope)
                .execute(&self.state.db)
                .await
                .unwrap();
        }
        let s = session::create(&self.state, id, None, None).await.unwrap();
        Client {
            cookie: format!("{}={}", session::SESSION_COOKIE, s.token),
            csrf: s.csrf_token,
        }
    }

    pub async fn send(&self, req: Request<Body>) -> Response<Body> {
        self.app.clone().oneshot(req).await.unwrap()
    }
}

impl Client {
    pub fn get(&self, uri: &str) -> Request<Body> {
        Request::get(uri)
            .header(header::COOKIE, &self.cookie)
            .body(Body::empty())
            .unwrap()
    }

    pub fn req(&self, method: &str, uri: &str) -> axum::http::request::Builder {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::COOKIE, &self.cookie)
            .header("x-csrf-token", &self.csrf)
    }

    pub fn post_json(&self, uri: &str, body: Value) -> Request<Body> {
        self.req("POST", uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }
}

pub async fn body_bytes(resp: Response<Body>) -> Bytes {
    resp.into_body().collect().await.unwrap().to_bytes()
}

pub async fn json(resp: Response<Body>) -> (StatusCode, Value) {
    let status = resp.status();
    let b = body_bytes(resp).await;
    (status, serde_json::from_slice(&b).unwrap_or(Value::Null))
}
