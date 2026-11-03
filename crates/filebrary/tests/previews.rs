//! HTTP tests for `/api/previews` and `/api/sources/{id}/preview`, against a
//! temporary data directory.

mod common;

use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode, header},
};
use filebrary::{AppState, auth::Perms};
use http_body_util::BodyExt;
use image::{DynamicImage, Rgb, RgbImage};
use tower::ServiceExt;

struct Fixture {
    app: common::TestApp,
    files: std::path::PathBuf,
    state: AppState,
}

async fn fixture() -> Fixture {
    let app = common::TestApp::new().await;
    // Canonical path, so symlink tests compare like with like.
    let files = std::fs::canonicalize(app.root()).unwrap();
    let state = app.state.clone();
    Fixture { app, files, state }
}

/// Creates a user with `perms` on the `files` source and returns a cookie.
async fn login(f: &Fixture, name: &str, perms: Perms) -> String {
    login_scoped(f, name, perms, "/").await
}

/// Like [`login`], confined to the folder `scope`.
async fn login_scoped(f: &Fixture, name: &str, perms: Perms, scope: &str) -> String {
    f.app.user(name, false, perms, scope).await.cookie
}

async fn get(
    state: &AppState,
    uri: &str,
    cookie: &str,
    etag: Option<&str>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut req = Request::get(uri);
    if !cookie.is_empty() {
        req = req.header(header::COOKIE, cookie);
    }
    if let Some(e) = etag {
        req = req.header(header::IF_NONE_MATCH, e);
    }
    let resp = filebrary::app(state.clone())
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, headers, body)
}

fn write_photo(path: &std::path::Path) {
    DynamicImage::ImageRgb8(RgbImage::from_pixel(1000, 500, Rgb([10, 200, 30])))
        .save(path)
        .unwrap();
}

#[tokio::test]
async fn thumbnail_round_trip_with_etag() {
    let f = fixture().await;
    write_photo(&f.files.join("cat photo.jpg"));
    let cookie = login(&f, "alice", Perms::VIEW | Perms::DOWNLOAD).await;
    let uri = "/api/sources/files/preview?path=/cat%20photo.jpg";

    let (st, h, body) = get(&f.state, uri, &cookie, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(h[header::CONTENT_TYPE], "image/jpeg");
    let img = image::load_from_memory(&body).unwrap();
    assert_eq!((img.width(), img.height()), (256, 128));

    let etag = h[header::ETAG].to_str().unwrap().to_owned();
    let (st, _, body) = get(&f.state, uri, &cookie, Some(&etag)).await;
    assert_eq!(st, StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());

    // JPEG is browser-native: the large variant is the original file.
    let (st, h, body) = get(&f.state, &format!("{uri}&variant=large"), &cookie, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body, std::fs::read(f.files.join("cat photo.jpg")).unwrap());
    assert!(
        h[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
}

#[tokio::test]
async fn previews_need_download_permission() {
    let f = fixture().await;
    write_photo(&f.files.join("a.jpg"));
    let viewer = login(&f, "bob", Perms::VIEW).await;
    let uri = "/api/sources/files/preview?path=/a.jpg";
    let (st, _, _) = get(&f.state, uri, &viewer, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _, _) = get(&f.state, uri, "", None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn traversal_and_symlinks_are_refused() {
    let f = fixture().await;
    let cookie = login(&f, "carol", Perms::VIEW | Perms::DOWNLOAD).await;
    for path in [
        "/../secret.jpg",
        "/%2e%2e/secret.jpg",
        "/a/../../secret.jpg",
    ] {
        let uri = format!("/api/sources/files/preview?path={path}");
        let (st, _, _) = get(&f.state, &uri, &cookie, None).await;
        assert!(st.is_client_error(), "{path} gave {st}");
    }
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        write_photo(&outside.path().join("secret.jpg"));
        std::os::unix::fs::symlink(outside.path().join("secret.jpg"), f.files.join("link.jpg"))
            .unwrap();
        let uri = "/api/sources/files/preview?path=/link.jpg";
        let (st, _, _) = get(&f.state, uri, &cookie, None).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn unsupported_files_say_no_preview() {
    let f = fixture().await;
    std::fs::write(f.files.join("notes.txt"), "hi").unwrap();
    let cookie = login(&f, "dan", Perms::VIEW | Perms::DOWNLOAD).await;
    let uri = "/api/sources/files/preview?path=/notes.txt";
    let (st, _, body) = get(&f.state, uri, &cookie, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"], "no_preview");
    let (st, _, _) = get(&f.state, &format!("{uri}&variant=huge"), &cookie, None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn capabilities_report_image_support() {
    let f = fixture().await;
    let cookie = login(&f, "eve", Perms::VIEW).await;
    let (st, _, body) = get(&f.state, "/api/previews", &cookie, None).await;
    assert_eq!(st, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["enabled"], true);
    assert_eq!(v["kinds"]["image"], true);
    assert_eq!(v["kinds"]["raw"], true);
    assert_eq!(v["thumb_size"], 256);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_cannot_leave_the_users_scope() {
    let f = fixture().await;
    std::fs::create_dir_all(f.files.join("alice")).unwrap();
    std::fs::create_dir_all(f.files.join("bob")).unwrap();
    write_photo(&f.files.join("bob/private.jpg"));
    write_photo(&f.files.join("alice/mine.jpg"));
    // Inside the source, but outside alice's folder.
    std::os::unix::fs::symlink(
        f.files.join("bob/private.jpg"),
        f.files.join("alice/peek.jpg"),
    )
    .unwrap();
    let cookie = login_scoped(&f, "alice", Perms::VIEW | Perms::DOWNLOAD, "/alice").await;

    let (st, _, _) = get(
        &f.state,
        "/api/sources/files/preview?path=/mine.jpg",
        &cookie,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    for route in ["preview", "media", "stream"] {
        let uri = format!("/api/sources/files/{route}?path=/peek.jpg");
        let (st, _, _) = get(&f.state, &uri, &cookie, None).await;
        assert!(
            st == StatusCode::FORBIDDEN || st == StatusCode::NOT_FOUND,
            "{route} gave {st}"
        );
    }
}
