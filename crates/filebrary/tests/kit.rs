//! The kit's server crates on Filebrary's accounts (see `src/kit/`).

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{TestApp, json};
use filebrary::auth::Perms;
use serde_json::{Value, json as j};

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn health_is_the_kits_answer_on_every_old_path() {
    let t = TestApp::new().await;
    for path in ["/api/health", "/healthz", "/health"] {
        let (s, v) = json(t.send(get(path)).await).await;
        assert_eq!(s, StatusCode::OK, "{path}");
        assert_eq!(v["ok"], true, "{path}");
        assert_eq!(v["status"], "ok", "{path}: old probes read `status`");
        assert_eq!(v["server"], "rust");
        assert_eq!(v["version"], filebrary::kit::VERSION);
    }
}

#[tokio::test]
async fn kit_routes_use_filebrary_sessions_and_roles() {
    let t = TestApp::new().await;
    let admin = t.user("ana", true, Perms::ALL, "/").await;
    let member = t.user("bo", false, Perms::ALL, "/").await;

    // Signed-in routes take a Filebrary session; no session is 401.
    assert_eq!(
        t.send(get("/api/changelog")).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let (s, _) = json(t.send(member.get("/api/changelog")).await).await;
    assert_eq!(s, StatusCode::OK);

    // Admin routes: an administrator yes, a member 403.
    assert_eq!(
        t.send(member.get("/api/diagnostics")).await.status(),
        StatusCode::FORBIDDEN
    );
    let (s, v) = json(t.send(admin.get("/api/diagnostics")).await).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let names: Vec<&str> = v["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    for want in [
        "Data folder",
        "Database",
        "Settings database",
        "Source files",
    ] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }

    // A cookie session needs the CSRF header on unsafe requests.
    let no_csrf = Request::put("/api/auth/prefs")
        .header(header::COOKIE, &member.cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"whats_new_popup":false}"#))
        .unwrap();
    assert_eq!(t.send(no_csrf).await.status(), StatusCode::FORBIDDEN);
    let put = member
        .req("PUT", "/api/auth/prefs")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"whats_new_popup":false}"#))
        .unwrap();
    let (s, v) = json(t.send(put).await).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, v) = json(t.send(member.get("/api/auth/prefs")).await).await;
    assert_eq!(v["whats_new_popup"], false);
}

#[tokio::test]
async fn settings_schema_and_the_update_card() {
    let t = TestApp::new().await;
    let admin = t.user("ana", true, Perms::ALL, "/").await;
    let (s, v) = json(t.send(admin.get("/api/settings/schema")).await).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v.to_string().contains("update_check"), "{v}");

    // Switch the check off, so the card answers without asking GitHub.
    let put = admin
        .req("PUT", "/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"update_check":false}"#))
        .unwrap();
    let (s, v) = json(t.send(put).await).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(!t.state.kit.settings.bool("update_check"));

    let (s, v) = json(t.send(admin.get("/api/update")).await).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["enabled"], false);
    assert_eq!(v["current"], filebrary::kit::VERSION);
    assert_eq!(v["can_update"], false);
    assert_eq!(v["is_newer"], false);

    // Docker images are updated by pulling: "Update now" says how.
    let (s, v) = json(t.send(admin.post_json("/api/update", j!({}))).await).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert!(
        v["detail"]
            .as_str()
            .unwrap()
            .contains("docker compose pull")
    );
}

#[tokio::test]
async fn notices_and_crash_logs() {
    let t = TestApp::new().await;
    let admin = t.user("ana", true, Perms::ALL, "/").await;
    let member = t.user("bo", false, Perms::ALL, "/").await;

    let (s, v) = json(t.send(admin.post_json("/api/notices/check", j!({}))).await).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = json(t.send(member.get("/api/notices")).await).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // Everything is fine in a fresh test server: no banner shows.
    let shown = |v: &Value| v.to_string().contains("can't be reached");
    assert!(!shown(&v), "{v}");

    // A source that vanished shows the sources banner after a check.
    std::fs::remove_dir_all(t.root()).unwrap();
    let (s, v) = json(
        t.send(admin.post_json("/api/notices/sources/check", j!({})))
            .await,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(shown(&v), "{v}");

    let (s, v) = json(
        t.send(member.post_json(
            "/api/crash-reports",
            j!({"summary": "boom", "text": "stack"}),
        ))
        .await,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = json(t.send(admin.get("/api/crash-reports")).await).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v.to_string().contains("boom"), "{v}");
}
