//! Video and audio through `ffprobe` and `ffmpeg`: metadata, poster frames,
//! cover art, HEIC/AVIF decoding, and on-the-fly transcoding for files the
//! browser cannot play.
//!
//! Seeking in files the browser plays natively is handled by HTTP range
//! requests on the normal download route. For transcoded streams the client
//! asks for a start time instead (see [`transcode`]), using the duration from
//! [`probe`] to draw its own seek bar.

use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;
use std::task::{Context, Poll};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::process::{Child, ChildStdout, Command};

use crate::error::{PreviewError, Result};
use crate::tools::{Tools, ffmpeg_input, run};

/// What `ffprobe` tells us about a media file, trimmed to what the UI needs.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MediaInfo {
    /// Duration in seconds, if known.
    pub duration: Option<f64>,
    /// Container format names, e.g. `mov,mp4,m4a,3gp,3g2,mj2`.
    pub container: Option<String>,
    /// Codec of the first real video stream (cover art is not counted).
    pub video_codec: Option<String>,
    /// Width of the first video stream.
    pub width: Option<u32>,
    /// Height of the first video stream.
    pub height: Option<u32>,
    /// Codec of the first audio stream.
    pub audio_codec: Option<String>,
    /// Whether the file carries embedded cover art.
    pub has_cover: bool,
    /// `title` tag, if present.
    pub title: Option<String>,
    /// `artist` tag, if present.
    pub artist: Option<String>,
    /// `album` tag, if present.
    pub album: Option<String>,
}

impl MediaInfo {
    /// Whether a current browser (Chrome, Firefox, Safari) can most likely
    /// play this file as-is. When it cannot, the UI should use [`transcode`].
    pub fn browser_playable(&self) -> bool {
        let container = self.container.as_deref().unwrap_or_default();
        let ok_container = [
            "mp4", "mov", "webm", "matroska", "ogg", "mp3", "flac", "wav", "aac",
        ]
        .iter()
        .any(|c| container.split(',').any(|x| x == *c));
        let ok_video = match self.video_codec.as_deref() {
            None => true,
            Some(v) => matches!(v, "h264" | "vp8" | "vp9" | "av1"),
        };
        let ok_audio = match self.audio_codec.as_deref() {
            None => true,
            Some(a) => {
                matches!(a, "aac" | "mp3" | "opus" | "vorbis" | "flac") || a.starts_with("pcm_")
            }
        };
        ok_container && ok_video && ok_audio
    }
}

#[derive(Deserialize)]
struct ProbeOut {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    #[serde(default)]
    disposition: ProbeDisposition,
}

#[derive(Deserialize, Default)]
struct ProbeDisposition {
    #[serde(default)]
    attached_pic: u8,
}

#[derive(Deserialize)]
struct ProbeFormat {
    format_name: Option<String>,
    duration: Option<String>,
    #[serde(default)]
    tags: std::collections::HashMap<String, String>,
}

/// Parse `ffprobe -print_format json` output.
pub fn parse_probe(json: &[u8]) -> Result<MediaInfo> {
    let p: ProbeOut =
        serde_json::from_slice(json).map_err(|e| PreviewError::Decode(e.to_string()))?;
    let mut info = MediaInfo::default();
    for s in &p.streams {
        match s.codec_type.as_deref() {
            Some("video") if s.disposition.attached_pic == 1 => info.has_cover = true,
            Some("video") if info.video_codec.is_none() => {
                info.video_codec = s.codec_name.clone();
                info.width = s.width;
                info.height = s.height;
            }
            Some("audio") if info.audio_codec.is_none() => info.audio_codec = s.codec_name.clone(),
            _ => {}
        }
    }
    if let Some(f) = p.format {
        info.container = f.format_name;
        info.duration = f
            .duration
            .and_then(|d| d.parse().ok())
            .filter(|d: &f64| d.is_finite());
        let tag = |k: &str| {
            f.tags
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(k))
                .map(|(_, v)| v.clone())
        };
        info.title = tag("title");
        info.artist = tag("artist");
        info.album = tag("album");
    }
    Ok(info)
}

/// Run `ffprobe` on a local file.
pub async fn probe(tools: &Tools, path: &Path, timeout: Duration) -> Result<MediaInfo> {
    let ffprobe = tools
        .ffprobe
        .as_deref()
        .ok_or(PreviewError::ToolMissing("ffprobe"))?;
    let mut args: Vec<std::ffi::OsString> = [
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    args.push(ffmpeg_input(path));
    let out = run("ffprobe", ffprobe, args, timeout).await?;
    parse_probe(&out)
}

/// Pick a poster-frame time: 10% into the video, at most one minute in.
pub fn poster_time(duration: Option<f64>) -> f64 {
    duration.map(|d| (d * 0.1).min(60.0)).unwrap_or(0.0)
}

/// Grab one frame of a video at `at` seconds, scaled by ffmpeg to fit a
/// `max` x `max` box, as PNG bytes.
pub async fn video_frame(
    tools: &Tools,
    path: &Path,
    at: f64,
    max: u32,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let ffmpeg = tools
        .ffmpeg
        .as_deref()
        .ok_or(PreviewError::ToolMissing("ffmpeg"))?;
    let scale = format!("scale=w={max}:h={max}:force_original_aspect_ratio=decrease");
    let frame = |at: f64| {
        let mut a: Vec<std::ffi::OsString> = vec!["-nostdin".into(), "-v".into(), "error".into()];
        if at > 0.0 {
            a.extend(["-ss".into(), format!("{at:.3}").into()]);
        }
        a.extend(["-i".into(), ffmpeg_input(path)]);
        a.extend(
            [
                "-map",
                "0:V:0",
                "-frames:v",
                "1",
                "-vf",
                &scale,
                "-f",
                "image2pipe",
                "-c:v",
                "png",
                "pipe:1",
            ]
            .into_iter()
            .map(Into::into),
        );
        a
    };
    let out = run("ffmpeg", ffmpeg, frame(at), timeout).await?;
    if !out.is_empty() {
        return Ok(out);
    }
    // Seeking past the end of a short or badly indexed file yields nothing:
    // fall back to the very first frame.
    let out = run("ffmpeg", ffmpeg, frame(0.0), timeout).await?;
    if out.is_empty() {
        return Err(PreviewError::Decode("no video frame".into()));
    }
    Ok(out)
}

/// Extract embedded cover art (ID3 APIC, FLAC picture, MP4 `covr`, ...) as
/// PNG bytes. Callers should check [`MediaInfo::has_cover`] first.
pub async fn cover_art(tools: &Tools, path: &Path, timeout: Duration) -> Result<Vec<u8>> {
    let ffmpeg = tools
        .ffmpeg
        .as_deref()
        .ok_or(PreviewError::ToolMissing("ffmpeg"))?;
    let mut a: Vec<std::ffi::OsString> =
        vec!["-nostdin".into(), "-v".into(), "error".into(), "-i".into()];
    a.push(ffmpeg_input(path));
    a.extend(
        [
            "-an",
            "-map",
            "0:v:0",
            "-frames:v",
            "1",
            "-f",
            "image2pipe",
            "-c:v",
            "png",
            "pipe:1",
        ]
        .into_iter()
        .map(Into::into),
    );
    let out = run("ffmpeg", ffmpeg, a, timeout).await?;
    if out.is_empty() {
        return Err(PreviewError::Unsupported);
    }
    Ok(out)
}

/// Decode a HEIC/HEIF/AVIF still to PNG bytes. Uses `heif-dec` when present
/// (handles grids, HDR and every libheif codec), else `ffmpeg`.
/// Temporary files go under `tmp`.
pub async fn heif_to_png(
    tools: &Tools,
    path: &Path,
    tmp: &Path,
    timeout: Duration,
) -> Result<Vec<u8>> {
    if let Some(heif) = tools.heif_dec.as_deref() {
        let dir = tempfile::tempdir_in(tmp)?;
        let out = dir.path().join("out.png");
        run(
            "heif-dec",
            heif,
            [path.as_os_str(), out.as_os_str()],
            timeout,
        )
        .await?;
        // Files with several images get numbered outputs (out-1.png, ...).
        let first = if out.exists() {
            out
        } else {
            let mut names: Vec<_> = std::fs::read_dir(dir.path())?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "png"))
                .collect();
            names.sort();
            names
                .into_iter()
                .next()
                .ok_or_else(|| PreviewError::Decode("heif-dec wrote nothing".into()))?
        };
        return Ok(tokio::fs::read(first).await?);
    }
    let ffmpeg = tools
        .ffmpeg
        .as_deref()
        .ok_or(PreviewError::ToolMissing("heif-dec or ffmpeg"))?;
    let mut a: Vec<std::ffi::OsString> =
        vec!["-nostdin".into(), "-v".into(), "error".into(), "-i".into()];
    a.push(ffmpeg_input(path));
    a.extend(
        [
            "-frames:v",
            "1",
            "-f",
            "image2pipe",
            "-c:v",
            "png",
            "pipe:1",
        ]
        .into_iter()
        .map(Into::into),
    );
    let out = run("ffmpeg", ffmpeg, a, timeout).await?;
    if out.is_empty() {
        return Err(PreviewError::Decode("ffmpeg produced no image".into()));
    }
    Ok(out)
}

/// What a transcoded stream should contain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscodeTarget {
    /// H.264 + AAC in fragmented MP4, scaled down to at most this height.
    Video {
        /// Maximum output height in pixels (720 is a good default).
        max_height: u32,
    },
    /// AAC audio in fragmented MP4, for audio formats browsers cannot play.
    Audio,
}

/// Build the ffmpeg argument list for [`transcode`]. Streams already in a
/// browser codec are copied instead of re-encoded.
pub fn transcode_args(
    path: &Path,
    start: f64,
    info: &MediaInfo,
    target: TranscodeTarget,
) -> Vec<std::ffi::OsString> {
    let mut a: Vec<std::ffi::OsString> = vec!["-nostdin".into(), "-v".into(), "error".into()];
    if start > 0.0 && start.is_finite() {
        a.extend(["-ss".into(), format!("{start:.3}").into()]);
    }
    a.extend(["-i".into(), ffmpeg_input(path)]);
    let s = |v: &[&str]| {
        v.iter()
            .map(|x| std::ffi::OsString::from(*x))
            .collect::<Vec<_>>()
    };
    match target {
        TranscodeTarget::Video { max_height } => {
            a.extend(s(&["-map", "0:V:0", "-map", "0:a:0?"]));
            let fits = info.height.is_none_or(|h| h <= max_height);
            if info.video_codec.as_deref() == Some("h264") && fits {
                a.extend(s(&["-c:v", "copy"]));
            } else {
                let vf = format!("scale=-2:'min({max_height},ih)'");
                a.extend(s(&[
                    "-c:v", "libx264", "-preset", "veryfast", "-crf", "23", "-pix_fmt", "yuv420p",
                    "-vf",
                ]));
                a.push(vf.into());
            }
        }
        TranscodeTarget::Audio => a.extend(s(&["-vn", "-map", "0:a:0"])),
    }
    if matches!(info.audio_codec.as_deref(), Some("aac")) {
        a.extend(s(&["-c:a", "copy"]));
    } else {
        a.extend(s(&["-c:a", "aac", "-b:a", "192k", "-ac", "2"]));
    }
    a.extend(s(&[
        "-movflags",
        "frag_keyframe+empty_moov+default_base_moof",
        "-f",
        "mp4",
        "pipe:1",
    ]));
    a
}

/// A running ffmpeg transcode. Read it like any [`AsyncRead`] (it is
/// fragmented MP4 data); dropping it kills ffmpeg.
pub struct TranscodeStream {
    _child: Child,
    stdout: ChildStdout,
}

impl AsyncRead for TranscodeStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}

/// Start transcoding `path` from `start` seconds for playback in a browser.
pub fn transcode(
    tools: &Tools,
    path: &Path,
    start: f64,
    info: &MediaInfo,
    target: TranscodeTarget,
) -> Result<TranscodeStream> {
    let ffmpeg = tools
        .ffmpeg
        .as_deref()
        .ok_or(PreviewError::ToolMissing("ffmpeg"))?;
    let mut child = Command::new(ffmpeg)
        .args(transcode_args(path, start, info, target))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child.stdout.take().expect("piped");
    Ok(TranscodeStream {
        _child: child,
        stdout,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROBE: &str = r#"{
      "streams": [
        {"codec_type": "audio", "codec_name": "mp3"},
        {"codec_type": "video", "codec_name": "mjpeg", "width": 500, "height": 500,
         "disposition": {"attached_pic": 1}}
      ],
      "format": {"format_name": "mp3", "duration": "187.5",
                 "tags": {"TITLE": "Song", "artist": "Band"}}
    }"#;

    #[test]
    fn probe_parsing_separates_cover_art_from_video() {
        let i = parse_probe(PROBE.as_bytes()).unwrap();
        assert!(i.has_cover);
        assert_eq!(i.video_codec, None);
        assert_eq!(i.audio_codec.as_deref(), Some("mp3"));
        assert_eq!(i.duration, Some(187.5));
        assert_eq!(i.title.as_deref(), Some("Song"));
        assert_eq!(i.artist.as_deref(), Some("Band"));
        assert!(i.browser_playable());
    }

    #[test]
    fn hevc_mkv_is_not_browser_playable() {
        let i = MediaInfo {
            container: Some("matroska,webm".into()),
            video_codec: Some("hevc".into()),
            audio_codec: Some("ac3".into()),
            ..Default::default()
        };
        assert!(!i.browser_playable());
    }

    #[test]
    fn poster_time_is_bounded() {
        assert_eq!(poster_time(None), 0.0);
        assert_eq!(poster_time(Some(20.0)), 2.0);
        assert_eq!(poster_time(Some(7200.0)), 60.0);
    }

    #[test]
    fn transcode_copies_h264_and_aac() {
        let info = MediaInfo {
            video_codec: Some("h264".into()),
            audio_codec: Some("aac".into()),
            height: Some(720),
            ..Default::default()
        };
        let args = transcode_args(
            Path::new("/m/a.mkv"),
            12.5,
            &info,
            TranscodeTarget::Video { max_height: 720 },
        );
        let joined: Vec<_> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let joined = joined.join(" ");
        assert!(joined.contains("-ss 12.500 -i file:/m/a.mkv"));
        assert!(joined.contains("-c:v copy"));
        assert!(joined.contains("-c:a copy"));
        assert!(!joined.contains("libx264"));
    }

    #[test]
    fn transcode_reencodes_hevc() {
        let info = MediaInfo {
            video_codec: Some("hevc".into()),
            height: Some(2160),
            ..Default::default()
        };
        let args = transcode_args(
            Path::new("/m/a.mkv"),
            0.0,
            &info,
            TranscodeTarget::Video { max_height: 720 },
        );
        let joined: Vec<_> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(joined.contains(&"libx264".to_string()));
        assert!(!joined.contains(&"-ss".to_string()));
    }
}
