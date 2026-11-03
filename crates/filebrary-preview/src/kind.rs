//! Deciding what kind of preview a file gets.
//!
//! Classification is by extension first and by magic bytes second, so a file
//! named `photo` with JPEG content still gets a thumbnail. Nothing here reads
//! more than the first few bytes of a file.

use std::path::Path;

/// The preview family a file belongs to. Each family has its own pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    /// Formats the pure Rust `image` crate decodes: JPEG, PNG, GIF, WebP,
    /// TIFF, BMP, ICO.
    Image,
    /// Camera RAW files (CR2, CR3, NEF, ARW, DNG, RAF, ORF, RW2, PEF, ...).
    /// Previewed from the JPEG the camera embeds, in pure Rust.
    Raw,
    /// HEIC/HEIF and AVIF stills. Decoded with `heif-dec` or `ffmpeg`.
    Heif,
    /// Video containers. Frames are pulled with `ffmpeg`.
    Video,
    /// Audio files. The thumbnail is the embedded cover art, if any.
    Audio,
    /// PDF documents. Rendered with `pdftoppm`.
    Pdf,
    /// Office and OpenDocument files. Converted to PDF with LibreOffice.
    Office,
}

impl FileKind {
    /// Classify a file by its name, falling back to `head` (the first bytes
    /// of the file, 32 are enough) when the extension is unknown.
    pub fn detect(path: &Path, head: Option<&[u8]>) -> Option<FileKind> {
        let by_ext = path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(|e| Self::from_extension(&e.to_ascii_lowercase()));
        by_ext.or_else(|| head.and_then(Self::sniff))
    }

    /// Classify by lowercase extension, without the dot.
    pub fn from_extension(ext: &str) -> Option<FileKind> {
        use FileKind::*;
        Some(match ext {
            "jpg" | "jpeg" | "jpe" | "jfif" | "png" | "apng" | "gif" | "webp" | "tif" | "tiff"
            | "bmp" | "ico" => Image,
            "cr2" | "cr3" | "crw" | "nef" | "nrw" | "arw" | "srf" | "sr2" | "dng" | "raf"
            | "orf" | "rw2" | "pef" | "srw" | "x3f" | "3fr" | "iiq" | "erf" | "kdc" | "mrw"
            | "mos" | "rwl" => Raw,
            "heic" | "heif" | "hif" | "avif" => Heif,
            "mp4" | "m4v" | "mov" | "mkv" | "webm" | "avi" | "wmv" | "flv" | "mpg" | "mpeg"
            | "m2ts" | "mts" | "ts" | "3gp" | "ogv" => Video,
            "mp3" | "flac" | "ogg" | "oga" | "opus" | "m4a" | "aac" | "wav" | "wma" | "aiff"
            | "aif" | "alac" | "ape" | "wv" => Audio,
            "pdf" => Pdf,
            "doc" | "docx" | "odt" | "rtf" | "xls" | "xlsx" | "ods" | "csv" | "ppt" | "pptx"
            | "odp" | "odg" | "vsd" | "vsdx" | "pages" | "numbers" | "key" => Office,
            _ => return None,
        })
    }

    /// Classify by magic bytes.
    pub fn sniff(head: &[u8]) -> Option<FileKind> {
        use FileKind::*;
        let starts = |m: &[u8]| head.starts_with(m);
        if starts(b"\xFF\xD8\xFF")
            || starts(b"\x89PNG\r\n\x1a\n")
            || starts(b"GIF87a")
            || starts(b"GIF89a")
            || starts(b"BM")
            || (starts(b"RIFF") && head.get(8..12) == Some(b"WEBP"))
        {
            return Some(Image);
        }
        if starts(b"%PDF-") {
            return Some(Pdf);
        }
        if head.get(4..8) == Some(b"ftyp") {
            let brand = head.get(8..12).unwrap_or_default();
            return match brand {
                b"heic" | b"heix" | b"heim" | b"heis" | b"mif1" | b"msf1" | b"avif" | b"avis" => {
                    Some(Heif)
                }
                b"crx " => Some(Raw),
                b"M4A " | b"M4B " => Some(Audio),
                _ => Some(Video),
            };
        }
        if starts(b"\x1A\x45\xDF\xA3") {
            return Some(Video); // Matroska / WebM
        }
        if starts(b"fLaC") || starts(b"ID3") || starts(b"OggS") {
            return Some(Audio);
        }
        if starts(b"II*\0") || starts(b"MM\0*") {
            // Plain TIFF and most TIFF-based RAW files share this header. The
            // RAW path handles both (it falls back to a full TIFF decode).
            return Some(Raw);
        }
        None
    }

    /// Whether browsers can show the original file directly, so the "large"
    /// preview can simply be the file itself.
    pub fn browser_native(path: &Path) -> bool {
        matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .as_deref(),
            Some(
                "jpg"
                    | "jpeg"
                    | "jpe"
                    | "jfif"
                    | "png"
                    | "apng"
                    | "gif"
                    | "webp"
                    | "bmp"
                    | "ico"
                    | "avif"
            )
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_are_case_insensitive() {
        assert_eq!(
            FileKind::detect(Path::new("a/B.JPG"), None),
            Some(FileKind::Image)
        );
        assert_eq!(
            FileKind::detect(Path::new("x.NeF"), None),
            Some(FileKind::Raw)
        );
        assert_eq!(
            FileKind::detect(Path::new("x.HEIC"), None),
            Some(FileKind::Heif)
        );
        assert_eq!(
            FileKind::detect(Path::new("x.docx"), None),
            Some(FileKind::Office)
        );
    }

    #[test]
    fn sniff_falls_back_on_magic() {
        assert_eq!(
            FileKind::detect(Path::new("noext"), Some(b"\xFF\xD8\xFF\xE0")),
            Some(FileKind::Image)
        );
        assert_eq!(
            FileKind::detect(Path::new("noext"), Some(b"%PDF-1.7")),
            Some(FileKind::Pdf)
        );
        assert_eq!(
            FileKind::detect(Path::new("noext"), Some(b"\0\0\0\x18ftypheic\0\0")),
            Some(FileKind::Heif)
        );
        assert_eq!(
            FileKind::detect(Path::new("noext"), Some(b"\0\0\0\x18ftypisom\0\0")),
            Some(FileKind::Video)
        );
        assert_eq!(
            FileKind::detect(Path::new("notes.txt"), Some(b"hello")),
            None
        );
    }
}
