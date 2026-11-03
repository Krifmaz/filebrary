//! Protocol tests modelled on the litmus WebDAV test suite (basic, copymove,
//! props, locks and http groups), run in-process against temporary local
//! sources.

mod common;

use axum::http::StatusCode;
use common::Dav;

const PROPFIND_ALL: &str =
    r#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:allprop/></D:propfind>"#;

fn prop(names: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:prop>{names}</D:prop></D:propfind>"#
    )
}

// ------------------------------------------------------------------- basic

#[tokio::test]
async fn options_advertises_class_2() {
    let d = Dav::new();
    // No credentials: Windows sends OPTIONS before authenticating.
    let r = d.req(None, "OPTIONS", "/dav/a/", &[], "").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("dav"), "1, 2");
    assert_eq!(r.header("ms-author-via"), "DAV");
    for m in ["PROPFIND", "LOCK", "UNLOCK", "COPY", "MOVE", "MKCOL"] {
        assert!(r.header("allow").contains(m));
    }
}

#[tokio::test]
async fn auth_is_required() {
    let d = Dav::new();
    let r = d
        .req(None, "PROPFIND", "/dav/a/", &[("depth", "0")], "")
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(
        r.header("www-authenticate")
            .starts_with("Basic realm=\"Filebrary\"")
    );
    let r = d.req(Some("nobody"), "GET", "/dav/a/", &[], "").await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn put_get_delete() {
    let d = Dav::new();
    let r = d
        .alice("PUT", "/dav/a/res", &[], "This is\na test file.\n")
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let etag = r.header("etag").to_owned();
    assert!(etag.starts_with('"'));
    let r = d.alice("GET", "/dav/a/res", &[], "").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, "This is\na test file.\n");
    assert_eq!(r.header("etag"), etag);
    // Overwrite answers 204.
    let r = d.alice("PUT", "/dav/a/res", &[], "x").await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = d.alice("DELETE", "/dav/a/res", &[], "").await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = d.alice("DELETE", "/dav/a/res", &[], "").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = d.alice("GET", "/dav/a/res", &[], "").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn put_utf8_and_spaces() {
    let d = Dav::new();
    let r = d
        .alice("PUT", "/dav/a/r%C3%A9s%20um%C3%A9.txt", &[], "x")
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert!(d.dir_a.path().join("rés umé.txt").exists());
    let r = d.alice("PROPFIND", "/dav/a/", &[("depth", "1")], "").await;
    assert!(
        r.hrefs()
            .contains(&"/dav/a/r%C3%A9s%20um%C3%A9.txt".to_owned())
    );
}

#[tokio::test]
async fn put_no_parent_and_over_collection() {
    let d = Dav::new();
    let r = d.alice("PUT", "/dav/a/nope/res", &[], "x").await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    d.alice("MKCOL", "/dav/a/coll", &[], "").await;
    let r = d.alice("PUT", "/dav/a/coll", &[], "x").await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
    let r = d
        .alice("PUT", "/dav/a/x", &[("content-range", "bytes 0-0/1")], "x")
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn mkcol_rules() {
    let d = Dav::new();
    assert_eq!(
        d.alice("MKCOL", "/dav/a/coll/", &[], "").await.status,
        StatusCode::CREATED
    );
    assert!(d.dir_a.path().join("coll").is_dir());
    // Again: 405.
    assert_eq!(
        d.alice("MKCOL", "/dav/a/coll/", &[], "").await.status,
        StatusCode::METHOD_NOT_ALLOWED
    );
    // Over a file: 405.
    d.alice("PUT", "/dav/a/f", &[], "x").await;
    assert_eq!(
        d.alice("MKCOL", "/dav/a/f", &[], "").await.status,
        StatusCode::METHOD_NOT_ALLOWED
    );
    // No parent: 409.
    assert_eq!(
        d.alice("MKCOL", "/dav/a/x/y/z", &[], "").await.status,
        StatusCode::CONFLICT
    );
    // With a body: 415.
    assert_eq!(
        d.alice(
            "MKCOL",
            "/dav/a/withbody",
            &[("content-type", "text/xml")],
            "<foo/>"
        )
        .await
        .status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    // Delete the collection.
    d.alice("PUT", "/dav/a/coll/inner", &[], "x").await;
    assert_eq!(
        d.alice("DELETE", "/dav/a/coll/", &[], "").await.status,
        StatusCode::NO_CONTENT
    );
    assert!(!d.dir_a.path().join("coll").exists());
    // The source root cannot be deleted.
    assert_eq!(
        d.alice("DELETE", "/dav/a/", &[], "").await.status,
        StatusCode::FORBIDDEN
    );
}

// ---------------------------------------------------------------- copymove

#[tokio::test]
async fn copy_simple_and_overwrite() {
    let d = Dav::new();
    d.alice("PUT", "/dav/a/src", &[], "source").await;
    let r = d
        .alice(
            "COPY",
            "/dav/a/src",
            &[("destination", "http://localhost/dav/a/dest")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(d.alice("GET", "/dav/a/dest", &[], "").await.body, "source");
    // Overwrite: F refuses with 412.
    let r = d
        .alice(
            "COPY",
            "/dav/a/src",
            &[("destination", "/dav/a/dest"), ("overwrite", "F")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::PRECONDITION_FAILED);
    // T (the default) replaces with 204.
    d.alice("PUT", "/dav/a/src", &[], "newer").await;
    let r = d
        .alice(
            "COPY",
            "/dav/a/src",
            &[("destination", "/dav/a/dest"), ("overwrite", "T")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(d.alice("GET", "/dav/a/dest", &[], "").await.body, "newer");
    // No destination parent: 409.
    let r = d
        .alice(
            "COPY",
            "/dav/a/src",
            &[("destination", "/dav/a/nocoll/dest")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    // Onto itself: 403.
    let r = d
        .alice("COPY", "/dav/a/src", &[("destination", "/dav/a/src")], "")
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Missing Destination: 400; outside the mount: 502.
    assert_eq!(
        d.alice("COPY", "/dav/a/src", &[], "").await.status,
        StatusCode::BAD_REQUEST
    );
    let r = d
        .alice(
            "COPY",
            "/dav/a/src",
            &[("destination", "http://other/elsewhere/x")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn copy_collection_deep_and_shallow() {
    let d = Dav::new();
    d.alice("MKCOL", "/dav/a/ccsrc", &[], "").await;
    d.alice("MKCOL", "/dav/a/ccsrc/sub", &[], "").await;
    for i in 0..3 {
        d.alice("PUT", &format!("/dav/a/ccsrc/foo.{i}"), &[], "x")
            .await;
    }
    d.alice("PUT", "/dav/a/ccsrc/sub/deep", &[], "deep").await;
    let r = d
        .alice(
            "COPY",
            "/dav/a/ccsrc/",
            &[("destination", "/dav/a/ccdest/")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(
        d.alice("GET", "/dav/a/ccdest/sub/deep", &[], "").await.body,
        "deep"
    );
    assert_eq!(
        d.alice("GET", "/dav/a/ccdest/foo.2", &[], "").await.status,
        StatusCode::OK
    );
    // Depth 0 copies only the collection itself.
    let r = d
        .alice(
            "COPY",
            "/dav/a/ccsrc/",
            &[("destination", "/dav/a/shallow/"), ("depth", "0")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert!(d.dir_a.path().join("shallow").is_dir());
    assert_eq!(
        std::fs::read_dir(d.dir_a.path().join("shallow"))
            .unwrap()
            .count(),
        0
    );
    // Depth 1 is not allowed for COPY.
    let r = d
        .alice(
            "COPY",
            "/dav/a/ccsrc/",
            &[("destination", "/dav/a/x/"), ("depth", "1")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // Copying into itself: 409.
    let r = d
        .alice(
            "COPY",
            "/dav/a/ccsrc/",
            &[("destination", "/dav/a/ccsrc/sub/x/")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    // Overwriting a collection with a collection replaces it entirely.
    let r = d
        .alice(
            "COPY",
            "/dav/a/shallow/",
            &[("destination", "/dav/a/ccdest/")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(!d.dir_a.path().join("ccdest/sub").exists());
}

#[tokio::test]
async fn move_file_and_collection() {
    let d = Dav::new();
    d.alice("PUT", "/dav/a/move", &[], "m").await;
    d.alice("PUT", "/dav/a/move2", &[], "m2").await;
    d.alice("MKCOL", "/dav/a/movecoll", &[], "").await;
    let r = d
        .alice(
            "MOVE",
            "/dav/a/move",
            &[("destination", "/dav/a/movedest")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(
        d.alice("GET", "/dav/a/move", &[], "").await.status,
        StatusCode::NOT_FOUND
    );
    // Overwrite F onto an existing file: 412.
    let r = d
        .alice(
            "MOVE",
            "/dav/a/move2",
            &[("destination", "/dav/a/movedest"), ("overwrite", "F")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::PRECONDITION_FAILED);
    let r = d
        .alice(
            "MOVE",
            "/dav/a/move2",
            &[("destination", "/dav/a/movedest")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(d.alice("GET", "/dav/a/movedest", &[], "").await.body, "m2");
    // Move a file over a collection, replacing it.
    let r = d
        .alice(
            "MOVE",
            "/dav/a/movedest",
            &[("destination", "/dav/a/movecoll")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(d.dir_a.path().join("movecoll").is_file());
    // Move a collection with content.
    d.alice("MKCOL", "/dav/a/mc", &[], "").await;
    d.alice("PUT", "/dav/a/mc/f", &[], "f").await;
    let r = d
        .alice("MOVE", "/dav/a/mc/", &[("destination", "/dav/a/mc2/")], "")
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(d.alice("GET", "/dav/a/mc2/f", &[], "").await.body, "f");
    // MOVE requires Depth infinity.
    let r = d
        .alice(
            "MOVE",
            "/dav/a/mc2/",
            &[("destination", "/dav/a/mc3/"), ("depth", "0")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // Into itself, or over its own parent: 409.
    let r = d
        .alice(
            "MOVE",
            "/dav/a/mc2/",
            &[("destination", "/dav/a/mc2/in/")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    d.alice("MKCOL", "/dav/a/mc2/kid", &[], "").await;
    let r = d
        .alice(
            "MOVE",
            "/dav/a/mc2/kid/",
            &[("destination", "/dav/a/mc2/")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn copy_and_move_across_sources() {
    let d = Dav::new();
    d.alice("MKCOL", "/dav/a/t", &[], "").await;
    d.alice("PUT", "/dav/a/t/f", &[], "hello").await;
    let r = d
        .alice("COPY", "/dav/a/t/", &[("destination", "/dav/b/t/")], "")
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(
        std::fs::read_to_string(d.dir_b.path().join("t/f")).unwrap(),
        "hello"
    );
    let r = d
        .alice("MOVE", "/dav/a/t/f", &[("destination", "/dav/b/moved")], "")
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert!(!d.dir_a.path().join("t/f").exists());
    assert!(d.dir_b.path().join("moved").exists());
    // bob has no access to source b at all.
    let r = d
        .req(
            Some("bob"),
            "COPY",
            "/dav/a/t/",
            &[("destination", "/dav/b/x/")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

// ------------------------------------------------------------------- props

#[tokio::test]
async fn propfind_depths_and_live_props() {
    let d = Dav::new();
    d.alice("PUT", "/dav/a/file.txt", &[], "12345").await;
    let r = d
        .alice(
            "PROPFIND",
            "/dav/a/file.txt",
            &[("depth", "0")],
            PROPFIND_ALL,
        )
        .await;
    assert_eq!(r.status, StatusCode::MULTI_STATUS);
    assert!(r.header("content-type").starts_with("application/xml"));
    let props = r.propstats();
    let names: Vec<&str> = props.iter().map(|p| p.1.as_str()).collect();
    for n in [
        "resourcetype",
        "getcontentlength",
        "getcontenttype",
        "getetag",
        "getlastmodified",
        "supportedlock",
        "lockdiscovery",
        "displayname",
    ] {
        assert!(names.contains(&n), "missing {n} in {}", r.body);
    }
    assert!(
        r.body
            .contains("<D:getcontentlength>5</D:getcontentlength>")
    );
    assert!(
        r.body
            .contains("<D:getcontenttype>text/plain</D:getcontenttype>")
    );
    // RFC 1123 date, as Windows and Finder expect.
    assert!(r.body.contains(" GMT</D:getlastmodified>"));

    // Depth 1 lists children; collections have trailing slashes.
    d.alice("MKCOL", "/dav/a/sub", &[], "").await;
    let r = d.alice("PROPFIND", "/dav/a", &[("depth", "1")], "").await;
    let hrefs = r.hrefs();
    assert_eq!(hrefs[0], "/dav/a/");
    assert!(hrefs.contains(&"/dav/a/sub/".to_owned()));
    assert!(hrefs.contains(&"/dav/a/file.txt".to_owned()));
    assert!(r.body.contains("<D:collection/>"));

    // Depth infinity is refused.
    let r = d
        .alice("PROPFIND", "/dav/a/", &[("depth", "infinity")], "")
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.body.contains("propfind-finite-depth"));
    let r = d.alice("PROPFIND", "/dav/a/", &[], "").await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Bad bodies: 400. Missing resources: 404.
    let r = d
        .alice("PROPFIND", "/dav/a/", &[("depth", "0")], "<foo>")
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = d
        .alice("PROPFIND", "/dav/a/nope", &[("depth", "0")], "")
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn propfind_specific_and_propname() {
    let d = Dav::new();
    d.alice("PUT", "/dav/a/f", &[], "x").await;
    let r = d
        .alice(
            "PROPFIND",
            "/dav/a/f",
            &[("depth", "0")],
            &prop(r#"<D:getetag/><Z:nonesuch xmlns:Z="http://example.com/neon/litmus/"/>"#),
        )
        .await;
    let p = r.propstats();
    assert!(p.contains(&("/dav/a/f".into(), "getetag".into(), 200)));
    assert!(p.contains(&("/dav/a/f".into(), "nonesuch".into(), 404)));
    let r = d
        .alice(
            "PROPFIND",
            "/dav/a/f",
            &[("depth", "0")],
            r#"<D:propfind xmlns:D="DAV:"><D:propname/></D:propfind>"#,
        )
        .await;
    assert!(r.body.contains("<D:getetag/>"));
    assert!(!r.body.contains("</D:getetag>")); // names only, no values
}

#[tokio::test]
async fn mount_root_lists_visible_sources() {
    let d = Dav::new();
    let r = d.alice("PROPFIND", "/dav/", &[("depth", "1")], "").await;
    assert_eq!(r.status, StatusCode::MULTI_STATUS);
    assert_eq!(r.hrefs(), vec!["/dav/", "/dav/a/", "/dav/b/"]);
    let r = d
        .req(Some("bob"), "PROPFIND", "/dav", &[("depth", "1")], "")
        .await;
    assert_eq!(r.hrefs(), vec!["/dav/", "/dav/a/"]);
    assert_eq!(
        d.alice("PUT", "/dav/x", &[], "").await.status,
        StatusCode::NOT_FOUND // "x" is not a source
    );
    assert_eq!(
        d.alice("MKCOL", "/dav/", &[], "").await.status,
        StatusCode::METHOD_NOT_ALLOWED
    );
    let r = d.alice("GET", "/dav/", &[], "").await;
    assert!(r.body.contains("href=\"/dav/b/\""));
}

const NS: &str = "http://example.com/neon/litmus/";

fn proppatch(set: &str, remove: &str) -> String {
    let mut s = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><D:propertyupdate xmlns:D="DAV:" xmlns:Z="{NS}">"#
    );
    if !set.is_empty() {
        s.push_str(&format!("<D:set><D:prop>{set}</D:prop></D:set>"));
    }
    if !remove.is_empty() {
        s.push_str(&format!("<D:remove><D:prop>{remove}</D:prop></D:remove>"));
    }
    s.push_str("</D:propertyupdate>");
    s
}

#[tokio::test]
async fn dead_properties_round_trip() {
    let d = Dav::new();
    d.alice("PUT", "/dav/a/prop", &[], "x").await;
    // propset: ten properties, plus a high-unicode value and a namespaced value.
    let mut set = String::new();
    for i in 0..10 {
        set.push_str(&format!("<Z:prop{i}>value{i}</Z:prop{i}>"));
    }
    set.push_str("<Z:high-unicode>\u{10000}</Z:high-unicode>");
    set.push_str(r#"<Z:valnspace><foo xmlns="http://bar">x &amp; y</foo></Z:valnspace>"#);
    set.push_str(r#"<nonamespace xmlns="">plain</nonamespace>"#);
    let r = d
        .alice("PROPPATCH", "/dav/a/prop", &[], &proppatch(&set, ""))
        .await;
    assert_eq!(r.status, StatusCode::MULTI_STATUS, "{}", r.body);
    assert!(r.propstats().iter().all(|p| p.2 == 200));

    let r = d
        .alice(
            "PROPFIND",
            "/dav/a/prop",
            &[("depth", "0")],
            &prop(&format!(
                r#"<Z:prop3 xmlns:Z="{NS}"/><Z:high-unicode xmlns:Z="{NS}"/><Z:valnspace xmlns:Z="{NS}"/><nonamespace xmlns=""/>"#
            )),
        )
        .await;
    let doc = roxmltree::Document::parse(&r.body).unwrap();
    let get = |ns: &str, name: &str| {
        doc.descendants()
            .find(|n| n.has_tag_name((ns, name)))
            .unwrap_or_else(|| panic!("{name} missing in {}", r.body))
    };
    assert_eq!(get(NS, "prop3").text(), Some("value3"));
    assert_eq!(get(NS, "high-unicode").text(), Some("\u{10000}"));
    let foo = get("http://bar", "foo");
    assert_eq!(foo.text(), Some("x & y"));
    assert_eq!(get("", "nonamespace").text(), Some("plain"));

    // propreplace and propremove in one request, applied in order.
    let r = d
        .alice(
            "PROPPATCH",
            "/dav/a/prop",
            &[],
            &proppatch("<Z:prop0>replaced</Z:prop0>", "<Z:prop1/>"),
        )
        .await;
    assert_eq!(r.status, StatusCode::MULTI_STATUS);
    let r = d
        .alice("PROPFIND", "/dav/a/prop", &[("depth", "0")], PROPFIND_ALL)
        .await;
    assert!(r.body.contains(">replaced</prop0>"));
    assert!(!r.body.contains("prop1"));

    // Properties follow MOVE and COPY and vanish on DELETE.
    d.alice(
        "MOVE",
        "/dav/a/prop",
        &[("destination", "/dav/a/prop2")],
        "",
    )
    .await;
    d.alice(
        "COPY",
        "/dav/a/prop2",
        &[("destination", "/dav/a/prop3")],
        "",
    )
    .await;
    for f in ["prop2", "prop3"] {
        let r = d
            .alice(
                "PROPFIND",
                &format!("/dav/a/{f}"),
                &[("depth", "0")],
                PROPFIND_ALL,
            )
            .await;
        assert!(r.body.contains(">replaced</prop0>"), "{f}");
    }
    d.alice("DELETE", "/dav/a/prop3", &[], "").await;
    d.alice("PUT", "/dav/a/prop3", &[], "x").await;
    let r = d
        .alice("PROPFIND", "/dav/a/prop3", &[("depth", "0")], PROPFIND_ALL)
        .await;
    assert!(!r.body.contains("prop0"));
}

#[tokio::test]
async fn protected_properties_fail_atomically() {
    let d = Dav::new();
    d.alice("PUT", "/dav/a/f", &[], "x").await;
    let r = d
        .alice(
            "PROPPATCH",
            "/dav/a/f",
            &[],
            &proppatch("<Z:fine>1</Z:fine><D:getetag>\"x\"</D:getetag>", ""),
        )
        .await;
    assert_eq!(r.status, StatusCode::MULTI_STATUS);
    let p = r.propstats();
    assert!(p.contains(&("/dav/a/f".into(), "getetag".into(), 403)));
    assert!(p.contains(&("/dav/a/f".into(), "fine".into(), 424)));
    assert!(r.body.contains("cannot-modify-protected-property"));
    let r = d
        .alice("PROPFIND", "/dav/a/f", &[("depth", "0")], PROPFIND_ALL)
        .await;
    assert!(!r.body.contains("fine"));
}

#[tokio::test]
async fn windows_explorer_win32_properties() {
    let d = Dav::new();
    d.alice("PUT", "/dav/a/doc.txt", &[], "x").await;
    let body = r#"<?xml version="1.0" encoding="utf-8" ?><D:propertyupdate xmlns:D="DAV:" xmlns:Z="urn:schemas-microsoft-com:"><D:set><D:prop><Z:Win32CreationTime>Tue, 03 Nov 2026 17:00:00 GMT</Z:Win32CreationTime><Z:Win32LastAccessTime>Tue, 03 Nov 2026 17:00:00 GMT</Z:Win32LastAccessTime><Z:Win32LastModifiedTime>Tue, 03 Nov 2026 17:00:00 GMT</Z:Win32LastModifiedTime><Z:Win32FileAttributes>00000020</Z:Win32FileAttributes></D:prop></D:set></D:propertyupdate>"#;
    let r = d.alice("PROPPATCH", "/dav/a/doc.txt", &[], body).await;
    assert_eq!(r.status, StatusCode::MULTI_STATUS);
    assert_eq!(r.propstats().len(), 4);
    assert!(r.propstats().iter().all(|p| p.2 == 200));
}

// ------------------------------------------------------------------- locks

#[tokio::test]
async fn exclusive_lock_lifecycle() {
    let d = Dav::new();
    d.alice("PUT", "/dav/a/lockme", &[], "x").await;
    let token = d.lock("alice", "/dav/a/lockme", "0", false).await;
    assert!(token.starts_with("opaquelocktoken:"));

    // discover: lockdiscovery shows the lock.
    let r = d
        .alice(
            "PROPFIND",
            "/dav/a/lockme",
            &[("depth", "0")],
            &prop("<D:lockdiscovery/>"),
        )
        .await;
    assert!(r.body.contains(&token));
    assert!(r.body.contains("<D:exclusive/>"));
    assert!(r.body.contains("litmus test suite"));
    assert!(
        r.body
            .contains("<D:lockroot><D:href>/dav/a/lockme</D:href></D:lockroot>")
    );

    // notowner_modify: another user, or no token, gets 423.
    let r = d.req(Some("bob"), "PUT", "/dav/a/lockme", &[], "bob").await;
    assert_eq!(r.status, StatusCode::LOCKED);
    assert!(r.body.contains("lock-token-submitted"));
    assert_eq!(
        d.alice("PUT", "/dav/a/lockme", &[], "no token")
            .await
            .status,
        StatusCode::LOCKED
    );
    assert_eq!(
        d.alice("DELETE", "/dav/a/lockme", &[], "").await.status,
        StatusCode::LOCKED
    );
    // A stolen token does not help bob.
    let iff = format!("(<{token}>)");
    let r = d
        .req(Some("bob"), "PUT", "/dav/a/lockme", &[("if", &iff)], "bob")
        .await;
    assert_eq!(r.status, StatusCode::LOCKED);
    // notowner_lock: bob cannot lock it either.
    let r = d
        .req(
            Some("bob"),
            "LOCK",
            "/dav/a/lockme",
            &[],
            r#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#,
        )
        .await;
    assert_eq!(r.status, StatusCode::LOCKED);
    assert!(r.body.contains("no-conflicting-lock"));

    // owner_modify: with the token it works.
    assert_eq!(
        d.alice("PUT", "/dav/a/lockme", &[("if", &iff)], "mine")
            .await
            .status,
        StatusCode::NO_CONTENT
    );

    // refresh.
    let r = d
        .alice(
            "LOCK",
            "/dav/a/lockme",
            &[("if", &iff), ("timeout", "Second-300")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains(&token));

    // notowner unlock: 403. Wrong resource: 409. Right: 204.
    let lt = format!("<{token}>");
    assert_eq!(
        d.req(
            Some("bob"),
            "UNLOCK",
            "/dav/a/lockme",
            &[("lock-token", &lt)],
            ""
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    d.alice("PUT", "/dav/a/other", &[], "x").await;
    assert_eq!(
        d.alice("UNLOCK", "/dav/a/other", &[("lock-token", &lt)], "")
            .await
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        d.alice("UNLOCK", "/dav/a/lockme", &[("lock-token", &lt)], "")
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        d.req(Some("bob"), "PUT", "/dav/a/lockme", &[], "bob")
            .await
            .status,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn conditional_puts() {
    let d = Dav::new();
    d.alice("PUT", "/dav/a/c", &[], "x").await;
    let token = d.lock("alice", "/dav/a/c", "0", false).await;
    let etag = d
        .alice("HEAD", "/dav/a/c", &[], "")
        .await
        .header("etag")
        .to_owned();

    let put = |iff: String| {
        let d = &d;
        async move {
            d.alice("PUT", "/dav/a/c", &[("if", &iff)], "y")
                .await
                .status
        }
    };
    // cond_put
    assert!(put(format!("(<{token}> [{etag}])")).await.is_success());
    let etag = d
        .alice("HEAD", "/dav/a/c", &[], "")
        .await
        .header("etag")
        .to_owned();
    // fail_cond_put
    assert_eq!(
        put(format!("(<{token}> [\"fake-etag\"])")).await,
        StatusCode::PRECONDITION_FAILED
    );
    // cond_put_with_not
    assert!(
        put(format!("(<{token}>) (Not <DAV:no-lock>)"))
            .await
            .is_success()
    );
    // cond_put_corrupt_token: the If header passes (second list) but the lock
    // token is not submitted.
    assert_eq!(
        put("(<opaquelocktoken:foobar>) (Not <DAV:no-lock>)".into()).await,
        StatusCode::LOCKED
    );
    let etag2 = d
        .alice("HEAD", "/dav/a/c", &[], "")
        .await
        .header("etag")
        .to_owned();
    // complex_cond_put
    assert!(
        put(format!(
            "(<{token}> [{etag2}]) (Not <DAV:no-lock> [{etag2}])"
        ))
        .await
        .is_success()
    );
    let _ = etag;
    // fail_complex_cond_put
    assert_eq!(
        put(format!(
            "(<{token}> [W/\"fake\"]) (Not <DAV:no-lock> [W/\"fake\"])"
        ))
        .await,
        StatusCode::PRECONDITION_FAILED
    );
    // Tagged list naming the resource.
    assert!(
        put(format!("<http://localhost/dav/a/c> (<{token}>)"))
            .await
            .is_success()
    );
    // Malformed If: 400.
    assert_eq!(put("(<unterminated".into()).await, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn shared_locks() {
    let d = Dav::new();
    d.alice("PUT", "/dav/a/s", &[], "x").await;
    let t1 = d.lock("alice", "/dav/a/s", "0", true).await;
    let t2 = d.lock("bob", "/dav/a/s", "0", true).await;
    assert_ne!(t1, t2);
    // An exclusive lock now conflicts.
    let r = d
        .alice(
            "LOCK",
            "/dav/a/s",
            &[],
            r#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#,
        )
        .await;
    assert_eq!(r.status, StatusCode::LOCKED);
    let r = d
        .alice(
            "PROPFIND",
            "/dav/a/s",
            &[("depth", "0")],
            &prop("<D:lockdiscovery/>"),
        )
        .await;
    assert!(r.body.contains(&t1) && r.body.contains(&t2));
    // Writing needs every lock that applies: alice holds only one.
    let r = d
        .alice("PUT", "/dav/a/s", &[("if", &format!("(<{t1}>)"))], "y")
        .await;
    assert_eq!(r.status, StatusCode::LOCKED);
}

#[tokio::test]
async fn collection_locks() {
    let d = Dav::new();
    d.alice("MKCOL", "/dav/a/lc", &[], "").await;
    d.alice("PUT", "/dav/a/lc/inner", &[], "x").await;
    let token = d.lock("alice", "/dav/a/lc/", "infinity", false).await;
    let iff = format!("(<{token}>)");
    // Members are covered: modifying, adding and deleting need the token.
    assert_eq!(
        d.alice("PUT", "/dav/a/lc/inner", &[], "y").await.status,
        StatusCode::LOCKED
    );
    assert_eq!(
        d.alice("PUT", "/dav/a/lc/new", &[], "y").await.status,
        StatusCode::LOCKED
    );
    assert_eq!(
        d.alice("MKCOL", "/dav/a/lc/sub", &[], "").await.status,
        StatusCode::LOCKED
    );
    assert_eq!(
        d.alice("DELETE", "/dav/a/lc/inner", &[], "").await.status,
        StatusCode::LOCKED
    );
    assert_eq!(
        d.alice("PUT", "/dav/a/lc/inner", &[("if", &iff)], "y")
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    // Tagged with the collection URL works for a member too.
    let tagged = format!("<http://localhost/dav/a/lc/> (<{token}>)");
    assert_eq!(
        d.alice("PUT", "/dav/a/lc/new", &[("if", &tagged)], "y")
            .await
            .status,
        StatusCode::CREATED
    );
    // Moving the locked collection away needs the token; COPY out does not.
    assert_eq!(
        d.alice("MOVE", "/dav/a/lc/", &[("destination", "/dav/a/lc2/")], "")
            .await
            .status,
        StatusCode::LOCKED
    );
    assert_eq!(
        d.alice("COPY", "/dav/a/lc/", &[("destination", "/dav/a/lc3/")], "")
            .await
            .status,
        StatusCode::CREATED
    );
    // Deleting the parent of a locked resource needs the token too.
    d.alice("MKCOL", "/dav/a/p", &[], "").await;
    d.alice("PUT", "/dav/a/p/f", &[], "x").await;
    let ft = d.lock("alice", "/dav/a/p/f", "0", false).await;
    assert_eq!(
        d.alice("DELETE", "/dav/a/p/", &[], "").await.status,
        StatusCode::LOCKED
    );
    assert_eq!(
        d.alice(
            "DELETE",
            "/dav/a/p/",
            &[("if", &format!("</dav/a/p/f> (<{ft}>)"))],
            ""
        )
        .await
        .status,
        StatusCode::NO_CONTENT
    );
    // Depth 1 is not a valid lock depth.
    let r = d
        .alice(
            "LOCK",
            "/dav/a/lc3/",
            &[("depth", "1")],
            r#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#,
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unmapped_lock_creates_empty_file() {
    let d = Dav::new();
    let r = d
        .alice(
            "LOCK",
            "/dav/a/unmapped.txt",
            &[("depth", "0")],
            r#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let token = r.header("lock-token").trim_matches(['<', '>']).to_owned();
    assert_eq!(
        std::fs::read(d.dir_a.path().join("unmapped.txt"))
            .unwrap()
            .len(),
        0
    );
    // The Finder pattern: LOCK, PUT with the token, UNLOCK.
    let r = d
        .alice(
            "PUT",
            "/dav/a/unmapped.txt",
            &[("if", &format!("(<{token}>)"))],
            "data",
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = d
        .alice(
            "UNLOCK",
            "/dav/a/unmapped.txt",
            &[("lock-token", &format!("<{token}>"))],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    // No parent: 409.
    let r = d
        .alice(
            "LOCK",
            "/dav/a/none/x",
            &[],
            r#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#,
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn delete_and_move_drop_locks() {
    let d = Dav::new();
    d.alice("PUT", "/dav/a/gone", &[], "x").await;
    let t = d.lock("alice", "/dav/a/gone", "0", false).await;
    let iff = format!("(<{t}>)");
    d.alice(
        "MOVE",
        "/dav/a/gone",
        &[("destination", "/dav/a/here"), ("if", &iff)],
        "",
    )
    .await;
    // The lock did not move with the file.
    assert_eq!(
        d.req(Some("bob"), "PUT", "/dav/a/here", &[], "y")
            .await
            .status,
        StatusCode::NO_CONTENT
    );
}

// -------------------------------------------------------------------- http

#[tokio::test]
async fn ranges_and_conditional_get() {
    let d = Dav::new();
    d.alice("PUT", "/dav/a/r.bin", &[], "0123456789").await;
    let r = d
        .alice("GET", "/dav/a/r.bin", &[("range", "bytes=2-4")], "")
        .await;
    assert_eq!(r.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(r.body, "234");
    assert_eq!(r.header("content-range"), "bytes 2-4/10");
    assert_eq!(r.header("content-length"), "3");
    let r = d
        .alice("GET", "/dav/a/r.bin", &[("range", "bytes=-3")], "")
        .await;
    assert_eq!(r.body, "789");
    let r = d
        .alice("GET", "/dav/a/r.bin", &[("range", "bytes=20-")], "")
        .await;
    assert_eq!(r.status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(r.header("content-range"), "bytes */10");
    let r = d.alice("HEAD", "/dav/a/r.bin", &[], "").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("content-length"), "10");
    assert_eq!(r.header("accept-ranges"), "bytes");
    assert!(r.body.is_empty());
    let etag = r.header("etag").to_owned();
    let r = d
        .alice("GET", "/dav/a/r.bin", &[("if-none-match", &etag)], "")
        .await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED);
    // If-Range with a stale tag sends the whole file.
    let r = d
        .alice(
            "GET",
            "/dav/a/r.bin",
            &[("range", "bytes=0-0"), ("if-range", "\"old\"")],
            "",
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body.len(), 10);
    // PUT If-None-Match: * refuses to overwrite.
    let r = d
        .alice("PUT", "/dav/a/r.bin", &[("if-none-match", "*")], "x")
        .await;
    assert_eq!(r.status, StatusCode::PRECONDITION_FAILED);
}

#[tokio::test]
async fn chunked_put_from_finder() {
    let d = Dav::new();
    let chunks: Vec<Result<bytes::Bytes, std::io::Error>> = (0..100)
        .map(|i| Ok(bytes::Bytes::from(format!("chunk{i:03}\n"))))
        .collect();
    let body = axum::body::Body::from_stream(futures::stream::iter(chunks));
    let r = d
        .req(
            Some("alice"),
            "PUT",
            "/dav/a/big.txt",
            &[
                ("transfer-encoding", "chunked"),
                ("x-expected-entity-length", "900"),
            ],
            body,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let data = std::fs::read_to_string(d.dir_a.path().join("big.txt")).unwrap();
    assert_eq!(data.len(), 900);
    assert!(data.ends_with("chunk099\n"));
}

#[tokio::test]
async fn changes_are_reported() {
    let d = Dav::new();
    d.alice("PUT", "/dav/a/f", &[], "x").await;
    d.alice("MKCOL", "/dav/a/c", &[], "").await;
    d.alice("MOVE", "/dav/a/f", &[("destination", "/dav/a/c/f")], "")
        .await;
    d.alice("DELETE", "/dav/a/c/", &[], "").await;
    let changes = d.auth.changes.lock().unwrap();
    assert_eq!(changes.len(), 4);
    assert!(matches!(
        &changes[0],
        filebrary_webdav::Change::Written { created: true, .. }
    ));
    assert!(matches!(
        &changes[3],
        filebrary_webdav::Change::Deleted { .. }
    ));
}
