//! Integration tests for full-text content search: files in a temporary
//! directory are scanned by the file-name indexer, picked up by the content
//! worker through its event stream, and searched through `GET /api/search`
//! with `content=`, including permission scoping and the per-source switch.

use std::{path::Path, time::Duration};

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use filebrary::{
    AppState,
    auth::{Perms, Subject, permissions, session, users},
    config::{Config, SourceConfig, SourceKind},
    db,
    search::scan,
    storage::VPath,
};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Creates a user with VIEW on `files` (scoped) and on `other` (root), and
/// returns their session cookie value.
async fn user(state: &AppState, name: &str, scope: &str) -> String {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO users (username, password_hash, is_admin, created_at, updated_at) VALUES (?, 'x', 0, 0, 0) RETURNING id",
    )
    .bind(name)
    .fetch_one(&state.db)
    .await
    .unwrap();
    for (source, scope) in [("files", scope), ("other", "/")] {
        let scope = VPath::parse(scope).unwrap();
        permissions::set(&state.db, source, &scope, Subject::User(id), Perms::VIEW)
            .await
            .unwrap();
        users::set_scope(&state.db, id, source, &scope)
            .await
            .unwrap();
    }
    session::create(state, id, None, None).await.unwrap().token
}

struct Env {
    _tmp: tempfile::TempDir,
    state: AppState,
}

impl Env {
    async fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let files = tmp.path().join("files");
        let other = tmp.path().join("other");
        write(&files, "team/plan.md", "The zebra migration plan for 2027.");
        write(&files, "team/budget.csv", "item,cost\nzebra feed,120");
        write(
            &files,
            "private/diary.txt",
            "zebra secrets nobody should find",
        );
        write(&files, "team/photo.jpg", "zebra (not text, never indexed)");
        write(
            &other,
            "notes.txt",
            "zebra notes in a source with content search off",
        );
        let config = Config {
            data_dir: tmp.path().join("data"),
            sources: vec![
                SourceConfig {
                    name: "files".into(),
                    kind: SourceKind::Local,
                    path: files,
                    content_search: true,
                },
                SourceConfig {
                    name: "other".into(),
                    kind: SourceKind::Local,
                    path: other,
                    content_search: false,
                },
            ],
            ..Config::default()
        };
        let pool = db::open_memory().await.unwrap();
        let state = AppState::new(config, pool, [3; 32]).unwrap();
        assert!(state.content_index.is_some());
        filebrary::content::start(&state).unwrap();
        for name in ["files", "other"] {
            let src = state.source(name).unwrap();
            let root = std::fs::canonicalize(tmp.path().join(name)).unwrap();
            scan::full_scan(&state.search, src, Some(&root))
                .await
                .unwrap();
        }
        let env = Self { _tmp: tmp, state };
        env.wait_for_docs(3).await;
        env
    }

    async fn wait_for_docs(&self, n: u64) {
        let index = self.state.content_index.clone().unwrap();
        for _ in 0..200 {
            if index.doc_count(Some("files")).unwrap() >= n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("content index never reached {n} documents");
    }

    async fn get(&self, token: &str, uri: &str) -> (StatusCode, Value) {
        let req = Request::get(uri)
            .header(
                header::COOKIE,
                format!("{}={token}", session::SESSION_COOKIE),
            )
            .body(Body::empty())
            .unwrap();
        let resp = filebrary::app(self.state.clone())
            .oneshot(req)
            .await
            .unwrap();
        let status = resp.status();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    async fn hits(&self, token: &str, query: &str) -> Vec<(String, String, String)> {
        let (status, v) = self.get(token, &format!("/api/search?{query}")).await;
        assert_eq!(status, StatusCode::OK, "{query}: {v}");
        v["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| {
                (
                    h["source"].as_str().unwrap().to_owned(),
                    h["path"].as_str().unwrap().to_owned(),
                    h["snippet"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    }
}

#[tokio::test]
async fn content_search_end_to_end() {
    let e = Env::new().await;
    let all = user(&e.state, "all", "/").await;
    let team = user(&e.state, "team", "/team").await;

    // Text, CSV and the private file match; the JPEG and the source with
    // content search off do not.
    let mut paths: Vec<String> = e
        .hits(&all, "content=zebra")
        .await
        .into_iter()
        .map(|(s, p, _)| format!("{s}:{p}"))
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        [
            "files:/private/diary.txt",
            "files:/team/budget.csv",
            "files:/team/plan.md"
        ]
    );

    // Snippets are escaped HTML with highlighted terms.
    let hits = e.hits(&all, "content=migration").await;
    assert_eq!(hits.len(), 1);
    assert!(hits[0].2.contains("<b>migration</b>"), "{}", hits[0].2);

    // A scoped user only finds files inside their folder, relative to it.
    let mut paths: Vec<String> = e
        .hits(&team, "content=zebra")
        .await
        .into_iter()
        .map(|h| h.1)
        .collect();
    paths.sort();
    assert_eq!(paths, ["/budget.csv", "/plan.md"]);
    assert!(e.hits(&team, "content=secrets").await.is_empty());

    // A per-folder rule that removes view on /private hides its matches.
    let ruled = user(&e.state, "ruled", "/").await;
    let (rid,): (i64,) = sqlx::query_as("SELECT id FROM users WHERE username = 'ruled'")
        .fetch_one(&e.state.db)
        .await
        .unwrap();
    permissions::set(
        &e.state.db,
        "files",
        &VPath::parse("/private").unwrap(),
        Subject::User(rid),
        Perms::default(),
    )
    .await
    .unwrap();
    assert!(e.hits(&ruled, "content=secrets").await.is_empty());
    assert_eq!(e.hits(&ruled, "content=zebra").await.len(), 2);

    // Combined with name and type filters.
    let hits = e.hits(&all, "content=zebra&q=plan").await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].1, "/team/plan.md");
    let hits = e.hits(&all, "content=zebra&ext=csv").await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].1, "/team/budget.csv");

    // Phrases, paging and explicit sort.
    assert_eq!(
        e.hits(&all, "content=%22migration%20plan%22").await.len(),
        1
    );
    assert!(
        e.hits(&all, "content=%22plan%20migration%22")
            .await
            .is_empty()
    );
    assert_eq!(e.hits(&all, "content=zebra&limit=2").await.len(), 2);
    assert_eq!(
        e.hits(&all, "content=zebra&limit=2&offset=2").await.len(),
        1
    );
    let by_name: Vec<String> = e
        .hits(&all, "content=zebra&sort=name")
        .await
        .into_iter()
        .map(|h| h.1)
        .collect();
    assert_eq!(
        by_name,
        ["/team/budget.csv", "/private/diary.txt", "/team/plan.md"]
    );

    // Edits and deletes flow through the watcher/rescan events.
    let root = std::fs::canonicalize(e._tmp.path().join("files")).unwrap();
    std::fs::write(
        root.join("team/plan.md"),
        "Now about giraffes, longer text.",
    )
    .unwrap();
    std::fs::remove_file(root.join("private/diary.txt")).unwrap();
    let src = e.state.source("files").unwrap();
    scan::full_scan(&e.state.search, src, Some(&root))
        .await
        .unwrap();
    let mut found = false;
    for _ in 0..200 {
        if e.hits(&all, "content=giraffes").await.len() == 1 {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(found, "edited file was not re-indexed");
    assert_eq!(e.hits(&all, "content=zebra").await.len(), 1);
}

#[tokio::test]
async fn content_search_switched_off() {
    let tmp = tempfile::tempdir().unwrap();
    let mut config = Config {
        data_dir: tmp.path().join("data"),
        sources: vec![SourceConfig {
            name: "files".into(),
            kind: SourceKind::Local,
            path: tmp.path().join("files"),
            content_search: true,
        }],
        ..Config::default()
    };
    config.fulltext.enabled = false;
    let pool = db::open_memory().await.unwrap();
    let state = AppState::new(config, pool, [3; 32]).unwrap();
    assert!(state.content_index.is_none());
    assert!(filebrary::content::start(&state).is_none());
    let e = Env { _tmp: tmp, state };
    let token = user(&e.state, "u", "/").await;
    let (status, v) = e.get(&token, "/api/search?content=zebra").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{v}");
}
