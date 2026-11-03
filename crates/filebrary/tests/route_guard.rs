//! Every route refuses requests it should not serve.
//!
//! [`ROUTES`] lists every HTTP route the server registers, with who may use
//! it. For each one the test checks that:
//!
//! * a logged-out request is refused with 401 (unless the route is public);
//! * a logged-in user with no grants gets 403 or 404 from administrator and
//!   per-source routes;
//! * a read-only user gets 403 or 404 from every route that changes files;
//! * a cookie session without the CSRF header is refused with 403 on every
//!   state-changing route (even for an administrator).
//!
//! [`table_covers_every_registered_route`] scans the source for `.route(`
//! calls, so a new route that is not added to the table fails the build's
//! tests instead of slipping through unchecked.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use common::TestApp;
use filebrary::auth::Perms;

/// Who may use a route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Access {
    /// Anyone, by design (login page, public shares, health, docs).
    Public,
    /// Any logged-in user (their own account, settings, their own items).
    User,
    /// A user with a grant on the source named in the path.
    Source,
    /// A user with a grant on the source, changing files there (refused to
    /// read-only accounts).
    SourceWrite,
    /// Administrators only.
    Admin,
}

use Access::*;

/// `(method, concrete path, access)`. Path parameters are filled in: the
/// source is `files`, numeric ids are `1`, tokens and names are `t` or `n`.
const ROUTES: &[(&str, &str, Access)] = &[
    // first-run setup, login, account
    ("GET", "/api/setup", Public),
    ("POST", "/api/setup", Public),
    ("POST", "/api/auth/login", Public),
    ("POST", "/api/auth/logout", User),
    ("GET", "/api/auth/methods", Public),
    ("GET", "/api/auth/oidc/login", Public),
    ("GET", "/api/auth/oidc/callback", Public),
    ("GET", "/api/me", User),
    ("POST", "/api/me/password", User),
    ("GET", "/api/me/permissions?source=files", User),
    ("GET", "/api/me/preferences", User),
    ("PUT", "/api/me/preferences", User),
    ("GET", "/api/me/theme", User),
    ("PUT", "/api/me/theme", User),
    ("GET", "/api/me/totp", User),
    ("POST", "/api/me/totp/setup", User),
    ("POST", "/api/me/totp/confirm", User),
    ("POST", "/api/me/totp/recovery-codes", User),
    ("POST", "/api/me/totp/disable", User),
    ("GET", "/api/me/api-keys", User),
    ("POST", "/api/me/api-keys", User),
    ("DELETE", "/api/me/api-keys/1", User),
    ("GET", "/api/me/quota", User),
    // user and group administration
    ("GET", "/api/users", Admin),
    ("POST", "/api/users", Admin),
    ("GET", "/api/users/1", Admin),
    ("PUT", "/api/users/1", Admin),
    ("DELETE", "/api/users/1", Admin),
    ("PUT", "/api/users/1/home", Admin),
    ("GET", "/api/users/1/scopes", Admin),
    ("PUT", "/api/users/1/scopes", Admin),
    ("GET", "/api/users/1/groups", Admin),
    ("DELETE", "/api/users/1/totp", Admin),
    ("GET", "/api/users/1/api-keys", Admin),
    ("DELETE", "/api/api-keys/1", Admin),
    ("GET", "/api/users/1/quota", Admin),
    ("PUT", "/api/users/1/quota", Admin),
    ("POST", "/api/users/1/quota/rescan", Admin),
    ("GET", "/api/groups", Admin),
    ("POST", "/api/groups", Admin),
    ("DELETE", "/api/groups/1", Admin),
    ("GET", "/api/groups/1/members", Admin),
    ("PUT", "/api/groups/1/members/1", Admin),
    ("DELETE", "/api/groups/1/members/1", Admin),
    ("GET", "/api/permissions", Admin),
    ("PUT", "/api/permissions", Admin),
    ("DELETE", "/api/permissions/1", Admin),
    ("GET", "/api/admin/notifications", Admin),
    ("POST", "/api/admin/notifications/n/test", Admin),
    ("PUT", "/api/admin/branding", Admin),
    ("PUT", "/api/admin/branding/logo", Admin),
    ("DELETE", "/api/admin/branding/logo", Admin),
    ("POST", "/api/search/reindex", Admin),
    // files
    ("GET", "/api/sources", User),
    ("GET", "/api/sources/files/list?path=/", Source),
    ("GET", "/api/sources/files/stat?path=/a.txt", Source),
    ("GET", "/api/sources/files/download?path=/a.txt", Source),
    ("GET", "/api/sources/files/zip?path=/a.txt", Source),
    ("POST", "/api/sources/files/mkdir", SourceWrite),
    ("POST", "/api/sources/files/move", SourceWrite),
    ("POST", "/api/sources/files/copy", SourceWrite),
    ("POST", "/api/sources/files/delete", SourceWrite),
    ("GET", "/api/sources/files/text?path=/a.txt", Source),
    ("PUT", "/api/sources/files/text?path=/new.txt", SourceWrite),
    ("POST", "/api/sources/files/uploads", SourceWrite),
    ("GET", "/api/uploads/t", User),
    ("PATCH", "/api/uploads/t", User),
    ("DELETE", "/api/uploads/t", User),
    ("GET", "/api/previews", User),
    ("GET", "/api/sources/files/preview?path=/a.txt", Source),
    ("GET", "/api/sources/files/media?path=/a.txt", Source),
    ("GET", "/api/sources/files/stream?path=/a.txt", Source),
    // search
    ("GET", "/api/search?q=a", User),
    ("GET", "/api/search/status", User),
    // shares
    ("GET", "/api/shares", User),
    ("POST", "/api/shares", User),
    ("GET", "/api/shares/t", User),
    ("PATCH", "/api/shares/t", User),
    ("DELETE", "/api/shares/t", User),
    ("GET", "/api/public/shares/t", Public),
    ("POST", "/api/public/shares/t/unlock", Public),
    ("GET", "/api/public/shares/t/download", Public),
    ("POST", "/api/public/shares/t/uploads", Public),
    // organisation
    ("GET", "/api/trash", User),
    ("POST", "/api/trash", User),
    ("DELETE", "/api/trash", User),
    ("DELETE", "/api/trash/1", User),
    ("POST", "/api/trash/1/restore", User),
    ("GET", "/api/tags", User),
    ("POST", "/api/tags", User),
    ("GET", "/api/tags/file?source=files&path=/a.txt", User),
    ("PUT", "/api/tags/file", User),
    ("PATCH", "/api/tags/1", User),
    ("DELETE", "/api/tags/1", User),
    ("GET", "/api/tags/1/files", User),
    ("POST", "/api/tags/1/files", User),
    ("DELETE", "/api/tags/1/files", User),
    ("GET", "/api/stars", User),
    ("PUT", "/api/stars", User),
    ("DELETE", "/api/stars", User),
    ("GET", "/api/annotations?source=files&path=/", User),
    ("GET", "/api/activity", User),
    ("GET", "/api/comments?source=files&path=/a.txt", User),
    ("POST", "/api/comments", User),
    ("GET", "/api/comments/counts?source=files&path=/", User),
    ("PATCH", "/api/comments/1", User),
    ("DELETE", "/api/comments/1", User),
    ("GET", "/api/collections", User),
    ("POST", "/api/collections", User),
    ("GET", "/api/collections/1", User),
    ("PATCH", "/api/collections/1", User),
    ("DELETE", "/api/collections/1", User),
    ("POST", "/api/collections/1/items", User),
    ("DELETE", "/api/collections/1/items/1", User),
    // casting
    ("POST", "/api/cast", User),
    ("GET", "/api/public/cast/t/n", Public),
    // branding (the login page needs it)
    ("GET", "/api/branding", Public),
    ("GET", "/api/branding/logo", Public),
    // operations
    ("GET", "/api/health", Public),
    ("GET", "/api/openapi.json", Public),
    ("GET", "/api/docs", Public),
    ("GET", "/api/docs/docs.js", Public),
    ("GET", "/api/docs/docs.css", Public),
    ("GET", "/healthz", Public),
    ("GET", "/health", Public),
    ("GET", "/readyz", Public),
    // Prometheus scrapes without a session; set `metrics.token` to require
    // a bearer token (tested in tests/ops.rs).
    ("GET", "/metrics", Public),
    ("GET", "/manifest.webmanifest", Public),
];

fn is_unsafe(method: &str) -> bool {
    !matches!(method, "GET" | "HEAD" | "OPTIONS")
}

/// A well-formed body for routes that validate theirs before checking
/// permissions, so the test reaches the permission check.
fn body_for(uri: &str) -> &'static str {
    let path = uri.split('?').next().unwrap_or(uri);
    match path.rsplit('/').next().unwrap_or("") {
        "mkdir" => r#"{"path":"/new"}"#,
        "move" | "copy" => r#"{"from":"/a.txt","to":"/b.txt"}"#,
        "delete" if path.starts_with("/api/sources/") => r#"{"paths":["/a.txt"]}"#,
        "uploads" if path.starts_with("/api/sources/") => r#"{"path":"/u.bin","size":1}"#,
        "text" => "new text",
        _ => "{}",
    }
}

fn request(method: &str, uri: &str, cookie: Option<&str>, csrf: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if uri.contains("/text?") && method == "PUT" {
        // Create a new file: the editor's precondition header.
        b = b.header(header::IF_NONE_MATCH, "*");
    }
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    if let Some(t) = csrf {
        b = b.header("x-csrf-token", t);
    }
    if is_unsafe(method) {
        b.header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body_for(uri)))
            .unwrap()
    } else {
        b.body(Body::empty()).unwrap()
    }
}

#[tokio::test]
async fn logged_out_requests_are_refused() {
    let t = TestApp::new().await;
    let mut bad = Vec::new();
    for &(method, uri, access) in ROUTES {
        if access == Public {
            continue;
        }
        let s = t.send(request(method, uri, None, None)).await.status();
        if s != StatusCode::UNAUTHORIZED {
            bad.push(format!("{method} {uri}: {s}"));
        }
    }
    assert!(
        bad.is_empty(),
        "served without a session:\n{}",
        bad.join("\n")
    );
}

#[tokio::test]
async fn garbage_and_forged_cookies_are_refused() {
    let t = TestApp::new().await;
    let mut bad = Vec::new();
    for cookie in [
        "fb_session=",
        "fb_session=nope",
        "fb_session=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    ] {
        for &(method, uri, access) in ROUTES {
            if access == Public {
                continue;
            }
            let s = t
                .send(request(method, uri, Some(cookie), Some("x")))
                .await
                .status();
            if s != StatusCode::UNAUTHORIZED {
                bad.push(format!("{method} {uri} with {cookie:?}: {s}"));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "served with a bad cookie:\n{}",
        bad.join("\n")
    );
}

#[tokio::test]
async fn users_without_rights_are_refused() {
    let t = TestApp::new().await;
    std::fs::write(t.root().join("a.txt"), "hello").unwrap();
    // No rule at all: reaches nothing.
    let nobody = t.user("nobody", false, Perms::NONE, "/").await;
    let mut bad = Vec::new();
    for &(method, uri, access) in ROUTES {
        if !matches!(access, Admin | Source | SourceWrite) {
            continue;
        }
        let s = t
            .send(request(
                method,
                uri,
                Some(&nobody.cookie),
                Some(&nobody.csrf),
            ))
            .await
            .status();
        if !matches!(s, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND) {
            bad.push(format!("{method} {uri}: {s}"));
        }
    }
    assert!(
        bad.is_empty(),
        "served to a user without rights:\n{}",
        bad.join("\n")
    );
}

#[tokio::test]
async fn read_only_users_cannot_change_files() {
    let t = TestApp::new().await;
    std::fs::write(t.root().join("a.txt"), "hello").unwrap();
    let ro = t.user("reader", false, Perms::ALL, "/").await;
    sqlx::query("UPDATE users SET read_only = 1 WHERE username = 'reader'")
        .execute(&t.state.db)
        .await
        .unwrap();
    let mut bad = Vec::new();
    for &(method, uri, access) in ROUTES {
        if access != SourceWrite {
            continue;
        }
        let s = t
            .send(request(method, uri, Some(&ro.cookie), Some(&ro.csrf)))
            .await
            .status();
        if !matches!(s, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND) {
            bad.push(format!("{method} {uri}: {s}"));
        }
    }
    assert!(
        bad.is_empty(),
        "read-only user changed files:\n{}",
        bad.join("\n")
    );
    // Control: the same request from a regular user with the same rules works,
    // so the refusals above come from the role, not from a malformed request.
    let rw = t.user("writer", false, Perms::ALL, "/").await;
    for (method, uri) in [
        ("POST", "/api/sources/files/mkdir"),
        ("PUT", "/api/sources/files/text?path=/new.txt"),
    ] {
        let s = t
            .send(request(method, uri, Some(&rw.cookie), Some(&rw.csrf)))
            .await
            .status();
        assert!(s.is_success(), "{method} {uri} as a regular user: {s}");
    }
    // But reading works.
    let s = t
        .send(request(
            "GET",
            "/api/sources/files/download?path=/a.txt",
            Some(&ro.cookie),
            None,
        ))
        .await
        .status();
    assert_eq!(s, StatusCode::OK);
    // And a share link (which would let others download) is refused.
    let s = t
        .send(ro.post_json(
            "/api/shares",
            serde_json::json!({"source": "files", "paths": ["/a.txt"]}),
        ))
        .await
        .status();
    assert!(
        matches!(s, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "{s}"
    );
}

#[tokio::test]
async fn state_changes_need_the_csrf_token() {
    let t = TestApp::new().await;
    let admin = t.user("root", true, Perms::ALL, "/").await;
    let mut bad = Vec::new();
    for &(method, uri, access) in ROUTES {
        if access == Public || !is_unsafe(method) {
            continue;
        }
        for csrf in [None, Some("wrong")] {
            let s = t
                .send(request(method, uri, Some(&admin.cookie), csrf))
                .await
                .status();
            if s != StatusCode::FORBIDDEN {
                bad.push(format!("{method} {uri} csrf={csrf:?}: {s}"));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "changed state without CSRF:\n{}",
        bad.join("\n")
    );
}

#[tokio::test]
async fn webdav_needs_credentials() {
    let t = TestApp::new().await;
    for method in ["PROPFIND", "GET", "PUT", "DELETE", "MKCOL"] {
        let s = t
            .send(request(method, "/dav/files/", None, None))
            .await
            .status();
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{method} /dav/files/");
    }
}

/// Every `.route("…")` literal in the source must match a row of [`ROUTES`]
/// (after mapping path parameters), so the table stays exhaustive.
#[test]
fn table_covers_every_registered_route() {
    fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                rs_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    rs_files(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    // Table paths with parameters put back as `{}` and the query dropped.
    let table: Vec<String> = ROUTES
        .iter()
        .map(|(_, uri, _)| {
            let segs: Vec<&str> = uri.split('?').next().unwrap().split('/').collect();
            segs.iter()
                .enumerate()
                .map(|(i, seg)| match *seg {
                    "files" if i > 0 && segs[i - 1] == "sources" => "{}",
                    "1" | "t" | "n" => "{}",
                    s => s,
                })
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect();
    let mut missing = Vec::new();
    for f in files {
        let src = std::fs::read_to_string(&f).unwrap();
        let mut rest = src.as_str();
        while let Some(i) = rest.find(".route(") {
            rest = &rest[i + 7..];
            let lit = rest.trim_start();
            let Some(lit) = lit.strip_prefix('"') else {
                continue;
            };
            let path = &lit[..lit.find('"').unwrap()];
            // `{param}` → `{}`; a nested router's "/" is its prefix itself.
            let norm: String = path
                .split('/')
                .map(|seg| if seg.starts_with('{') { "{}" } else { seg })
                .collect::<Vec<_>>()
                .join("/");
            let norm = norm.trim_end_matches('/');
            let found = table.iter().any(|t| {
                t.ends_with(norm)
                    && (norm.is_empty()
                        || t.len() == norm.len()
                        || t.as_bytes()[t.len() - norm.len() - 1] != b'{')
            });
            if !found && !norm.is_empty() {
                missing.push(format!("{}: {path}", f.display()));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "routes missing from ROUTES:\n{}",
        missing.join("\n")
    );
}
