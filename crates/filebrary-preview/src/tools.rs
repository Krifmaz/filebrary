//! Finding and running external tools.
//!
//! Every tool is started with [`tokio::process::Command`] and an explicit
//! argument vector: no shell is involved, so file names are never parsed as
//! shell syntax. Input paths are always absolute (they come from a storage
//! root), so they can never be mistaken for a command-line option, and
//! `ffmpeg` inputs are additionally prefixed with `file:` so a name such as
//! `http:x` is not treated as a protocol.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::error::{PreviewError, Result};

/// Paths to the external programs previews can use. A `None` entry means the
/// tool is not available and the matching file types get no preview (or a
/// pure Rust fallback where one exists).
#[derive(Debug, Clone, Default)]
pub struct Tools {
    /// `ffmpeg`: video frames, audio cover art, HEIC/AVIF fallback, transcoding.
    pub ffmpeg: Option<PathBuf>,
    /// `ffprobe`: media duration and stream metadata.
    pub ffprobe: Option<PathBuf>,
    /// `pdftoppm` (poppler-utils): first page of a PDF as an image.
    pub pdftoppm: Option<PathBuf>,
    /// `soffice` (LibreOffice): Office documents to PDF.
    pub soffice: Option<PathBuf>,
    /// `heif-dec` or `heif-convert` (libheif examples): HEIC/AVIF to PNG.
    pub heif_dec: Option<PathBuf>,
}

impl Tools {
    /// Look every tool up on `PATH`.
    pub fn detect() -> Tools {
        Tools {
            ffmpeg: find_in_path("ffmpeg"),
            ffprobe: find_in_path("ffprobe"),
            pdftoppm: find_in_path("pdftoppm"),
            soffice: find_in_path("soffice").or_else(|| find_in_path("libreoffice")),
            heif_dec: find_in_path("heif-dec").or_else(|| find_in_path("heif-convert")),
        }
    }

    /// A one-line summary for the startup log, such as
    /// `ffmpeg=yes ffprobe=yes pdftoppm=no soffice=no heif-dec=no`.
    pub fn summary(&self) -> String {
        let yn = |o: &Option<PathBuf>| if o.is_some() { "yes" } else { "no" };
        format!(
            "ffmpeg={} ffprobe={} pdftoppm={} soffice={} heif-dec={}",
            yn(&self.ffmpeg),
            yn(&self.ffprobe),
            yn(&self.pdftoppm),
            yn(&self.soffice),
            yn(&self.heif_dec)
        )
    }
}

/// Search `PATH` for an executable called `name` (with `.exe` on Windows).
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    std::env::split_paths(&path)
        .map(|dir| dir.join(&file))
        .find(|p| is_executable(p))
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Build the `file:` URL-ish argument `ffmpeg` should get for a local path.
pub fn ffmpeg_input(path: &Path) -> std::ffi::OsString {
    let mut s = std::ffi::OsString::from("file:");
    s.push(path.as_os_str());
    s
}

/// Run `program` with `args`, wait at most `timeout`, and return its stdout.
///
/// The child gets no stdin, is killed if the timeout passes or the future is
/// dropped, and on failure the tail of its stderr goes into the error.
pub async fn run<I, S>(
    tool: &'static str,
    program: &Path,
    args: I,
    timeout: Duration,
) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    tracing::debug!(tool, ?cmd, "running preview tool");
    let mut child = cmd.spawn()?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");

    let work = async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let (a, b, status) = tokio::join!(
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err),
            child.wait()
        );
        a?;
        b?;
        Ok::<_, std::io::Error>((status?, out, err))
    };

    match tokio::time::timeout(timeout, work).await {
        Err(_) => Err(PreviewError::Timeout(tool)),
        Ok(Err(e)) => Err(e.into()),
        Ok(Ok((status, out, err))) => {
            if status.success() {
                Ok(out)
            } else {
                Err(PreviewError::Tool {
                    tool,
                    message: stderr_tail(&err),
                })
            }
        }
    }
}

/// Keep the last few hundred bytes of stderr, which is where tools put the
/// actual reason they failed.
fn stderr_tail(err: &[u8]) -> String {
    let s = String::from_utf8_lossy(err);
    let s = s.trim();
    let start = s.len().saturating_sub(400);
    let start = (start..s.len())
        .find(|&i| s.is_char_boundary(i))
        .unwrap_or(s.len());
    s[start..].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_input_prefixes_file_protocol() {
        assert_eq!(
            ffmpeg_input(Path::new("/srv/http:x.mp4")),
            "file:/srv/http:x.mp4"
        );
    }

    #[test]
    fn stderr_tail_keeps_the_end() {
        let long = "x".repeat(1000) + "the real error";
        let t = stderr_tail(long.as_bytes());
        assert!(t.ends_with("the real error"));
        assert!(t.len() <= 400);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_reports_failures_and_timeouts() {
        let sh = Path::new("/bin/sh");
        let out = run("sh", sh, ["-c", "printf ok"], Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(out, b"ok");
        let err = run(
            "sh",
            sh,
            ["-c", "echo boom >&2; exit 3"],
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, PreviewError::Tool { message, .. } if message == "boom"));
        let err = run("sh", sh, ["-c", "sleep 5"], Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(matches!(err, PreviewError::Timeout("sh")));
    }
}
