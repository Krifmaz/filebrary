//! Integration tests for the search index: scanning a temporary directory,
//! the watcher, the HTTP API, permissions and path safety.

use std::{path::Path, time::Duration};

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use filebrary::{
    AppState,
    auth::{Perms, session},
    config::{Config, SourceConfig, SourceKind},
    db,
    search::scan,
};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

struct Env {
    _tmp: tempfile::TempDir,
    root: std::path::PathBuf,
    state: AppState,
    admin: String,
    bob: String,
}

fn write(root: &Path, rel: &str, bytes: usize) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, vec![b'x'; bytes]).unwrap();
}

async fn user(state: &AppState, name: &str, admin: bool, grant: Option<(Perms, &str)>) -> String {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO users (username, password_hash, is_admin, created_at) VALUES (?, 'x', ?, 0) RETURNING id",
    )
    .bind(name)
    .bind(admin)
    .fetch_one(&state.db)
    .await
    .unwrap();
    if let Some((perms, scope)) = grant {
        sqlx::query(
            "INSERT INTO user_sources (user_id, source, perms, scope) VALUES (?, 'files', ?, ?)",
        )
        .bind(id)
        .bind(perms.bits())
        .bind(scope)
        .execute(&state.db)
        .await
        .unwrap();
    }
    session::create(state, id, None, None).await.unwrap().token
}

async fn env(watch: bool) -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("files");
    write(&root, "photos/beach.JPG", 2048);
    write(&root, "photos/2024/party.png", 10);
    write(&root, "docs/report-final.pdf", 5000);
    write(&root, "docs/notes.md", 100);
    write(&root, "home/bob/todo.txt", 3);
    write(&root, "home/bob/photos/cat.jpg", 40);
    write(&root, ".filebrary-trash/1/deleted-report.pdf", 1);
    std::fs::create_dir_all(root.join("empty")).unwrap();
    let mut config = Config {
        data_dir: tmp.path().join("data"),
        sources: vec![SourceConfig {
            name: "files".into(),
            kind: SourceKind::Local,
            path: root.clone(),
        }],
        ..Config::default()
    };
    config.search.watch = watch;
    let pool = db::open_memory().await.unwrap();
    let state = AppState::new(config, pool, [7; 32]).unwrap();
    let admin = user(&state, "admin", true, None).await;
    let bob = user(&state, "bob", false, Some((Perms::VIEW, "/home/bob"))).await;
    let root = std::fs::canonicalize(&root).unwrap();
    Env {
        _tmp: tmp,
        root,
        state,
        admin,
        bob,
    }
}

impl Env {
    async fn scan(&self) -> scan::ScanStats {
        let src = self.state.source("files").unwrap();
        scan::full_scan(&self.state.search, src, Some(&self.root))
            .await
            .unwrap()
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

    async fn paths(&self, token: &str, query: &str) -> Vec<String> {
        let (status, v) = self.get(token, &format!("/api/search?{query}")).await;
        assert_eq!(status, StatusCode::OK, "{query}: {v}");
        v["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["path"].as_str().unwrap().to_owned())
            .collect()
    }
}

#[tokio::test]
async fn scans_and_filters() {
    let e = env(false).await;
    let stats = e.scan().await;
    // 7 folders (photos, photos/2024, docs, home, home/bob, home/bob/photos, empty) + 6 files.
    assert_eq!(stats.seen, 13);
    assert_eq!(stats.changed, 13);

    let a = &e.admin;
    assert_eq!(e.paths(a, "q=report").await, ["/docs/report-final.pdf"]);
    assert_eq!(e.paths(a, "q=REP%20fin").await, ["/docs/report-final.pdf"]);
    // Short terms use LIKE.
    assert_eq!(e.paths(a, "q=md").await, ["/docs/notes.md"]);
    assert_eq!(
        e.paths(a, "type=image&sort=path").await,
        [
            "/home/bob/photos/cat.jpg",
            "/photos/2024/party.png",
            "/photos/beach.JPG"
        ]
    );
    assert_eq!(
        e.paths(a, "ext=jpg&sort=path").await,
        ["/home/bob/photos/cat.jpg", "/photos/beach.JPG"]
    );
    assert_eq!(
        e.paths(a, "min_size=2k&type=file&sort=path").await,
        ["/docs/report-final.pdf", "/photos/beach.JPG"]
    );
    assert_eq!(
        e.paths(a, "max_size=10&type=file&sort=path").await,
        ["/home/bob/todo.txt", "/photos/2024/party.png"]
    );
    assert_eq!(
        e.paths(a, "q=photos&type=dir&sort=path").await,
        ["/home/bob/photos", "/photos"]
    );
    assert_eq!(
        e.paths(a, "source=files&path=/photos&sort=path").await,
        [
            "/photos/2024",
            "/photos/2024/party.png",
            "/photos/beach.JPG"
        ]
    );
    assert!(e.paths(a, "after=2999-01-01").await.is_empty());
    assert_eq!(
        e.paths(a, "before=2999-01-01&q=notes").await,
        ["/docs/notes.md"]
    );
    // The trash folder is never indexed.
    assert!(e.paths(a, "q=deleted").await.is_empty());
    assert!(e.paths(a, "q=filebrary-trash").await.is_empty());

    // Paging.
    let (_, v) = e.get(a, "/api/search?type=file&limit=2").await;
    assert_eq!(v["hits"].as_array().unwrap().len(), 2);
    assert_eq!(v["has_more"], true);
    let (_, v) = e.get(a, "/api/search?type=file&limit=2&offset=4").await;
    assert_eq!(v["hits"].as_array().unwrap().len(), 2);
    assert_eq!(v["has_more"], false);

    // A hit carries type, size, mime and an RFC 3339 time.
    let (_, v) = e.get(a, "/api/search?q=beach").await;
    let h = &v["hits"][0];
    assert_eq!(h["type"], "image");
    assert_eq!(h["size"], 2048);
    assert_eq!(h["mime"], "image/jpeg");
    assert!(h["modified"].as_str().unwrap().contains('T'));
}

#[tokio::test]
async fn results_respect_scope_and_permissions() {
    let e = env(false).await;
    e.scan().await;
    // Bob only sees his scope, with paths relative to it.
    assert_eq!(
        e.paths(&e.bob, "type=file&sort=path").await,
        ["/photos/cat.jpg", "/todo.txt"]
    );
    assert!(e.paths(&e.bob, "q=report").await.is_empty());
    assert_eq!(
        e.paths(&e.bob, "source=files&path=/photos").await,
        ["/photos/cat.jpg"]
    );
    // His scope folder itself is not a result.
    assert!(e.paths(&e.bob, "q=bob").await.is_empty());

    // Traversal attempts are refused before any query runs.
    for q in [
        "source=files&path=/../docs",
        "source=files&path=..",
        "source=files&path=a/%2e%2e/..",
        "source=files&path=a%5C..%5Cb",
    ] {
        let (status, v) = e.get(&e.bob, &format!("/api/search?{q}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{q}: {v}");
    }
    // An encoded ".." arrives as the literal name "%2e%2e" after one decode,
    // which is simply a folder that does not exist.
    assert!(
        e.paths(&e.bob, "source=files&path=%252e%252e")
            .await
            .is_empty()
    );

    // Unknown or ungranted sources look the same.
    let (status, _) = e.get(&e.bob, "/api/search?source=nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Anonymous callers are rejected.
    let (status, _) = e.get("bogus", "/api/search?q=x").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Status is visible, errors only to admins; reindex is admin-only.
    let (status, v) = e.get(&e.bob, "/api/search/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v[0]["source"], "files");
    assert_eq!(v[0]["entries"], 13);
}

#[tokio::test]
async fn rescans_pick_up_changes_and_sweep() {
    let e = env(false).await;
    e.scan().await;
    let mut rx = e.state.search.subscribe();
    std::fs::remove_dir_all(e.root.join("photos")).unwrap();
    write(&e.root, "docs/notes.md", 101); // size change
    write(&e.root, "new/fresh.txt", 1);
    let stats = e.scan().await;
    assert_eq!(stats.removed, 4); // photos, photos/2024 and two images
    assert_eq!(stats.changed, 3); // notes.md, new, new/fresh.txt
    assert!(e.paths(&e.admin, "q=beach").await.is_empty());
    assert_eq!(e.paths(&e.admin, "q=fresh").await, ["/new/fresh.txt"]);

    let mut kinds = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        kinds.push(
            serde_json::to_value(&ev).unwrap()["event"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    assert_eq!(kinds.iter().filter(|k| *k == "upsert").count(), 3);
    assert_eq!(kinds.iter().filter(|k| *k == "remove").count(), 4);
    assert_eq!(kinds.last().unwrap(), "scan_done");
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_are_safe() {
    let e = env(false).await;
    let outside = e._tmp.path().join("outside");
    write(&outside, "secret-plans.txt", 1);
    std::os::unix::fs::symlink(&outside, e.root.join("escape")).unwrap();
    std::os::unix::fs::symlink(outside.join("secret-plans.txt"), e.root.join("leak.txt")).unwrap();
    // A loop back to the root must not hang or recurse.
    std::os::unix::fs::symlink(&e.root, e.root.join("docs/loop")).unwrap();
    std::os::unix::fs::symlink(e.root.join("docs/notes.md"), e.root.join("notes-link.md")).unwrap();
    e.scan().await;
    assert!(e.paths(&e.admin, "q=secret").await.is_empty());
    assert!(e.paths(&e.admin, "q=escape").await.is_empty());
    assert!(e.paths(&e.admin, "q=leak").await.is_empty());
    assert_eq!(e.paths(&e.admin, "q=loop").await, ["/docs/loop"]);
    assert_eq!(
        e.paths(&e.admin, "q=notes&sort=path").await,
        ["/docs/notes.md", "/notes-link.md"]
    );
    assert_eq!(e.paths(&e.admin, "q=party").await.len(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_never_leave_the_users_scope() {
    let e = env(false).await;
    let bob = e.root.join("home/bob");
    // Links inside Bob's scope that point elsewhere in the source.
    std::os::unix::fs::symlink(
        e.root.join("docs/report-final.pdf"),
        bob.join("stolen-report.pdf"),
    )
    .unwrap();
    std::os::unix::fs::symlink(e.root.join("docs"), bob.join("stolen-docs")).unwrap();
    std::os::unix::fs::symlink(&e.root, bob.join("stolen-root")).unwrap();
    // A link that stays inside his scope is fine.
    std::os::unix::fs::symlink(bob.join("todo.txt"), bob.join("todo-link.txt")).unwrap();
    e.scan().await;

    assert!(e.paths(&e.bob, "q=stolen").await.is_empty());
    assert!(e.paths(&e.bob, "q=report").await.is_empty());
    assert!(e.paths(&e.bob, "q=notes").await.is_empty());
    assert_eq!(
        e.paths(&e.bob, "q=todo&sort=path").await,
        ["/todo-link.txt", "/todo.txt"]
    );
    assert_eq!(
        e.paths(&e.bob, "source=files&path=/photos").await,
        ["/photos/cat.jpg"]
    );
    // The admin sees the links (but nothing *through* them: linked folders are not walked).
    assert_eq!(
        e.paths(&e.admin, "q=stolen&sort=path").await,
        [
            "/home/bob/stolen-docs",
            "/home/bob/stolen-report.pdf",
            "/home/bob/stolen-root"
        ]
    );
    assert_eq!(
        e.paths(&e.admin, "q=report&sort=path").await,
        ["/docs/report-final.pdf", "/home/bob/stolen-report.pdf"]
    );
}

#[tokio::test]
async fn tag_filter() {
    let e = env(false).await;
    e.scan().await;
    // Without the tag tables (another feature's migration) the filter is refused.
    let (status, _) = e.get(&e.admin, "/api/search?tag=x").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    for sql in [
        "CREATE TABLE IF NOT EXISTS tags (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE COLLATE NOCASE)",
        "CREATE TABLE IF NOT EXISTS file_tags (tag_id INTEGER, source TEXT, path TEXT, PRIMARY KEY (tag_id, source, path))",
        "INSERT INTO tags (id, name) VALUES (1, 'Holiday'), (2, 'best')",
        "INSERT INTO file_tags VALUES (1, 'files', '/photos/beach.JPG'), (2, 'files', '/photos/beach.JPG'),
                                      (1, 'files', '/photos/2024/party.png'), (1, 'files', '/home/bob/photos/cat.jpg')",
    ] {
        sqlx::query(sql).execute(&e.state.db).await.unwrap();
    }
    assert_eq!(
        e.paths(&e.admin, "tag=holiday&sort=path").await,
        [
            "/home/bob/photos/cat.jpg",
            "/photos/2024/party.png",
            "/photos/beach.JPG"
        ]
    );
    assert_eq!(
        e.paths(&e.admin, "tag=holiday,BEST").await,
        ["/photos/beach.JPG"]
    );
    assert_eq!(e.paths(&e.bob, "tag=holiday").await, ["/photos/cat.jpg"]);
}

#[tokio::test]
async fn bad_filters_are_rejected() {
    let e = env(false).await;
    for q in [
        "type=pictures",
        "sort=random()",
        "limit=0",
        "min_size=lots",
        "after=yesterday",
        "path=/docs",
        "content=hello",
        "unknown=1",
    ] {
        let (status, v) = e.get(&e.admin, &format!("/api/search?{q}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{q}: {v}");
        assert_eq!(v["error"], "bad_request");
    }
}

/// Polls until `f` holds or ten seconds pass.
async fn eventually<F: AsyncFn() -> bool>(what: &str, f: F) {
    for _ in 0..100 {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for: {what}");
}

#[tokio::test]
async fn watcher_keeps_the_index_fresh() {
    let e = env(true).await;
    e.state.search.start(&e.state);
    eventually("initial scan", async || {
        e.state.search.status()[0].last_scan.is_some()
    })
    .await;
    assert!(e.state.search.status()[0].watching);

    write(&e.root, "docs/added-later.txt", 5);
    eventually("new file indexed", async || {
        e.paths(&e.admin, "q=added-later").await == ["/docs/added-later.txt"]
    })
    .await;

    // A folder created with contents in one go is walked.
    write(&e.root, "music/album/track01.flac", 5);
    eventually("new folder walked", async || {
        e.paths(&e.admin, "q=track01").await == ["/music/album/track01.flac"]
    })
    .await;

    // Renaming a folder moves its subtree.
    std::fs::rename(e.root.join("docs"), e.root.join("papers")).unwrap();
    eventually("rename followed", async || {
        e.paths(&e.admin, "q=report").await == ["/papers/report-final.pdf"]
    })
    .await;

    // Deleting removes the subtree.
    std::fs::remove_dir_all(e.root.join("papers")).unwrap();
    eventually("delete followed", async || {
        e.paths(&e.admin, "q=notes").await.is_empty()
    })
    .await;

    // Changes inside the trash stay out of the index.
    write(&e.root, ".filebrary-trash/2/trashed-thing.txt", 1);
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(e.paths(&e.admin, "q=trashed").await.is_empty());
}
