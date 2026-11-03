//! Integration tests for the trash bin, tags, stars and the activity log,
//! driving the full router against a temporary directory.

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use filebrary::{
    AppState,
    auth::{Perms, session},
    config::{Config, SourceConfig, SourceKind},
    db,
    organize::{self, trash::TRASH_DIR},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

struct Client {
    cookie: String,
    csrf: String,
}

struct Env {
    _dir: TempDir,
    root: std::path::PathBuf,
    state: AppState,
    app: Router,
    admin: Client,
    bob: Client,
}

async fn env_with(trash_days: u64) -> Env {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("files");
    std::fs::create_dir_all(root.join("home/bob/docs")).unwrap();
    std::fs::write(root.join("home/bob/docs/a.txt"), b"hello").unwrap();
    std::fs::write(root.join("home/bob/b.txt"), b"bee").unwrap();
    std::fs::write(root.join("secret.txt"), b"admin only").unwrap();
    let config = Config {
        data_dir: dir.path().join("data"),
        trash_retention_days: trash_days,
        sources: vec![SourceConfig {
            name: "files".into(),
            kind: SourceKind::Local,
            path: root.clone(),
        }],
        ..Config::default()
    };
    std::fs::create_dir_all(&config.data_dir).unwrap();
    let pool = db::open_memory().await.unwrap();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, is_admin, created_at)
         VALUES (1, 'admin', 'x', 1, 0), (2, 'bob', 'x', 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let bob_perms = Perms::VIEW | Perms::DOWNLOAD | Perms::UPLOAD | Perms::EDIT | Perms::DELETE;
    sqlx::query("INSERT INTO user_sources (user_id, source, perms, scope) VALUES (2, 'files', ?, '/home/bob')")
        .bind(bob_perms.bits())
        .execute(&pool)
        .await
        .unwrap();
    let state = AppState::new(config, pool, [7; 32]).unwrap();
    let login = |id| {
        let state = state.clone();
        async move {
            let s = session::create(&state, id, None, None).await.unwrap();
            Client {
                cookie: format!("{}={}", session::SESSION_COOKIE, s.token),
                csrf: s.csrf_token,
            }
        }
    };
    let admin = login(1).await;
    let bob = login(2).await;
    Env {
        _dir: dir,
        root,
        app: filebrary::app(state.clone()),
        state,
        admin,
        bob,
    }
}

async fn env() -> Env {
    env_with(30).await
}

impl Env {
    async fn call(
        &self,
        who: &Client,
        method: Method,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::COOKIE, &who.cookie)
            .header("x-csrf-token", &who.csrf);
        let body = match body {
            Some(v) => {
                req = req.header(header::CONTENT_TYPE, "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let resp = self
            .app
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, v)
    }
}

#[tokio::test]
async fn trash_and_restore_round_trip() {
    let e = env().await;
    let (s, v) = e
        .call(
            &e.bob,
            Method::POST,
            "/api/trash",
            Some(json!({"source": "files", "paths": ["/docs"]})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let item = &v["trashed"][0];
    assert_eq!(item["path"], "/docs");
    assert_eq!(item["kind"], "dir");
    assert_eq!(item["deleted_by"], "bob");
    let id = item["id"].as_str().unwrap().to_owned();
    assert!(!e.root.join("home/bob/docs").exists());
    assert!(e.root.join(TRASH_DIR).join(&id).join("a.txt").exists());

    // Bob sees it; the listing is scoped.
    let (_, list) = e.call(&e.bob, Method::GET, "/api/trash", None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    // The admin sees the real path.
    let (_, list) = e
        .call(&e.admin, Method::GET, "/api/trash?source=files", None)
        .await;
    assert_eq!(list[0]["path"], "/home/bob/docs");

    // Restoring onto an existing path conflicts, then renames.
    std::fs::create_dir_all(e.root.join("home/bob/docs")).unwrap();
    let uri = format!("/api/trash/{id}/restore");
    let (s, _) = e.call(&e.bob, Method::POST, &uri, None).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, v) = e
        .call(
            &e.bob,
            Method::POST,
            &uri,
            Some(json!({"on_conflict": "rename"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["path"], "/docs (restored)");
    assert!(e.root.join("home/bob/docs (restored)/a.txt").exists());
    let (_, list) = e.call(&e.bob, Method::GET, "/api/trash", None).await;
    assert!(list.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn restore_recreates_missing_parents_and_to_elsewhere() {
    let e = env().await;
    let (_, v) = e
        .call(
            &e.bob,
            Method::POST,
            "/api/trash",
            Some(json!({"source": "files", "paths": ["docs/a.txt"]})),
        )
        .await;
    let id = v["trashed"][0]["id"].as_str().unwrap().to_owned();
    std::fs::remove_dir_all(e.root.join("home/bob/docs")).unwrap();
    let (s, v) = e
        .call(
            &e.bob,
            Method::POST,
            &format!("/api/trash/{id}/restore"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        std::fs::read(e.root.join("home/bob/docs/a.txt")).unwrap(),
        b"hello"
    );

    let (_, v) = e
        .call(
            &e.bob,
            Method::POST,
            "/api/trash",
            Some(json!({"source": "files", "paths": ["b.txt"]})),
        )
        .await;
    let id = v["trashed"][0]["id"].as_str().unwrap().to_owned();
    let (s, v) = e
        .call(
            &e.bob,
            Method::POST,
            &format!("/api/trash/{id}/restore"),
            Some(json!({"to": "/new/b2.txt"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(e.root.join("home/bob/new/b2.txt").exists());
}

#[tokio::test]
async fn trash_permissions_and_reserved_folder() {
    let e = env().await;
    // Bob cannot reach outside his scope: "../secret.txt" is refused by the parser,
    // and the admin's file is simply not in his tree.
    let (s, _) = e
        .call(
            &e.bob,
            Method::POST,
            "/api/trash",
            Some(json!({"source": "files", "paths": ["../secret.txt"]})),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // The admin trashes a root file; bob must not see or touch it.
    let (_, v) = e
        .call(
            &e.admin,
            Method::POST,
            "/api/trash",
            Some(json!({"source": "files", "paths": ["secret.txt"]})),
        )
        .await;
    let id = v["trashed"][0]["id"].as_str().unwrap().to_owned();
    let (_, list) = e.call(&e.bob, Method::GET, "/api/trash", None).await;
    assert!(list.as_array().unwrap().is_empty());
    for (m, uri) in [
        (Method::POST, format!("/api/trash/{id}/restore")),
        (Method::DELETE, format!("/api/trash/{id}")),
    ] {
        let (s, _) = e.call(&e.bob, m, &uri, None).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }
    // Emptying as bob leaves the admin's item alone.
    let (_, v) = e.call(&e.bob, Method::DELETE, "/api/trash", None).await;
    assert_eq!(v["purged"], 0);
    // Nobody can trash the trash folder or the root.
    let (s, _) = e
        .call(
            &e.admin,
            Method::POST,
            "/api/trash",
            Some(json!({"source": "files", "paths": [TRASH_DIR]})),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = e
        .call(
            &e.admin,
            Method::POST,
            "/api/trash",
            Some(json!({"source": "files", "paths": ["/"]})),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // CSRF is enforced.
    let resp = e
        .app
        .clone()
        .oneshot(
            Request::post("/api/trash")
                .header(header::COOKIE, &e.bob.cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"source": "files", "paths": ["b.txt"]}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    // The admin purges for good.
    let (s, _) = e
        .call(&e.admin, Method::DELETE, &format!("/api/trash/{id}"), None)
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert!(!e.root.join(TRASH_DIR).join(&id).exists());
}

#[tokio::test]
async fn auto_purge_and_disabled_trash() {
    let e = env().await;
    let (_, v) = e
        .call(
            &e.bob,
            Method::POST,
            "/api/trash",
            Some(json!({"source": "files", "paths": ["b.txt"]})),
        )
        .await;
    let id = v["trashed"][0]["id"].as_str().unwrap().to_owned();
    organize::maintain(&e.state).await;
    assert!(e.root.join(TRASH_DIR).join(&id).exists(), "not expired yet");
    sqlx::query("UPDATE trash SET deleted_at = deleted_at - 31 * 86400")
        .execute(&e.state.db)
        .await
        .unwrap();
    organize::maintain(&e.state).await;
    assert!(!e.root.join(TRASH_DIR).join(&id).exists());
    let (_, ev) = e
        .call(
            &e.admin,
            Method::GET,
            "/api/activity?action=trash.purge",
            None,
        )
        .await;
    assert_eq!(ev["items"][0]["path"], "/home/bob/b.txt");

    let e = env_with(0).await;
    let (_, v) = e
        .call(
            &e.bob,
            Method::POST,
            "/api/trash",
            Some(json!({"source": "files", "paths": ["b.txt"]})),
        )
        .await;
    assert_eq!(v["deleted"][0], "/b.txt");
    assert!(!e.root.join("home/bob/b.txt").exists());
    assert!(!e.root.join(TRASH_DIR).exists());
}

#[tokio::test]
async fn tags_follow_files_through_the_trash() {
    let e = env().await;
    let (s, v) = e
        .call(&e.bob, Method::PUT, "/api/tags/file", Some(json!({"source": "files", "path": "docs/a.txt", "tags": ["Work", "urgent", "work"]})))
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let names: Vec<_> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["urgent", "Work"]);
    let (s, _) = e
        .call(
            &e.bob,
            Method::PUT,
            "/api/stars",
            Some(json!({"source": "files", "path": "docs"})),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    let (_, ann) = e
        .call(
            &e.bob,
            Method::GET,
            "/api/annotations?source=files&path=/docs",
            None,
        )
        .await;
    assert_eq!(ann["a.txt"]["tags"].as_array().unwrap().len(), 2);
    let (_, ann) = e
        .call(
            &e.bob,
            Method::GET,
            "/api/annotations?source=files&path=/",
            None,
        )
        .await;
    assert_eq!(ann["docs"]["starred"], true);

    // Trash the folder: tags and star vanish from view, then come back.
    let (_, v) = e
        .call(
            &e.bob,
            Method::POST,
            "/api/trash",
            Some(json!({"source": "files", "paths": ["docs"]})),
        )
        .await;
    let id = v["trashed"][0]["id"].as_str().unwrap().to_owned();
    let (_, stars) = e.call(&e.bob, Method::GET, "/api/stars", None).await;
    assert!(stars.as_array().unwrap().is_empty());
    let (_, tags) = e.call(&e.bob, Method::GET, "/api/tags", None).await;
    let work = tags
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "Work")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let (_, files) = e
        .call(
            &e.bob,
            Method::GET,
            &format!("/api/tags/{work}/files"),
            None,
        )
        .await;
    assert!(files.as_array().unwrap().is_empty());

    e.call(
        &e.bob,
        Method::POST,
        &format!("/api/trash/{id}/restore"),
        None,
    )
    .await;
    let (_, stars) = e.call(&e.bob, Method::GET, "/api/stars", None).await;
    assert_eq!(stars[0]["path"], "/docs");
    let (_, files) = e
        .call(
            &e.bob,
            Method::GET,
            &format!("/api/tags/{work}/files"),
            None,
        )
        .await;
    assert_eq!(files[0]["path"], "/docs/a.txt");

    // Bulk remove, unstar.
    let (s, _) = e
        .call(
            &e.bob,
            Method::DELETE,
            &format!("/api/tags/{work}/files"),
            Some(json!({"source": "files", "paths": ["docs/a.txt"]})),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, t) = e
        .call(
            &e.bob,
            Method::GET,
            "/api/tags/file?source=files&path=docs/a.txt",
            None,
        )
        .await;
    assert_eq!(t.as_array().unwrap().len(), 1);
    e.call(
        &e.bob,
        Method::DELETE,
        "/api/stars?source=files&path=docs",
        None,
    )
    .await;
    let (_, stars) = e.call(&e.bob, Method::GET, "/api/stars", None).await;
    assert!(stars.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn tag_admin_rules() {
    let e = env().await;
    let (s, v) = e
        .call(
            &e.bob,
            Method::POST,
            "/api/tags",
            Some(json!({"name": "Holiday", "color": "#FF0000"})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(v["color"], "#ff0000");
    let id = v["id"].as_i64().unwrap();
    let (s, _) = e
        .call(
            &e.bob,
            Method::POST,
            "/api/tags",
            Some(json!({"name": "holiday"})),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = e
        .call(
            &e.bob,
            Method::PATCH,
            &format!("/api/tags/{id}"),
            Some(json!({"name": "x"})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v) = e
        .call(
            &e.admin,
            Method::PATCH,
            &format!("/api/tags/{id}"),
            Some(json!({"name": "Trips", "color": ""})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["color"], Value::Null);
    // Tagging a missing file or one outside the scope fails.
    let (s, _) = e
        .call(
            &e.bob,
            Method::POST,
            &format!("/api/tags/{id}/files"),
            Some(json!({"source": "files", "paths": ["nope.txt"]})),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = e
        .call(
            &e.bob,
            Method::POST,
            &format!("/api/tags/{id}/files"),
            Some(json!({"source": "files", "paths": ["b.txt"]})),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = e
        .call(&e.admin, Method::DELETE, &format!("/api/tags/{id}"), None)
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, t) = e
        .call(
            &e.bob,
            Method::GET,
            "/api/tags/file?source=files&path=b.txt",
            None,
        )
        .await;
    assert!(t.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn activity_is_scoped_and_paged() {
    let e = env().await;
    e.call(
        &e.admin,
        Method::POST,
        "/api/trash",
        Some(json!({"source": "files", "paths": ["secret.txt"]})),
    )
    .await;
    for p in ["b.txt", "docs/a.txt"] {
        e.call(
            &e.bob,
            Method::POST,
            "/api/trash",
            Some(json!({"source": "files", "paths": [p]})),
        )
        .await;
    }
    let (s, all) = e.call(&e.admin, Method::GET, "/api/activity", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(all["items"].as_array().unwrap().len(), 3);

    // Bob sees only his tree, with scoped paths.
    let (_, mine) = e.call(&e.bob, Method::GET, "/api/activity", None).await;
    let items = mine["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["path"], "/docs/a.txt");
    assert_eq!(items[0]["username"], "bob");
    assert_eq!(items[0]["action"], "trash");

    // Path filter (recursive) and paging.
    let (_, f) = e
        .call(
            &e.bob,
            Method::GET,
            "/api/activity?source=files&path=/docs",
            None,
        )
        .await;
    assert_eq!(f["items"].as_array().unwrap().len(), 1);
    let (_, p1) = e
        .call(&e.admin, Method::GET, "/api/activity?limit=2", None)
        .await;
    assert_eq!(p1["items"].as_array().unwrap().len(), 2);
    let before = p1["next_before"].as_i64().unwrap();
    let (_, p2) = e
        .call(
            &e.admin,
            Method::GET,
            &format!("/api/activity?limit=2&before={before}"),
            None,
        )
        .await;
    assert_eq!(p2["items"].as_array().unwrap().len(), 1);
    assert_eq!(p2["next_before"], Value::Null);
    let (s, _) = e
        .call(&e.bob, Method::GET, "/api/activity?path=/x", None)
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

/// A symlink inside bob's folder that points elsewhere in the source must not
/// let him trash, restore into or tag files outside his scope.
#[cfg(unix)]
#[tokio::test]
async fn symlinks_cannot_escape_the_scope() {
    let e = env().await;
    std::fs::create_dir_all(e.root.join("private")).unwrap();
    std::fs::write(e.root.join("private/p.txt"), b"private").unwrap();
    std::os::unix::fs::symlink(e.root.join("private"), e.root.join("home/bob/link")).unwrap();

    let (s, _) = e
        .call(
            &e.bob,
            Method::POST,
            "/api/trash",
            Some(json!({"source": "files", "paths": ["link/p.txt"]})),
        )
        .await;
    assert!(
        s == StatusCode::FORBIDDEN || s == StatusCode::NOT_FOUND,
        "{s}"
    );
    assert!(e.root.join("private/p.txt").exists());

    let (s, _) = e
        .call(
            &e.bob,
            Method::PUT,
            "/api/tags/file",
            Some(json!({"source": "files", "path": "link/p.txt", "tags": ["x"]})),
        )
        .await;
    assert!(
        s == StatusCode::FORBIDDEN || s == StatusCode::NOT_FOUND,
        "{s}"
    );
    let (s, _) = e
        .call(
            &e.bob,
            Method::PUT,
            "/api/stars",
            Some(json!({"source": "files", "path": "link/p.txt"})),
        )
        .await;
    assert!(
        s == StatusCode::FORBIDDEN || s == StatusCode::NOT_FOUND,
        "{s}"
    );

    // Restoring through the link is refused and the item stays in the trash.
    let (_, v) = e
        .call(
            &e.bob,
            Method::POST,
            "/api/trash",
            Some(json!({"source": "files", "paths": ["b.txt"]})),
        )
        .await;
    let id = v["trashed"][0]["id"].as_str().unwrap().to_owned();
    let (s, _) = e
        .call(
            &e.bob,
            Method::POST,
            &format!("/api/trash/{id}/restore"),
            Some(json!({"to": "link/b.txt"})),
        )
        .await;
    assert!(
        s == StatusCode::FORBIDDEN || s == StatusCode::NOT_FOUND,
        "{s}"
    );
    assert!(!e.root.join("private/b.txt").exists());
    let (_, list) = e.call(&e.bob, Method::GET, "/api/trash", None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    // Trashing the link itself moves the link, never its target.
    let (s, v) = e
        .call(
            &e.bob,
            Method::POST,
            "/api/trash",
            Some(json!({"source": "files", "paths": ["link"]})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(e.root.join("private/p.txt").exists());
    assert!(!e.root.join("home/bob/link").exists());
}

/// The files routes use the trash, keep tags attached and log activity.
#[tokio::test]
async fn files_routes_are_wired_in() {
    let e = env().await;
    e.call(
        &e.bob,
        Method::PUT,
        "/api/tags/file",
        Some(json!({"source": "files", "path": "b.txt", "tags": ["keep"]})),
    )
    .await;
    let (s, v) = e
        .call(
            &e.bob,
            Method::POST,
            "/api/sources/files/move",
            Some(json!({"from": "/b.txt", "to": "/docs/c.txt"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, t) = e
        .call(
            &e.bob,
            Method::GET,
            "/api/tags/file?source=files&path=/docs/c.txt",
            None,
        )
        .await;
    assert_eq!(t[0]["name"], "keep");
    let (s, v) = e
        .call(
            &e.bob,
            Method::POST,
            "/api/sources/files/copy",
            Some(json!({"from": "/docs/c.txt", "to": "/d.txt"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, t) = e
        .call(
            &e.bob,
            Method::GET,
            "/api/tags/file?source=files&path=/d.txt",
            None,
        )
        .await;
    assert_eq!(t[0]["name"], "keep");

    // Delete goes to the trash, which the admin's root listing hides.
    let (s, v) = e
        .call(
            &e.bob,
            Method::POST,
            "/api/sources/files/delete",
            Some(json!({"paths": ["/docs/c.txt"]})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["deleted"][0], "/docs/c.txt");
    let (_, list) = e.call(&e.bob, Method::GET, "/api/trash", None).await;
    assert_eq!(list[0]["path"], "/docs/c.txt");
    let (_, listing) = e
        .call(
            &e.admin,
            Method::GET,
            "/api/sources/files/list?path=/",
            None,
        )
        .await;
    assert!(
        listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["name"] != TRASH_DIR),
        "{listing}"
    );
    let (s, _) = e
        .call(
            &e.admin,
            Method::GET,
            &format!("/api/sources/files/list?path=/{TRASH_DIR}"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = e
        .call(
            &e.admin,
            Method::POST,
            "/api/sources/files/move",
            Some(json!({"from": "/secret.txt", "to": format!("/{TRASH_DIR}/x")})),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let (_, ev) = e.call(&e.bob, Method::GET, "/api/activity", None).await;
    let actions: Vec<_> = ev["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["trash", "copy", "move", "tag"]);
    assert_eq!(ev["items"][2]["target"], "/docs/c.txt");
}
