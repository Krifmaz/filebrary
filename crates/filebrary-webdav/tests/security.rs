//! Path safety and permission tests: traversal in request URLs and
//! `Destination` headers, scoped users, symlinks, and every permission.

mod common;

use axum::http::StatusCode;
use common::Dav;

#[tokio::test]
async fn traversal_in_request_urls_is_refused() {
    let d = Dav::new();
    std::fs::write(d.dir_b.path().join("secret"), "b-secret").unwrap();
    for path in [
        "/dav/a/../b/secret",
        "/dav/a/%2e%2e/b/secret",
        "/dav/a/%2E%2E/b/secret",
        "/dav/a/..%2fb%2fsecret",
        "/dav/a/%2e%2e%2fb%2fsecret",
        "/dav/a/..%5cb%5csecret",
        "/dav/a/x%00/secret",
        "/dav/..",
    ] {
        for method in ["GET", "PUT", "DELETE", "PROPFIND", "MKCOL"] {
            let r = d
                .req(Some("bob"), method, path, &[("depth", "0")], "")
                .await;
            assert!(
                r.status == StatusCode::FORBIDDEN || r.status == StatusCode::BAD_REQUEST,
                "{method} {path}: {}",
                r.status
            );
            assert!(!r.body.contains("b-secret"));
        }
    }
    assert!(d.dir_b.path().join("secret").exists());
}

#[tokio::test]
async fn traversal_in_destination_is_refused() {
    let d = Dav::new();
    d.req(Some("scoped"), "PUT", "/dav/a/f", &[], "x").await;
    for dest in [
        "/dav/a/../a/escaped",
        "/dav/a/%2e%2e/escaped",
        "http://localhost/dav/a/..%2fescaped",
        "/dav/a/sub%2f..%2f..%2fescaped",
    ] {
        for m in ["COPY", "MOVE"] {
            let r = d
                .req(Some("scoped"), m, "/dav/a/f", &[("destination", dest)], "")
                .await;
            assert!(
                r.status == StatusCode::FORBIDDEN || r.status == StatusCode::BAD_REQUEST,
                "{m} -> {dest}: {}",
                r.status
            );
        }
    }
    // Nothing landed outside the user's scope.
    assert!(!d.dir_a.path().join("escaped").exists());
    assert!(d.dir_a.path().join("home/f").exists());
}

#[tokio::test]
async fn scoped_user_stays_in_scope() {
    let d = Dav::new();
    std::fs::write(d.dir_a.path().join("top-secret"), "secret").unwrap();
    // scoped sees a/home as the root of source a.
    let r = d
        .req(Some("scoped"), "PUT", "/dav/a/mine.txt", &[], "mine")
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert!(d.dir_a.path().join("home/mine.txt").exists());
    let r = d
        .req(Some("scoped"), "GET", "/dav/a/top-secret", &[], "")
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = d
        .req(Some("scoped"), "PROPFIND", "/dav/a/", &[("depth", "1")], "")
        .await;
    assert_eq!(r.hrefs(), vec!["/dav/a/", "/dav/a/mine.txt"]);
    // Their own root cannot be deleted or moved.
    assert_eq!(
        d.req(Some("scoped"), "DELETE", "/dav/a/", &[], "")
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    // Source b is invisible.
    assert_eq!(
        d.req(Some("scoped"), "PROPFIND", "/dav/b/", &[("depth", "0")], "")
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn locks_are_shared_across_scopes() {
    let d = Dav::new();
    d.alice("PUT", "/dav/a/home/shared.txt", &[], "x").await;
    // alice (unscoped) locks the file that scoped sees as /shared.txt.
    let token = d.lock("alice", "/dav/a/home/shared.txt", "0", false).await;
    let r = d
        .req(Some("scoped"), "PUT", "/dav/a/shared.txt", &[], "y")
        .await;
    assert_eq!(r.status, StatusCode::LOCKED);
    // scoped sees the lock, with the lock root mapped into their view.
    let r = d
        .req(
            Some("scoped"),
            "PROPFIND",
            "/dav/a/shared.txt",
            &[("depth", "0")],
            r#"<D:propfind xmlns:D="DAV:"><D:prop><D:lockdiscovery/></D:prop></D:propfind>"#,
        )
        .await;
    assert!(r.body.contains(&token));
    assert!(
        r.body
            .contains("<D:href>/dav/a/shared.txt</D:href></D:lockroot>")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_out_of_scope_are_refused() {
    use std::os::unix::fs::symlink;
    let d = Dav::new();
    std::fs::write(d.dir_a.path().join("private.txt"), "private").unwrap();
    std::fs::create_dir(d.dir_a.path().join("private")).unwrap();
    // Links inside scoped's home that point elsewhere in the same source.
    symlink(
        d.dir_a.path().join("private.txt"),
        d.dir_a.path().join("home/file"),
    )
    .unwrap();
    symlink(
        d.dir_a.path().join("private"),
        d.dir_a.path().join("home/dir"),
    )
    .unwrap();
    // And one out of the source entirely.
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("etc"), "outside").unwrap();
    symlink(outside.path(), d.dir_a.path().join("home/out")).unwrap();

    for path in ["/dav/a/file", "/dav/a/dir/", "/dav/a/out/etc"] {
        let r = d.req(Some("scoped"), "GET", path, &[], "").await;
        assert!(!r.status.is_success(), "GET {path}: {}", r.status);
        assert!(!r.body.contains("private") && !r.body.contains("outside"));
    }
    for path in ["/dav/a/file", "/dav/a/dir/w", "/dav/a/out/w"] {
        let r = d.req(Some("scoped"), "PUT", path, &[], "w").await;
        assert!(!r.status.is_success(), "PUT {path}: {}", r.status);
    }
    assert_eq!(
        std::fs::read_to_string(d.dir_a.path().join("private.txt")).unwrap(),
        "private"
    );
    assert!(!outside.path().join("w").exists());
    let r = d
        .req(
            Some("scoped"),
            "COPY",
            "/dav/a/file",
            &[("destination", "/dav/a/copied")],
            "",
        )
        .await;
    assert!(!r.status.is_success());
    assert!(!d.dir_a.path().join("private/w").exists());
    // Out-of-scope links are hidden from listings.
    let r = d
        .req(Some("scoped"), "PROPFIND", "/dav/a/", &[("depth", "1")], "")
        .await;
    for h in r.hrefs() {
        assert!(!h.contains("file") && !h.contains("out"), "{h}");
    }
    // An unscoped user is still confined to the source root.
    let r = d.alice("GET", "/dav/a/home/out/etc", &[], "").await;
    assert!(!r.status.is_success());
}

#[tokio::test]
async fn permissions_are_enforced() {
    let d = Dav::new();
    d.alice("PUT", "/dav/a/doc", &[], "content").await;
    d.alice("MKCOL", "/dav/a/dir", &[], "").await;

    // viewer: may list but not download or change anything.
    let v = Some("viewer");
    assert_eq!(
        d.req(v, "PROPFIND", "/dav/a/", &[("depth", "1")], "")
            .await
            .status,
        StatusCode::MULTI_STATUS
    );
    assert_eq!(
        d.req(v, "GET", "/dav/a/doc", &[], "").await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        d.req(v, "GET", "/dav/a/dir/", &[], "").await.status,
        StatusCode::OK
    );

    // reader: downloads, nothing else.
    let rd = Some("reader");
    assert_eq!(
        d.req(rd, "GET", "/dav/a/doc", &[], "").await.body,
        "content"
    );
    for (m, p, h) in [
        ("PUT", "/dav/a/new", vec![]),
        ("PUT", "/dav/a/doc", vec![]),
        ("MKCOL", "/dav/a/new2", vec![]),
        ("DELETE", "/dav/a/doc", vec![]),
        ("PROPPATCH", "/dav/a/doc", vec![]),
        ("MOVE", "/dav/a/doc", vec![("destination", "/dav/a/doc2")]),
        ("COPY", "/dav/a/doc", vec![("destination", "/dav/a/doc3")]),
        ("LOCK", "/dav/a/doc", vec![]),
    ] {
        let body = if m == "PROPPATCH" {
            r#"<D:propertyupdate xmlns:D="DAV:"><D:set><D:prop><x xmlns="urn:x">1</x></D:prop></D:set></D:propertyupdate>"#
        } else if m == "LOCK" {
            r#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#
        } else {
            ""
        };
        let r = d.req(rd, m, p, &h, body).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "reader {m} {p}");
    }
    assert_eq!(
        std::fs::read_to_string(d.dir_a.path().join("doc")).unwrap(),
        "content"
    );
    assert!(!d.dir_a.path().join("doc3").exists());

    // No access at all: 404, never revealing that the source exists.
    assert_eq!(
        d.req(Some("bob"), "GET", "/dav/b/", &[], "").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn quota_properties_and_limits() {
    let d = Dav::new();
    let q = Some("quota");
    let r = d
        .req(
            q,
            "PROPFIND",
            "/dav/a/",
            &[("depth", "0")],
            r#"<D:propfind xmlns:D="DAV:"><D:prop><D:quota-available-bytes/><D:quota-used-bytes/></D:prop></D:propfind>"#,
        )
        .await;
    assert!(
        r.body
            .contains("<D:quota-available-bytes>10</D:quota-available-bytes>")
    );
    assert!(
        r.body
            .contains("<D:quota-used-bytes>90</D:quota-used-bytes>")
    );
    // A declared upload larger than what is left is refused up front.
    let r = d
        .req(
            q,
            "PUT",
            "/dav/a/big",
            &[("content-length", "11")],
            "x".repeat(11),
        )
        .await;
    assert_eq!(r.status, StatusCode::INSUFFICIENT_STORAGE);
    let r = d.req(q, "PUT", "/dav/a/small", &[], "x".repeat(10)).await;
    assert_eq!(r.status, StatusCode::CREATED);
    // Users without a quota get 404 for these properties.
    let r = d
        .alice(
            "PROPFIND",
            "/dav/a/",
            &[("depth", "0")],
            r#"<D:propfind xmlns:D="DAV:"><D:prop><D:quota-available-bytes/></D:prop></D:propfind>"#,
        )
        .await;
    assert!(r.body.contains("404"));
}

#[tokio::test]
async fn oversized_xml_bodies_are_refused() {
    let d = Dav::new();
    let huge = format!(
        r#"<D:propfind xmlns:D="DAV:"><D:prop>{}</D:prop></D:propfind>"#,
        "<D:getetag/>".repeat(200_000)
    );
    let r = d
        .alice("PROPFIND", "/dav/a/", &[("depth", "0")], &huge)
        .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn hidden_folders_are_invisible() {
    let d = Dav::new();
    let trash = d.dir_a.path().join(common::HIDDEN);
    std::fs::create_dir(&trash).unwrap();
    std::fs::write(trash.join("deleted.txt"), "old").unwrap();
    let r = d.alice("PROPFIND", "/dav/a/", &[("depth", "1")], "").await;
    assert!(!r.body.contains(common::HIDDEN));
    let r = d.alice("GET", "/dav/a/", &[], "").await;
    assert!(!r.body.contains(common::HIDDEN));
    let p = format!("/dav/a/{}/deleted.txt", common::HIDDEN);
    assert_eq!(
        d.alice("GET", &p, &[], "").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        d.alice("DELETE", &p, &[], "").await.status,
        StatusCode::NOT_FOUND
    );
    d.alice("PUT", "/dav/a/f", &[], "x").await;
    let dest = format!("/dav/a/{}/f", common::HIDDEN);
    let r = d
        .alice("MOVE", "/dav/a/f", &[("destination", &dest)], "")
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // A cross-source copy of the whole source skips it too.
    let r = d
        .alice("COPY", "/dav/a/", &[("destination", "/dav/b/all/")], "")
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert!(d.dir_b.path().join("all/f").exists());
    assert!(!d.dir_b.path().join("all").join(common::HIDDEN).exists());
}

#[tokio::test]
async fn scoped_cross_source_copy_uses_visible_paths() {
    let d = Dav::new();
    let both = Some("both");
    d.req(both, "MKCOL", "/dav/a/dir", &[], "").await;
    d.req(both, "PUT", "/dav/a/dir/f", &[], "data").await;
    assert!(d.dir_a.path().join("home/dir/f").exists());
    let r = d
        .req(
            both,
            "COPY",
            "/dav/a/dir/",
            &[("destination", "/dav/b/copy/")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(
        std::fs::read_to_string(d.dir_b.path().join("copy/f")).unwrap(),
        "data"
    );
    let r = d
        .req(
            both,
            "MOVE",
            "/dav/b/copy/f",
            &[("destination", "/dav/a/back")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert!(d.dir_a.path().join("home/back").exists());
    assert!(!d.dir_a.path().join("back").exists());
}
