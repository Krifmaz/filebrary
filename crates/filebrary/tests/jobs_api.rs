//! Integration tests for the job queue, conversions, custom jobs and archive
//! browsing, run through the full router against a temporary directory.
//!
//! Tests that need ffmpeg or LibreOffice are skipped (with a message) when
//! the program is not installed; CI's conversion job installs both.
// The custom-job helpers are only used by the Unix-only tests (they run
// Unix programs such as `sleep` and `sed`).
#![cfg_attr(not(unix), allow(dead_code, unused_imports))]

mod common;

use std::{io::Write, time::Duration};

use axum::http::{StatusCode, header};
use common::{Client, TestApp, body_bytes, json};
use filebrary::{
    AppState,
    auth::Perms,
    config::{Config, SourceConfig},
    db,
    jobs::{CustomJobConfig, CustomOutput, process::which},
};
use serde_json::{Value, json};

/// A test app whose config can be adjusted first (for custom jobs).
async fn app_with(f: impl FnOnce(&mut Config)) -> TestApp {
    let dir = tempfile::tempdir().unwrap();
    let files = dir.path().join("files");
    std::fs::create_dir_all(&files).unwrap();
    let mut config = Config {
        data_dir: dir.path().join("data"),
        sources: vec![SourceConfig::local("files", files)],
        ..Config::default()
    };
    f(&mut config);
    config.validate().unwrap();
    std::fs::create_dir_all(&config.data_dir).unwrap();
    let pool = db::open_memory().await.unwrap();
    let state = AppState::new(config, pool, [7u8; 32]).unwrap();
    let app = filebrary::app(state.clone());
    TestApp { dir, state, app }
}

fn custom(name: &str, program: &str, args: &[&str], output: CustomOutput) -> CustomJobConfig {
    toml::from_str::<CustomJobConfig>(&format!(
        "name = {name:?}\ntitle = {name:?}\nprogram = {program:?}\nargs = {args:?}\noutput = {:?}",
        match output {
            CustomOutput::None => "none",
            CustomOutput::Replace => "replace",
            CustomOutput::Files => "files",
        }
    ))
    .unwrap()
}

async fn submit(t: &TestApp, c: &Client, kind: &str, params: Value) -> (StatusCode, Value) {
    json(
        t.send(c.post_json("/api/jobs", json!({ "kind": kind, "params": params })))
            .await,
    )
    .await
}

/// Polls a job until `done(status)` holds, failing after 60 seconds.
async fn wait_for(t: &TestApp, c: &Client, id: i64, done: impl Fn(&str) -> bool) -> Value {
    for _ in 0..600 {
        let (s, v) = json(t.send(c.get(&format!("/api/jobs/{id}"))).await).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        if done(v["status"].as_str().unwrap()) {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("job {id} did not reach the expected state");
}

async fn finished(t: &TestApp, c: &Client, id: i64) -> Value {
    wait_for(t, c, id, |s| {
        ["succeeded", "failed", "cancelled"].contains(&s)
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn archive_create_and_browse() {
    let t = TestApp::new().await;
    let c = t.user("admin", true, Perms::ALL, "/").await;
    std::fs::create_dir_all(t.root().join("docs/sub")).unwrap();
    std::fs::write(t.root().join("docs/a.txt"), b"alpha").unwrap();
    std::fs::write(t.root().join("docs/sub/b.txt"), b"beta").unwrap();
    std::fs::write(t.root().join("c.txt"), b"gamma").unwrap();

    for (format, file) in [("zip", "pack.zip"), ("tar.gz", "pack.tar.gz")] {
        let (s, v) = submit(
            &t,
            &c,
            "archive.create",
            json!({"source":"files","paths":["/docs","/c.txt"],"dest":"/","name":"pack","format":format}),
        )
        .await;
        assert_eq!(s, StatusCode::ACCEPTED, "{v}");
        let job = finished(&t, &c, v["id"].as_i64().unwrap()).await;
        assert_eq!(job["status"], "succeeded", "{job}");
        assert_eq!(job["result"]["path"], format!("/{file}"));
        assert_eq!(job["progress"], 1.0);
        assert!(t.root().join(file).is_file());

        // Browse the top level and a folder.
        let (s, v) = json(
            t.send(c.get(&format!("/api/sources/files/archive?path=/{file}&dir=")))
                .await,
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let names: Vec<_> = v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["path"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["docs", "c.txt"]);
        let (_, v) = json(
            t.send(c.get(&format!("/api/sources/files/archive?path=/{file}&dir=docs")))
                .await,
        )
        .await;
        let names: Vec<_> = v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["path"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["docs/sub", "docs/a.txt"]);

        // Read one entry.
        let resp = t
            .send(c.get(&format!(
                "/api/sources/files/archive/entry?path=/{file}&entry=docs/sub/b.txt"
            )))
            .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.headers()[header::CONTENT_DISPOSITION]
                .to_str()
                .unwrap()
                .starts_with("attachment")
        );
        assert!(
            resp.headers()[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .contains("sandbox")
        );
        assert_eq!(&body_bytes(resp).await[..], b"beta");

        let resp = t
            .send(c.get(&format!(
                "/api/sources/files/archive/entry?path=/{file}&entry=nope.txt"
            )))
            .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // A second archive with the same name never overwrites the first.
    let (_, v) = submit(
        &t,
        &c,
        "archive.create",
        json!({"source":"files","paths":["/c.txt"],"dest":"/","name":"pack.zip","format":"zip"}),
    )
    .await;
    let job = finished(&t, &c, v["id"].as_i64().unwrap()).await;
    assert_eq!(job["result"]["path"], "/pack (1).zip");

    // Not an archive.
    let (s, _) = json(
        t.send(c.get("/api/sources/files/archive?path=/c.txt"))
            .await,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread")]
async fn hostile_archive_entries_are_hidden() {
    let t = TestApp::new().await;
    let c = t.user("admin", true, Perms::ALL, "/").await;
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    z.start_file("../../etc/evil", o).unwrap();
    z.write_all(b"x").unwrap();
    z.start_file("ok.txt", o).unwrap();
    z.write_all(b"fine").unwrap();
    std::fs::write(t.root().join("h.zip"), z.finish().unwrap().into_inner()).unwrap();

    let (s, v) = json(
        t.send(c.get("/api/sources/files/archive?path=/h.zip"))
            .await,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["entries"].as_array().unwrap().len(), 1);
    assert_eq!(v["entries"][0]["path"], "ok.txt");
    for bad in ["../../etc/evil", "..%2F..%2Fetc%2Fevil"] {
        let resp = t
            .send(c.get(&format!(
                "/api/sources/files/archive/entry?path=/h.zip&entry={bad}"
            )))
            .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{bad}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn scoped_users_stay_in_their_folder() {
    let t = TestApp::new().await;
    std::fs::create_dir_all(t.root().join("home/bob")).unwrap();
    std::fs::write(t.root().join("home/bob/mine.txt"), b"bob").unwrap();
    std::fs::write(t.root().join("secret.txt"), b"top secret").unwrap();
    let bob = t
        .user(
            "bob",
            false,
            Perms::VIEW | Perms::DOWNLOAD | Perms::UPLOAD,
            "/home/bob",
        )
        .await;

    // Paths are relative to bob's folder; the output lands there too.
    let (s, v) = submit(
        &t,
        &bob,
        "archive.create",
        json!({"source":"files","paths":["/mine.txt"],"dest":"/","format":"zip"}),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let job = finished(&t, &bob, v["id"].as_i64().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(job["result"]["path"], "/mine.txt.zip");
    assert!(t.root().join("home/bob/mine.txt.zip").is_file());
    // The activity log records the real path of the new file.
    let logged: Vec<(String, String)> = sqlx::query_as("SELECT action, path FROM activity")
        .fetch_all(&t.state.db)
        .await
        .unwrap();
    assert!(
        logged.contains(&("upload".into(), "/home/bob/mine.txt.zip".into())),
        "{logged:?}"
    );

    // `..` never gets out.
    let (s, _) = submit(
        &t,
        &bob,
        "archive.create",
        json!({"source":"files","paths":["/../../secret.txt"],"dest":"/","format":"zip"}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // A symlink in bob's folder pointing elsewhere in the source is refused.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            t.root().join("secret.txt"),
            t.root().join("home/bob/link.txt"),
        )
        .unwrap();
        let (s, v) = submit(
            &t,
            &bob,
            "archive.create",
            json!({"source":"files","paths":["/link.txt"],"dest":"/","format":"zip"}),
        )
        .await;
        assert!(s.is_client_error(), "{s} {v}");
    }

    // Unknown params are rejected (no way to smuggle options through).
    let (s, _) = submit(
        &t,
        &bob,
        "archive.create",
        json!({"source":"files","paths":["/mine.txt"],"dest":"/","format":"zip","program":"/bin/sh"}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Without upload permission, no job.
    let viewer = t
        .user("eve", false, Perms::VIEW | Perms::DOWNLOAD, "/home/bob")
        .await;
    let (s, _) = submit(
        &t,
        &viewer,
        "archive.create",
        json!({"source":"files","paths":["/mine.txt"],"dest":"/","format":"zip"}),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Users cannot see each other's jobs.
    let id = job["id"].as_i64().unwrap();
    let resp = t.send(viewer.get(&format!("/api/jobs/{id}"))).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let (_, list) = json(t.send(viewer.get("/api/jobs?all=true")).await).await;
    assert_eq!(list.as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn image_conversion() {
    let t = TestApp::new().await;
    let c = t.user("admin", true, Perms::ALL, "/").await;
    image::RgbImage::from_pixel(640, 480, image::Rgb([200, 10, 10]))
        .save(t.root().join("photo.png"))
        .unwrap();
    let (s, v) = submit(
        &t,
        &c,
        "convert.image",
        json!({"source":"files","path":"/photo.png","format":"webp","max_width":320}),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    assert_eq!(v["title"], "Resize photo.png to WEBP");
    let job = finished(&t, &c, v["id"].as_i64().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(job["result"]["path"], "/photo.webp");
    let img = image::open(t.root().join("photo.webp")).unwrap();
    assert_eq!((img.width(), img.height()), (320, 240));

    // A broken image fails with a readable error, and can be retried.
    std::fs::write(t.root().join("broken.png"), b"nope").unwrap();
    let (_, v) = submit(
        &t,
        &c,
        "convert.image",
        json!({"source":"files","path":"/broken.png","format":"jpeg"}),
    )
    .await;
    let id = v["id"].as_i64().unwrap();
    let job = finished(&t, &c, id).await;
    assert_eq!(job["status"], "failed");
    assert!(!job["error"].as_str().unwrap().is_empty());
    image::RgbImage::from_pixel(8, 8, image::Rgb([0, 0, 0]))
        .save_with_format(t.root().join("broken.png"), image::ImageFormat::Png)
        .unwrap();
    let (s, v) = json(
        t.send(c.post_json(&format!("/api/jobs/{id}/retry"), json!({})))
            .await,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let job = finished(&t, &c, id).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(job["attempts"], 2);

    // Only finished jobs can be retried or deleted.
    let (s, _) = json(
        t.send(c.post_json(&format!("/api/jobs/{id}/retry"), json!({})))
            .await,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let resp = t
        .send(
            c.req("DELETE", &format!("/api/jobs/{id}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = t.send(c.get(&format!("/api/jobs/{id}"))).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn cancel_and_retry_a_running_job() {
    let t = app_with(|cfg| {
        cfg.jobs.custom = vec![custom("slow", "sleep", &["30"], CustomOutput::None)];
    })
    .await;
    let c = t.user("admin", true, Perms::ALL, "/").await;
    std::fs::write(t.root().join("x.txt"), b"x").unwrap();

    let (s, v) = submit(
        &t,
        &c,
        "custom.slow",
        json!({"source":"files","path":"/x.txt"}),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let id = v["id"].as_i64().unwrap();
    wait_for(&t, &c, id, |s| s == "running").await;
    let (s, _) = json(
        t.send(c.post_json(&format!("/api/jobs/{id}/cancel"), json!({})))
            .await,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let job = finished(&t, &c, id).await;
    assert_eq!(job["status"], "cancelled");

    // Running jobs cannot be deleted; cancelled ones can be retried.
    let (s, _) = json(
        t.send(c.post_json(&format!("/api/jobs/{id}/retry"), json!({})))
            .await,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    wait_for(&t, &c, id, |s| s == "running").await;
    let resp = t
        .send(
            c.req("DELETE", &format!("/api/jobs/{id}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    t.send(c.post_json(&format!("/api/jobs/{id}/cancel"), json!({})))
        .await;
    let job = finished(&t, &c, id).await;
    assert_eq!(job["status"], "cancelled");
    assert_eq!(job["attempts"], 2);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn custom_jobs_replace_and_files() {
    let t = app_with(|cfg| {
        cfg.jobs.custom = vec![
            custom(
                "upper",
                "sed",
                // `-i.bak` (suffix attached) works with both GNU and BSD sed.
                &["-i.bak", "s/a/A/g", "{input}"],
                CustomOutput::Replace,
            ),
            custom(
                "copy",
                "cp",
                &["{input}", "{output_dir}/copy.{ext}"],
                CustomOutput::Files,
            ),
            custom("missing", "/no/such/program", &[], CustomOutput::None),
        ];
        let mut admin_only = custom("admin", "true", &[], CustomOutput::None);
        admin_only.admin_only = true;
        cfg.jobs.custom.push(admin_only);
    })
    .await;
    let admin = t.user("admin", true, Perms::ALL, "/").await;
    std::fs::write(t.root().join("$(id).txt"), b"banana").unwrap();

    // The odd file name is data, never shell syntax.
    let (s, v) = submit(
        &t,
        &admin,
        "custom.upper",
        json!({"source":"files","path":"/$(id).txt"}),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let job = finished(&t, &admin, v["id"].as_i64().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(
        std::fs::read(t.root().join("$(id).txt")).unwrap(),
        b"bAnAnA"
    );

    let (_, v) = submit(
        &t,
        &admin,
        "custom.copy",
        json!({"source":"files","path":"/$(id).txt"}),
    )
    .await;
    let job = finished(&t, &admin, v["id"].as_i64().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(job["result"]["files"], json!(["/copy.txt"]));
    assert_eq!(std::fs::read(t.root().join("copy.txt")).unwrap(), b"bAnAnA");

    // Kinds list shows availability; a missing program cannot be submitted.
    let (_, kinds) = json(t.send(admin.get("/api/jobs/kinds")).await).await;
    let missing = kinds
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["kind"] == "custom.missing")
        .unwrap();
    assert_eq!(missing["available"], false);
    let (s, _) = submit(
        &t,
        &admin,
        "custom.missing",
        json!({"source":"files","path":"/copy.txt"}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Admin-only jobs are hidden from and refused for other users.
    let user = t.user("u", false, Perms::ALL, "/").await;
    let (_, kinds) = json(t.send(user.get("/api/jobs/kinds")).await).await;
    assert!(
        kinds
            .as_array()
            .unwrap()
            .iter()
            .all(|k| k["kind"] != "custom.admin")
    );
    let (s, _) = submit(
        &t,
        &user,
        "custom.admin",
        json!({"source":"files","path":"/copy.txt"}),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Replace needs edit permission.
    let reader = t.user("r", false, Perms::VIEW | Perms::DOWNLOAD, "/").await;
    let (s, _) = submit(
        &t,
        &reader,
        "custom.upper",
        json!({"source":"files","path":"/copy.txt"}),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Jobs cannot be defined through the API.
    let (s, _) = submit(&t, &admin, "custom.new", json!({"program":"/bin/sh"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread")]
async fn video_conversion_with_ffmpeg() {
    let (Some(ffmpeg), Some(_)) = (which("ffmpeg"), which("ffprobe")) else {
        eprintln!("skipping: ffmpeg/ffprobe not installed");
        return;
    };
    let t = TestApp::new().await;
    let c = t.user("admin", true, Perms::ALL, "/").await;
    // A two-second test clip with sound.
    let status = std::process::Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=15:duration=2",
            "-f",
            "lavfi",
            "-i",
            "sine=duration=2",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(t.root().join("clip.mkv"))
        .status()
        .unwrap();
    assert!(status.success());

    let (s, v) = submit(
        &t,
        &c,
        "convert.video",
        json!({"source":"files","path":"/clip.mkv","format":"mp4","mode":"remux"}),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let job = finished(&t, &c, v["id"].as_i64().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(job["result"]["path"], "/clip.mp4");

    let (_, v) = submit(
        &t,
        &c,
        "convert.video",
        json!({"source":"files","path":"/clip.mkv","format":"mp4","max_height":120}),
    )
    .await;
    let job = finished(&t, &c, v["id"].as_i64().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(job["result"]["path"], "/clip (1).mp4");

    let (_, v) = submit(
        &t,
        &c,
        "convert.video",
        json!({"source":"files","path":"/clip.mkv","format":"mp3"}),
    )
    .await;
    let job = finished(&t, &c, v["id"].as_i64().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert!(t.root().join("clip.mp3").metadata().unwrap().len() > 1000);
}

#[tokio::test(flavor = "multi_thread")]
async fn document_conversion_with_libreoffice() {
    if which("soffice").is_none() {
        eprintln!("skipping: LibreOffice not installed");
        return;
    }
    let t = TestApp::new().await;
    let c = t.user("admin", true, Perms::ALL, "/").await;
    std::fs::write(t.root().join("notes.txt"), "Hello from Filebrary\n").unwrap();
    let (s, v) = submit(
        &t,
        &c,
        "convert.document",
        json!({"source":"files","path":"/notes.txt"}),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let job = finished(&t, &c, v["id"].as_i64().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let pdf = std::fs::read(t.root().join("notes.pdf")).unwrap();
    assert!(pdf.starts_with(b"%PDF"));

    let (s, _) = submit(
        &t,
        &c,
        "convert.document",
        json!({"source":"files","path":"/notes.pdf"}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}
