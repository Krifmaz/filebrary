//! End-to-end preview pipelines against a temporary directory.
//!
//! Tests that need an external tool skip themselves (with a note on stderr)
//! when the tool is not installed, so the suite passes on the slim CI runners
//! and exercises everything on runners with the conversion tools.

use std::path::{Path, PathBuf};
use std::time::Duration;

use filebrary_preview::{
    FileKind, PreviewCache, PreviewConfig, PreviewError, PreviewInput, PreviewOutput,
    PreviewService, Tools, Variant,
};
use image::{DynamicImage, Rgb, RgbImage};
use tokio::io::AsyncReadExt;

fn service(dir: &Path) -> PreviewService {
    let cache = PreviewCache::new(dir.join("cache"), 64 << 20).unwrap();
    PreviewService::new(PreviewConfig::default(), Tools::detect(), cache)
}

fn input(path: &Path) -> PreviewInput {
    let meta = std::fs::metadata(path).unwrap();
    PreviewInput {
        source: "test".into(),
        path: format!("/{}", path.file_name().unwrap().to_string_lossy()),
        local: path.to_path_buf(),
        size: meta.len(),
        modified: meta.modified().ok(),
    }
}

fn cached(out: PreviewOutput) -> filebrary_preview::CachedFile {
    match out {
        PreviewOutput::Cached(c) => c,
        PreviewOutput::Original => panic!("expected a cached preview"),
    }
}

fn dims(path: &Path) -> (u32, u32) {
    image::image_dimensions(path).unwrap()
}

fn have(tool: &str) -> bool {
    let ok = filebrary_preview::tools::find_in_path(tool).is_some();
    if !ok {
        eprintln!("skipping: {tool} not installed");
    }
    ok
}

/// Run a tool to build a fixture (test setup only, fixed arguments).
fn fixture(program: &str, args: &[&str]) {
    let st = std::process::Command::new(program)
        .args(args)
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "{program} failed: {}",
        String::from_utf8_lossy(&st.stderr)
    );
}

fn write_jpeg(path: &Path, w: u32, h: u32) {
    let img = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        Rgb([(x % 255) as u8, (y % 255) as u8, 90])
    }));
    img.save(path).unwrap();
}

#[tokio::test]
async fn image_thumbnail_is_cached_and_large_is_original() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path());
    let p = dir.path().join("photo.jpg");
    write_jpeg(&p, 1200, 800);

    let first = cached(svc.get(&input(&p), Variant::Thumb).await.unwrap());
    assert_eq!(first.content_type, "image/jpeg");
    assert_eq!(dims(&first.path), (256, 171));
    let second = cached(svc.get(&input(&p), Variant::Thumb).await.unwrap());
    assert_eq!(first, second);
    assert_eq!(
        svc.get(&input(&p), Variant::Large).await.unwrap(),
        PreviewOutput::Original
    );
}

#[tokio::test]
async fn tiff_gets_a_large_preview() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path());
    let p = dir.path().join("scan.tiff");
    write_jpeg(&p, 3000, 1000);
    let large = cached(svc.get(&input(&p), Variant::Large).await.unwrap());
    assert_eq!(dims(&large.path), (1920, 640));
}

#[tokio::test]
async fn editing_a_file_invalidates_its_thumbnail() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path());
    let p = dir.path().join("a.png");
    write_jpeg(&p, 400, 400);
    let a = cached(svc.get(&input(&p), Variant::Thumb).await.unwrap());
    write_jpeg(&p, 400, 200);
    let mut changed = input(&p);
    changed.modified = changed.modified.map(|m| m + Duration::from_secs(1));
    let b = cached(svc.get(&changed, Variant::Thumb).await.unwrap());
    assert_ne!(a.path, b.path);
    assert_eq!(dims(&b.path), (256, 128));
}

#[tokio::test]
async fn corrupt_files_fail_once_then_are_remembered() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path());
    let p = dir.path().join("broken.jpg");
    std::fs::write(&p, b"\xFF\xD8\xFF\xE0 definitely not a jpeg").unwrap();
    assert!(matches!(
        svc.get(&input(&p), Variant::Thumb).await,
        Err(PreviewError::Decode(_))
    ));
    assert!(matches!(
        svc.get(&input(&p), Variant::Thumb).await,
        Err(PreviewError::RecentlyFailed)
    ));
}

#[tokio::test]
async fn unknown_files_are_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path());
    let p = dir.path().join("notes.txt");
    std::fs::write(&p, "hello").unwrap();
    assert!(matches!(
        svc.get(&input(&p), Variant::Thumb).await,
        Err(PreviewError::Unsupported)
    ));
}

#[tokio::test]
async fn concurrent_requests_generate_once() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path());
    let p = dir.path().join("big.png");
    write_jpeg(&p, 2000, 2000);
    let inp = input(&p);
    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let (svc, inp) = (svc.clone(), inp.clone());
            tokio::spawn(async move { cached(svc.get(&inp, Variant::Thumb).await.unwrap()) })
        })
        .collect();
    let mut paths = Vec::new();
    for t in tasks {
        paths.push(t.await.unwrap().path);
    }
    paths.dedup();
    assert_eq!(paths.len(), 1);
    let entries: Vec<PathBuf> = walk(svc.cache().root());
    assert_eq!(entries.len(), 1, "exactly one cache file: {entries:?}");
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for shard in std::fs::read_dir(root).unwrap() {
        for f in std::fs::read_dir(shard.unwrap().path()).unwrap() {
            out.push(f.unwrap().path());
        }
    }
    out
}

#[tokio::test]
async fn video_poster_frame_and_transcode() {
    if !have("ffmpeg") || !have("ffprobe") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path());
    let p = dir.path().join("clip.mkv");
    let ps = p.to_str().unwrap();
    fixture(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=640x360:rate=10:duration=3",
            "-f",
            "lavfi",
            "-i",
            "sine=duration=3",
            "-c:v",
            "mpeg4",
            "-c:a",
            "mp2",
            "-shortest",
            ps,
        ],
    );
    let thumb = cached(svc.get(&input(&p), Variant::Thumb).await.unwrap());
    assert_eq!(dims(&thumb.path), (256, 144));

    let info = svc.probe(&input(&p)).await.unwrap();
    assert!((info.duration.unwrap() - 3.0).abs() < 0.5);
    assert_eq!(info.video_codec.as_deref(), Some("mpeg4"));
    assert!(!info.browser_playable());

    let mut stream = svc.transcode(&input(&p), 1.0, true).await.unwrap();
    let mut out = Vec::new();
    stream.read_to_end(&mut out).await.unwrap();
    assert!(out.len() > 1000);
    assert_eq!(&out[4..8], b"ftyp", "fragmented MP4 output");
}

#[tokio::test]
async fn audio_cover_art() {
    if !have("ffmpeg") || !have("ffprobe") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path());
    let cover = dir.path().join("cover.jpg");
    write_jpeg(&cover, 500, 500);
    let with = dir.path().join("song.mp3");
    let without = dir.path().join("plain.mp3");
    fixture(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=duration=1",
            "-i",
            cover.to_str().unwrap(),
            "-map",
            "0",
            "-map",
            "1",
            "-c:v",
            "mjpeg",
            "-disposition:v",
            "attached_pic",
            "-metadata",
            "title=Tune",
            with.to_str().unwrap(),
        ],
    );
    fixture(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=duration=1",
            without.to_str().unwrap(),
        ],
    );

    let thumb = cached(svc.get(&input(&with), Variant::Thumb).await.unwrap());
    assert_eq!(dims(&thumb.path), (256, 256));
    assert_eq!(
        svc.probe(&input(&with)).await.unwrap().title.as_deref(),
        Some("Tune")
    );
    assert!(matches!(
        svc.get(&input(&without), Variant::Thumb).await,
        Err(PreviewError::Unsupported)
    ));
}

#[tokio::test]
async fn office_to_pdf_and_thumbnail() {
    if !have("soffice") || !have("pdftoppm") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path());
    // A name with spaces and a leading dash checks argument handling.
    let p = dir.path().join("-q3 report (final).csv");
    std::fs::write(&p, "region,sales\nnorth,10\nsouth,20\n").unwrap();

    let pdf = cached(svc.get(&input(&p), Variant::Pdf).await.unwrap());
    assert_eq!(pdf.content_type, "application/pdf");
    assert!(std::fs::read(&pdf.path).unwrap().starts_with(b"%PDF-"));

    let thumb = cached(svc.get(&input(&p), Variant::Thumb).await.unwrap());
    let (w, h) = dims(&thumb.path);
    assert_eq!(w.max(h), 256);

    // The PDF itself then previews as a PDF.
    let as_pdf = dir.path().join("doc.pdf");
    std::fs::copy(&pdf.path, &as_pdf).unwrap();
    assert_eq!(
        svc.get(&input(&as_pdf), Variant::Pdf).await.unwrap(),
        PreviewOutput::Original
    );
    let t = cached(svc.get(&input(&as_pdf), Variant::Thumb).await.unwrap());
    assert_eq!(dims(&t.path).0.max(dims(&t.path).1), 256);
}

#[tokio::test]
async fn heif_via_tools() {
    // AVIF goes through the same path as HEIC; ffmpeg can usually write it.
    if !have("ffmpeg") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path());
    let p = dir.path().join("pic.avif");
    let ok = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=800x600",
            "-frames:v",
            "1",
            p.to_str().unwrap(),
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("skipping: this ffmpeg cannot write AVIF");
        return;
    }
    assert_eq!(FileKind::detect(&p, None), Some(FileKind::Heif));
    let thumb = cached(svc.get(&input(&p), Variant::Thumb).await.unwrap());
    assert_eq!(dims(&thumb.path), (256, 192));
}

#[tokio::test]
async fn missing_tools_are_reported_not_cached() {
    let dir = tempfile::tempdir().unwrap();
    let cache = PreviewCache::new(dir.path().join("cache"), 1 << 20).unwrap();
    let svc = PreviewService::new(PreviewConfig::default(), Tools::default(), cache);
    let p = dir.path().join("movie.mp4");
    std::fs::write(&p, b"\0\0\0\x18ftypisom").unwrap();
    assert!(matches!(
        svc.get(&input(&p), Variant::Thumb).await,
        Err(PreviewError::ToolMissing("ffmpeg"))
    ));
    assert!(!svc.supports(FileKind::Video));
    assert!(svc.supports(FileKind::Raw));
}
