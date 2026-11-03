//! PDF and Office documents: first-page images with `pdftoppm`, and Office
//! to PDF conversion with LibreOffice running headless.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{PreviewError, Result};
use crate::tools::{Tools, run};

/// Render the first page of a PDF to PNG bytes, scaled so its longer side is
/// `max` pixels.
pub async fn pdf_first_page(
    tools: &Tools,
    pdf: &Path,
    max: u32,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let pdftoppm = tools
        .pdftoppm
        .as_deref()
        .ok_or(PreviewError::ToolMissing("pdftoppm"))?;
    let dir = tempfile::tempdir()?;
    let prefix = dir.path().join("page");
    let scale = max.to_string();
    run(
        "pdftoppm",
        pdftoppm,
        [
            "-f".as_ref(),
            "1".as_ref(),
            "-l".as_ref(),
            "1".as_ref(),
            "-singlefile".as_ref(),
            "-png".as_ref(),
            "-scale-to".as_ref(),
            scale.as_ref(),
            pdf.as_os_str(),
            prefix.as_os_str(),
        ],
        timeout,
    )
    .await?;
    let png = dir.path().join("page.png");
    tokio::fs::read(&png)
        .await
        .map_err(|_| PreviewError::Decode("pdftoppm wrote no page".into()))
}

/// Convert an Office/OpenDocument file to PDF with LibreOffice and return the
/// PDF bytes.
///
/// Each run gets its own throwaway LibreOffice profile directory, so several
/// conversions can run at once without fighting over the profile lock, and
/// nothing a document does can persist into later runs.
pub async fn office_to_pdf(tools: &Tools, input: &Path, timeout: Duration) -> Result<Vec<u8>> {
    let soffice = tools
        .soffice
        .as_deref()
        .ok_or(PreviewError::ToolMissing("soffice"))?;
    let work = tempfile::tempdir()?;
    let profile = work.path().join("profile");
    let outdir = work.path().join("out");
    std::fs::create_dir(&outdir)?;
    let profile_arg = format!("-env:UserInstallation={}", file_url(&profile));
    run(
        "soffice",
        soffice,
        [
            "--headless".as_ref(),
            "--norestore".as_ref(),
            "--nologo".as_ref(),
            "--nodefault".as_ref(),
            "--nolockcheck".as_ref(),
            profile_arg.as_ref(),
            "--convert-to".as_ref(),
            "pdf".as_ref(),
            "--outdir".as_ref(),
            outdir.as_os_str(),
            input.as_os_str(),
        ],
        timeout,
    )
    .await?;
    let pdf = find_pdf(&outdir)?;
    Ok(tokio::fs::read(pdf).await?)
}

fn find_pdf(dir: &Path) -> Result<PathBuf> {
    std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().is_some_and(|e| e == "pdf"))
        .ok_or_else(|| PreviewError::Decode("LibreOffice produced no PDF".into()))
}

/// Turn an absolute path into a `file://` URL, percent-encoding everything
/// outside the unreserved set.
fn file_url(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/'); // Windows drive letter paths
    }
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_url_encodes_specials() {
        assert_eq!(file_url(Path::new("/tmp/a b/p%")), "file:///tmp/a%20b/p%25");
    }
}
