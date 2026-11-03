//! Integration tests for the editor endpoints (`/api/sources/{id}/text`),
//! driven in-process against a temporary directory.

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use filebrary::{
    AppState,
    auth::{Perms, session},
    config::{Config, SourceConfig, SourceKind},
    db,
};
use serde_json::Value;
use tower::ServiceExt;

struct Harness {
    app: Router,
    root: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

struct Login {
    cookie: String,
    csrf: String,
}

/// Builds an app with one source `s` and two users: `alice` (edit +
/// upload, scoped to `/home`) and `bob` (view + download only).
async fn setup() -> (Harness, Login, Login) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("files");
    std::fs::create_dir_all(root.join("home")).unwrap();
    let config = Config {
        data_dir: dir.path().join("data"),
        sources: vec![SourceConfig {
            name: "s".into(),
            kind: SourceKind::Local,
            path: root.clone(),
        }],
        ..Config::default()
    };
    let pool = db::open_memory().await.unwrap();
    let state = AppState::new(config, pool, [7; 32]).unwrap();

    let mut logins = Vec::new();
    for (name, perms, scope) in [
        (
            "alice",
            Perms::VIEW | Perms::DOWNLOAD | Perms::UPLOAD | Perms::EDIT,
            "/home",
        ),
        ("bob", Perms::VIEW | Perms::DOWNLOAD, "/"),
    ] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, created_at) VALUES (?, 'x', 0) RETURNING id",
        )
        .bind(name)
        .fetch_one(&state.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO user_sources (user_id, source, perms, scope) VALUES (?, 's', ?, ?)",
        )
        .bind(id)
        .bind(perms.bits())
        .bind(scope)
        .execute(&state.db)
        .await
        .unwrap();
        let s = session::create(&state, id, None, None).await.unwrap();
        logins.push(Login {
            cookie: format!("{}={}", session::SESSION_COOKIE, s.token),
            csrf: s.csrf_token,
        });
    }
    let bob = logins.pop().unwrap();
    let alice = logins.pop().unwrap();
    let app = filebrary::app(state);
    (
        Harness {
            app,
            root,
            _dir: dir,
        },
        alice,
        bob,
    )
}

impl Harness {
    async fn send(&self, req: Request<Body>) -> (StatusCode, Option<String>, Value) {
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let etag = resp
            .headers()
            .get(header::ETAG)
            .map(|v| v.to_str().unwrap().to_owned());
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, etag, json)
    }

    async fn get(&self, who: &Login, path: &str) -> (StatusCode, Option<String>, Value) {
        self.send(
            Request::get(format!("/api/sources/s/text?path={path}"))
                .header(header::COOKIE, &who.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    async fn put(
        &self,
        who: &Login,
        path: &str,
        pre: Option<(header::HeaderName, &str)>,
        body: impl Into<Body>,
    ) -> (StatusCode, Option<String>, Value) {
        let mut b = Request::put(format!("/api/sources/s/text?path={path}"))
            .header(header::COOKIE, &who.cookie)
            .header("x-csrf-token", &who.csrf);
        if let Some((k, v)) = pre {
            b = b.header(k, v);
        }
        self.send(b.body(body.into()).unwrap()).await
    }
}

#[tokio::test]
async fn load_edit_and_conflict() {
    let (h, alice, _) = setup().await;
    std::fs::write(h.root.join("home/notes.md"), "# Hi\r\n").unwrap();

    let (st, etag, body) = h.get(&alice, "/notes.md").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["content"], "# Hi\r\n");
    assert_eq!(body["path"], "/notes.md");
    let etag = etag.unwrap();
    assert_eq!(body["etag"], etag.as_str());

    // Save with the right ETag.
    let (st, new_tag, body) = h
        .put(
            &alice,
            "/notes.md",
            Some((header::IF_MATCH, &etag)),
            "# Hello\n",
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let new_tag = new_tag.unwrap();
    assert_ne!(new_tag, etag);
    assert_eq!(body["size"], 8);
    assert_eq!(
        std::fs::read_to_string(h.root.join("home/notes.md")).unwrap(),
        "# Hello\n"
    );

    // A second editor still holding the old ETag gets a conflict, with the
    // current ETag so it can choose to overwrite.
    let (st, _, body) = h
        .put(
            &alice,
            "/notes.md",
            Some((header::IF_MATCH, &etag)),
            "stale",
        )
        .await;
    assert_eq!(st, StatusCode::PRECONDITION_FAILED);
    assert_eq!(body["error"], "precondition_failed");
    assert_eq!(body["etag"], new_tag.as_str());
    assert_eq!(
        std::fs::read_to_string(h.root.join("home/notes.md")).unwrap(),
        "# Hello\n"
    );

    // A change made outside Filebrary is detected too.
    std::fs::write(h.root.join("home/notes.md"), "changed on disk").unwrap();
    let (st, _, _) = h
        .put(&alice, "/notes.md", Some((header::IF_MATCH, &new_tag)), "x")
        .await;
    assert_eq!(st, StatusCode::PRECONDITION_FAILED);
}

#[tokio::test]
async fn save_requires_a_precondition() {
    let (h, alice, _) = setup().await;
    std::fs::write(h.root.join("home/a.txt"), "a").unwrap();
    let (st, _, body) = h.put(&alice, "/a.txt", None, "b").await;
    assert_eq!(st, StatusCode::PRECONDITION_REQUIRED);
    assert_eq!(body["error"], "precondition_required");
    assert_eq!(
        std::fs::read_to_string(h.root.join("home/a.txt")).unwrap(),
        "a"
    );
}

#[tokio::test]
async fn create_new_file() {
    let (h, alice, _) = setup().await;
    let (st, etag, body) = h
        .put(
            &alice,
            "/new.rs",
            Some((header::IF_NONE_MATCH, "*")),
            "fn main() {}\n",
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    assert!(etag.is_some());
    assert_eq!(
        std::fs::read_to_string(h.root.join("home/new.rs")).unwrap(),
        "fn main() {}\n"
    );
    // Creating it again fails: it exists now.
    let (st, _, body) = h
        .put(
            &alice,
            "/new.rs",
            Some((header::IF_NONE_MATCH, "*")),
            "again",
        )
        .await;
    assert_eq!(st, StatusCode::PRECONDITION_FAILED);
    assert!(body["etag"].is_string());
}

#[tokio::test]
async fn refuses_binary_and_bad_input() {
    let (h, alice, _) = setup().await;
    std::fs::write(h.root.join("home/bin.dat"), [0u8, 159, 146, 150]).unwrap();
    let (st, _, body) = h.get(&alice, "/bin.dat").await;
    assert_eq!(st, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(body["error"], "not_text");

    let (st, _, _) = h.get(&alice, "/missing.txt").await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, _, _) = h.get(&alice, "/").await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, _, _) = h
        .put(
            &alice,
            "/x.txt",
            Some((header::IF_NONE_MATCH, "*")),
            vec![0xffu8, 0xfe],
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // Too large.
    let big = vec![b'a'; usize::try_from(filebrary::api::text::MAX_TEXT_BYTES).unwrap() + 1];
    let (st, _, _) = h
        .put(&alice, "/big.txt", Some((header::IF_NONE_MATCH, "*")), big)
        .await;
    assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(!h.root.join("home/big.txt").exists());
}

#[tokio::test]
async fn path_safety_and_scope() {
    let (h, alice, _) = setup().await;
    std::fs::write(h.root.join("secret.txt"), "top secret").unwrap();
    // Alice is confined to /home; `..` is refused, not resolved.
    for p in [
        "/../secret.txt",
        "..%2Fsecret.txt",
        "/home/..%2F..%2Fsecret.txt",
    ] {
        let (st, _, body) = h.get(&alice, p).await;
        assert!(
            st == StatusCode::BAD_REQUEST || st == StatusCode::NOT_FOUND,
            "{p}: {st} {body}"
        );
        assert_ne!(body["content"], "top secret");
    }
    #[cfg(unix)]
    {
        // A symlink leading outside the source root is refused.
        let outside = h.root.parent().unwrap().join("outside.txt");
        std::fs::write(&outside, "top secret").unwrap();
        std::os::unix::fs::symlink(&outside, h.root.join("home/link.txt")).unwrap();
        let (st, _, body) = h.get(&alice, "/link.txt").await;
        assert_ne!(st, StatusCode::OK, "{body}");
        let (st, _, _) = h
            .put(
                &alice,
                "/link.txt",
                Some((header::IF_MATCH, "\"x\"")),
                "pwn",
            )
            .await;
        assert_ne!(st, StatusCode::OK);
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "top secret");
    }
}

#[tokio::test]
async fn permissions_and_csrf() {
    let (h, alice, bob) = setup().await;
    std::fs::write(h.root.join("home/a.txt"), "a").unwrap();
    // Bob can read but not edit or create.
    let (st, etag, _) = h.get(&bob, "/home/a.txt").await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, _) = h
        .put(
            &bob,
            "/home/a.txt",
            Some((header::IF_MATCH, &etag.unwrap())),
            "b",
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _, _) = h
        .put(&bob, "/home/b.txt", Some((header::IF_NONE_MATCH, "*")), "b")
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // No CSRF header.
    let (st, _, _) = h
        .send(
            Request::put("/api/sources/s/text?path=/a.txt")
                .header(header::COOKIE, &alice.cookie)
                .header(header::IF_NONE_MATCH, "*")
                .body(Body::from("x"))
                .unwrap(),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // No session.
    let (st, _, _) = h
        .send(
            Request::get("/api/sources/s/text?path=/a.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}
