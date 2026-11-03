//! Integration tests for browsing, file operations, downloads, zips and uploads,
//! run against a temporary directory through the full router.

mod common;

use std::io::Read;

use axum::{
    body::Body,
    http::{StatusCode, header},
};
use common::{TestApp, body_bytes, json};
use filebrary::auth::Perms;
use serde_json::json;

#[tokio::test]
async fn requires_login_and_csrf() {
    let t = TestApp::new().await;
    let anon = t
        .send(
            axum::http::Request::get("/api/sources")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(anon.status(), StatusCode::UNAUTHORIZED);

    let c = t.user("admin", true, Perms::ALL, "/").await;
    let ok = t.send(c.get("/api/sources")).await;
    let (s, v) = json(ok).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v[0]["id"], "files");
    assert_eq!(v[0]["permissions"]["delete"], true);

    // Unsafe method without the CSRF header is refused.
    let req = axum::http::Request::post("/api/sources/files/mkdir")
        .header(header::COOKIE, &c.cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"path":"/x"}"#))
        .unwrap();
    assert_eq!(t.send(req).await.status(), StatusCode::FORBIDDEN);
    assert!(!t.root().join("x").exists());
}

#[tokio::test]
async fn browse_and_operations() {
    let t = TestApp::new().await;
    let c = t.user("admin", true, Perms::ALL, "/").await;
    std::fs::write(t.root().join("b.txt"), b"bee").unwrap();

    let (s, _) = json(
        t.send(c.post_json("/api/sources/files/mkdir", json!({"path": "/Docs"})))
            .await,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = json(
        t.send(c.post_json("/api/sources/files/mkdir", json!({"path": "/Docs"})))
            .await,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);

    let (s, v) = json(t.send(c.get("/api/sources/files/list?path=/")).await).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["entries"][0]["name"], "Docs"); // folders first
    assert_eq!(v["entries"][0]["is_dir"], true);
    assert_eq!(v["entries"][1]["path"], "/b.txt");
    assert_eq!(v["entries"][1]["size"], 3);

    let (s, _) = json(
        t.send(c.post_json(
            "/api/sources/files/copy",
            json!({"from": "/b.txt", "to": "/Docs/c.txt"}),
        ))
        .await,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, v) = json(
        t.send(c.post_json(
            "/api/sources/files/move",
            json!({"from": "/b.txt", "to": "/Docs/c.txt"}),
        ))
        .await,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    let (s, _) = json(
        t.send(c.post_json(
            "/api/sources/files/move",
            json!({"from": "/b.txt", "to": "/Docs/d.txt"}),
        ))
        .await,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(t.root().join("Docs/d.txt").exists());

    let (s, v) = json(
        t.send(c.post_json(
            "/api/sources/files/delete",
            json!({"paths": ["/Docs/c.txt", "/nope"]}),
        ))
        .await,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["deleted"], json!(["/Docs/c.txt"]));
    assert_eq!(v["failed"][0]["path"], "/nope");
}

#[tokio::test]
async fn traversal_is_refused() {
    let t = TestApp::new().await;
    let c = t.user("admin", true, Perms::ALL, "/").await;
    std::fs::write(t.dir.path().join("outside.txt"), b"secret").unwrap();
    for uri in [
        "/api/sources/files/download?path=../outside.txt",
        "/api/sources/files/download?path=%2e%2e%2foutside.txt",
        "/api/sources/files/download?path=..%2Foutside.txt",
        "/api/sources/files/download?path=..%5Coutside.txt",
        "/api/sources/files/list?path=/../..",
        "/api/sources/files/download?path=%252e%252e%252foutside.txt",
    ] {
        let resp = t.send(c.get(uri)).await;
        assert!(
            resp.status() == StatusCode::BAD_REQUEST || resp.status() == StatusCode::NOT_FOUND,
            "{uri} -> {}",
            resp.status()
        );
        let b = body_bytes(resp).await;
        assert!(!b.windows(6).any(|w| w == b"secret"), "{uri} leaked data");
    }
    let (s, _) = json(
        t.send(c.post_json("/api/sources/files/mkdir", json!({"path": "/a/../../evil"})))
            .await,
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!t.dir.path().join("evil").exists());

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(t.dir.path(), t.root().join("link")).unwrap();
        let resp = t
            .send(c.get("/api/sources/files/download?path=/link/outside.txt"))
            .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn permissions_and_scope() {
    let t = TestApp::new().await;
    std::fs::create_dir_all(t.root().join("home/bob")).unwrap();
    std::fs::write(t.root().join("home/bob/mine.txt"), b"bob").unwrap();
    std::fs::write(t.root().join("top.txt"), b"top").unwrap();
    let bob = t
        .user("bob", false, Perms::VIEW | Perms::DOWNLOAD, "/home/bob")
        .await;

    let (s, v) = json(t.send(bob.get("/api/sources/files/list?path=/")).await).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["entries"].as_array().unwrap().len(), 1);
    assert_eq!(v["entries"][0]["path"], "/mine.txt");
    let resp = t
        .send(bob.get("/api/sources/files/download?path=/mine.txt"))
        .await;
    assert_eq!(body_bytes(resp).await.as_ref(), b"bob");
    // The scope is a root for him: top.txt is unreachable.
    let resp = t
        .send(bob.get("/api/sources/files/download?path=/top.txt"))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    // No upload or delete permission.
    let (s, _) = json(
        t.send(bob.post_json("/api/sources/files/mkdir", json!({"path": "/x"})))
            .await,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = json(
        t.send(bob.post_json("/api/sources/files/delete", json!({"paths": ["/mine.txt"]})))
            .await,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(t.root().join("home/bob/mine.txt").exists());

    // Without view the source is invisible.
    let eve = t.user("eve", false, Perms::default(), "/").await;
    let (_, v) = json(t.send(eve.get("/api/sources")).await).await;
    assert_eq!(v, json!([]));
    assert_eq!(
        t.send(eve.get("/api/sources/files/list")).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn download_ranges_and_headers() {
    let t = TestApp::new().await;
    let c = t.user("admin", true, Perms::ALL, "/").await;
    std::fs::write(t.root().join("page.html"), b"0123456789").unwrap();

    let resp = t
        .send(c.get("/api/sources/files/download?path=/page.html"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let h = resp.headers().clone();
    assert_eq!(h[header::ACCEPT_RANGES], "bytes");
    assert!(
        h[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("attachment")
    );
    assert!(
        h[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
    let etag = h[header::ETAG].clone();
    assert_eq!(body_bytes(resp).await.as_ref(), b"0123456789");

    let mut req = c.get("/api/sources/files/download?path=/page.html&inline=1");
    req.headers_mut()
        .insert(header::RANGE, "bytes=2-4".parse().unwrap());
    let resp = t.send(req).await;
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(resp.headers()[header::CONTENT_RANGE], "bytes 2-4/10");
    assert!(
        resp.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("inline")
    );
    assert_eq!(body_bytes(resp).await.as_ref(), b"234");

    let mut req = c.get("/api/sources/files/download?path=/page.html");
    req.headers_mut()
        .insert(header::RANGE, "bytes=20-".parse().unwrap());
    let resp = t.send(req).await;
    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(resp.headers()[header::CONTENT_RANGE], "bytes */10");

    let mut req = c.get("/api/sources/files/download?path=/page.html");
    req.headers_mut().insert(header::IF_NONE_MATCH, etag);
    assert_eq!(t.send(req).await.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn zip_of_selection() {
    let t = TestApp::new().await;
    let c = t.user("admin", true, Perms::ALL, "/").await;
    std::fs::create_dir_all(t.root().join("dir/sub")).unwrap();
    std::fs::write(t.root().join("dir/sub/a.txt"), b"aaa").unwrap();
    std::fs::write(t.root().join("dir/photo.jpg"), b"jpg").unwrap();
    std::fs::write(t.root().join("one.txt"), b"one".repeat(1000)).unwrap();

    let resp = t
        .send(c.get("/api/sources/files/zip?path=/dir&path=/one.txt&name=pack"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains("pack.zip")
    );
    let bytes = body_bytes(resp).await;
    let mut ar = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let mut names: Vec<String> = ar.file_names().map(|n| n.unwrap().into_owned()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "dir/",
            "dir/photo.jpg",
            "dir/sub/",
            "dir/sub/a.txt",
            "one.txt"
        ]
    );
    let mut s = String::new();
    ar.by_name("dir/sub/a.txt")
        .unwrap()
        .read_to_string(&mut s)
        .unwrap();
    assert_eq!(s, "aaa");
    let mut s = String::new();
    ar.by_name("one.txt")
        .unwrap()
        .read_to_string(&mut s)
        .unwrap();
    assert_eq!(s.len(), 3000);

    // Whole root.
    let resp = t.send(c.get("/api/sources/files/zip?path=/")).await;
    let bytes = body_bytes(resp).await;
    let ar = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    assert!(ar.file_names().any(|n| n.unwrap() == "one.txt"));

    let resp = t.send(c.get("/api/sources/files/zip")).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn chunked_resumable_upload() {
    let t = TestApp::new().await;
    let c = t.user("admin", true, Perms::ALL, "/").await;
    let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();

    let (s, v) = json(
        t.send(c.post_json(
            "/api/sources/files/uploads",
            json!({"path": "/folder/deep/file.bin", "size": data.len()}),
        ))
        .await,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let uid = v["id"].as_str().unwrap().to_owned();
    assert_eq!(v["offset"], 0);

    let patch = |off: usize, body: &[u8]| {
        c.req("PATCH", &format!("/api/uploads/{uid}"))
            .header("upload-offset", off.to_string())
            .body(Body::from(body.to_vec()))
            .unwrap()
    };
    let (s, v) = json(t.send(patch(0, &data[..100_000])).await).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["offset"], 100_000);
    // Wrong offset: 409 telling where to resume.
    let (s, v) = json(t.send(patch(50, &data[50..60])).await).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(v["offset"], 100_000);
    // Status lets a client resume after a disconnect.
    let (_, v) = json(t.send(c.get(&format!("/api/uploads/{uid}"))).await).await;
    assert_eq!(v["offset"], 100_000);
    // More than declared is refused.
    let mut too_much = data[100_000..].to_vec();
    too_much.push(0);
    let (s, _) = json(t.send(patch(100_000, &too_much)).await).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, v) = json(t.send(patch(100_000, &data[100_000..])).await).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["done"], true);
    assert_eq!(v["entry"]["path"], "/folder/deep/file.bin");
    assert_eq!(
        std::fs::read(t.root().join("folder/deep/file.bin")).unwrap(),
        data
    );
    // The upload is gone afterwards and the staging area is empty.
    assert_eq!(
        t.send(c.get(&format!("/api/uploads/{uid}"))).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(std::fs::read_dir(&t.state.upload_dir).unwrap().count(), 0);

    // Existing target without overwrite.
    let (s, _) = json(
        t.send(c.post_json(
            "/api/sources/files/uploads",
            json!({"path": "/folder/deep/file.bin", "size": 1}),
        ))
        .await,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    // Empty files complete immediately.
    let (s, v) = json(
        t.send(c.post_json(
            "/api/sources/files/uploads",
            json!({"path": "/empty", "size": 0}),
        ))
        .await,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["done"], true);
    assert!(t.root().join("empty").exists());
}

#[tokio::test]
async fn upload_limits_and_ownership() {
    let t = TestApp::new().await;
    let c = t.user("admin", true, Perms::ALL, "/").await;
    let other = t.user("other", false, Perms::ALL, "/").await;
    let size = 3 * 1024 * 1024;
    let (_, v) = json(
        t.send(c.post_json(
            "/api/sources/files/uploads",
            json!({"path": "/big", "size": size}),
        ))
        .await,
    )
    .await;
    let uid = v["id"].as_str().unwrap().to_owned();
    // Chunks above max_chunk_mib (1 MiB in tests) are refused.
    let resp = t
        .send(
            c.req("PATCH", &format!("/api/uploads/{uid}"))
                .header("upload-offset", "0")
                .body(Body::from(vec![0u8; 1024 * 1024 + 1]))
                .unwrap(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    // Another user cannot see or cancel it.
    assert_eq!(
        t.send(other.get(&format!("/api/uploads/{uid}")))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let resp = t
        .send(
            other
                .req("DELETE", &format!("/api/uploads/{uid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    // The owner can cancel it.
    let resp = t
        .send(
            c.req("DELETE", &format!("/api/uploads/{uid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    // Bad ids never touch the disk.
    assert_eq!(
        t.send(c.get("/api/uploads/..%2f..%2fx")).await.status(),
        StatusCode::NOT_FOUND
    );
}

/// Regression: a symlink inside a user's scope pointing elsewhere in the same
/// source must not let the user read or write outside the scope.
#[cfg(unix)]
#[tokio::test]
async fn symlink_cannot_escape_user_scope() {
    use std::os::unix::fs::symlink;
    let t = TestApp::new().await;
    std::fs::create_dir_all(t.root().join("home/bob")).unwrap();
    std::fs::create_dir_all(t.root().join("home/alice")).unwrap();
    std::fs::write(t.root().join("home/alice/private.txt"), b"alice-secret").unwrap();
    symlink(t.root().join("home/alice"), t.root().join("home/bob/peek")).unwrap();
    let bob = t.user("bob", false, Perms::ALL, "/home/bob").await;

    let resp = t
        .send(bob.get("/api/sources/files/download?path=/peek/private.txt"))
        .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(
        !body_bytes(resp)
            .await
            .windows(12)
            .any(|w| w == b"alice-secret")
    );
    let resp = t.send(bob.get("/api/sources/files/zip?path=/peek")).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let (s, _) = json(
        t.send(bob.post_json("/api/sources/files/mkdir", json!({"path": "/peek/planted"})))
            .await,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(!t.root().join("home/alice/planted").exists());
    let (s, _) = json(
        t.send(bob.post_json(
            "/api/sources/files/delete",
            json!({"paths": ["/peek/private.txt"]}),
        ))
        .await,
    )
    .await;
    // The whole batch is refused up front, since the link leads out of scope.
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(t.root().join("home/alice/private.txt").exists());
}
