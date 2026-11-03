//! Plain-text extraction from the file formats the content index understands.
//!
//! Everything here is pure Rust and runs in-process: no external programs are
//! started and nothing goes through a shell. Every extractor is bounded so a
//! hostile file (a zip bomb, a gigantic spreadsheet, a PDF that makes the
//! parser panic) cannot exhaust memory or take down the server:
//!
//! * the caller is expected to cap the input size (see
//!   [`crate::FulltextConfig::max_file_bytes`]),
//! * decompressed zip members are read through a hard byte limit,
//! * the produced text is truncated to `max_chars`,
//! * the PDF parser runs inside [`std::panic::catch_unwind`].
//!
//! Supported formats:
//!
//! | Kind        | Extensions                                                     |
//! |-------------|----------------------------------------------------------------|
//! | Plain text  | `txt`, `md`, source code, config files, … (see [`TEXT_EXTENSIONS`]) and any extension-less file that is valid UTF-8 |
//! | PDF         | `pdf`                                                          |
//! | Word        | `docx`, `docm`, `dotx`                                         |
//! | PowerPoint  | `pptx`, `pptm`, `ppsx`                                         |
//! | Spreadsheet | `xlsx`, `xlsm`, `xlsb`, `xls`, `ods`                           |
//! | OpenDocument| `odt`, `odp`, `ott`                                            |

use std::io::{Cursor, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};

use quick_xml::events::Event;
use thiserror::Error;

/// Hard cap on how many bytes are decompressed from one member of an Office
/// (zip) container. Protects against zip bombs.
pub const MAX_ZIP_MEMBER_BYTES: u64 = 64 * 1024 * 1024;

/// Hard cap on the total number of bytes decompressed from one Office
/// container across all of its members.
pub const MAX_ZIP_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

/// Extensions that are indexed as UTF-8 (or lossy-decoded) plain text.
pub const TEXT_EXTENSIONS: &[&str] = &[
    "txt",
    "text",
    "md",
    "markdown",
    "rst",
    "adoc",
    "org",
    "tex",
    "log",
    "csv",
    "tsv",
    "json",
    "jsonc",
    "json5",
    "yaml",
    "yml",
    "toml",
    "ini",
    "cfg",
    "conf",
    "env",
    "properties",
    "xml",
    "html",
    "htm",
    "xhtml",
    "css",
    "scss",
    "sass",
    "less",
    "svg",
    "rs",
    "go",
    "py",
    "rb",
    "pl",
    "php",
    "js",
    "mjs",
    "cjs",
    "jsx",
    "ts",
    "tsx",
    "vue",
    "svelte",
    "java",
    "kt",
    "kts",
    "scala",
    "groovy",
    "gradle",
    "c",
    "h",
    "cc",
    "cpp",
    "cxx",
    "hpp",
    "hh",
    "cs",
    "fs",
    "swift",
    "m",
    "mm",
    "dart",
    "lua",
    "r",
    "jl",
    "hs",
    "ml",
    "mli",
    "ex",
    "exs",
    "erl",
    "clj",
    "cljs",
    "elm",
    "zig",
    "nim",
    "v",
    "sv",
    "vhd",
    "vhdl",
    "sql",
    "sh",
    "bash",
    "zsh",
    "fish",
    "ps1",
    "psm1",
    "bat",
    "cmd",
    "dockerfile",
    "makefile",
    "mk",
    "cmake",
    "nix",
    "tf",
    "hcl",
    "proto",
    "graphql",
    "gql",
    "srt",
    "vtt",
    "sub",
    "bib",
    "patch",
    "diff",
    "lock",
    "gitignore",
    "editorconfig",
];

/// What kind of document a file is, as far as text extraction is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    /// UTF-8 (or close enough) plain text, including source code.
    Text,
    /// A PDF document.
    Pdf,
    /// An Office Open XML word-processing document (`.docx`).
    Docx,
    /// An Office Open XML presentation (`.pptx`).
    Pptx,
    /// A spreadsheet readable by `calamine` (`.xlsx`, `.xls`, `.ods`, …).
    Spreadsheet,
    /// An OpenDocument text or presentation (`.odt`, `.odp`).
    OpenDocument,
}

/// Errors returned by [`extract_text`].
#[derive(Debug, Error)]
pub enum ExtractError {
    /// The file looked like a supported format but could not be parsed.
    #[error("could not parse {kind:?} document: {message}")]
    Parse {
        /// The format that was being parsed.
        kind: DocKind,
        /// A human-readable reason.
        message: String,
    },
    /// The parser panicked on this input. The panic was contained.
    #[error("the {0:?} parser crashed on this file")]
    Panicked(DocKind),
    /// A decompression limit was hit (likely a zip bomb).
    #[error("document expands beyond the decompression limit")]
    TooLarge,
}

impl ExtractError {
    fn parse(kind: DocKind, err: impl std::fmt::Display) -> Self {
        Self::Parse {
            kind,
            message: err.to_string(),
        }
    }
}

/// Works out which extractor applies to `file_name`, from its extension.
///
/// Returns `None` for formats the content index does not read (images,
/// video, archives, binaries, …). Extension-less files are treated as
/// [`DocKind::Text`] and later rejected by [`extract_text`] if they are not
/// text.
pub fn detect_kind(file_name: &str) -> Option<DocKind> {
    let base = file_name.rsplit('/').next().unwrap_or(file_name);
    let lower = base.to_ascii_lowercase();
    // Well-known extension-less text files.
    if matches!(
        lower.as_str(),
        "readme" | "license" | "licence" | "copying" | "makefile" | "dockerfile" | "changelog"
    ) {
        return Some(DocKind::Text);
    }
    let ext = match lower.rsplit_once('.') {
        Some(("", _)) => return Some(DocKind::Text), // dotfile such as `.bashrc`
        Some((_, ext)) => ext,
        None => return Some(DocKind::Text),
    };
    Some(match ext {
        "pdf" => DocKind::Pdf,
        "docx" | "docm" | "dotx" | "dotm" => DocKind::Docx,
        "pptx" | "pptm" | "ppsx" | "potx" => DocKind::Pptx,
        "xlsx" | "xlsm" | "xlsb" | "xls" | "xla" | "xlam" | "ods" => DocKind::Spreadsheet,
        "odt" | "ott" | "odp" | "otp" => DocKind::OpenDocument,
        e if TEXT_EXTENSIONS.contains(&e) => DocKind::Text,
        _ => return None,
    })
}

/// Extracts searchable plain text from a document held in memory.
///
/// * `file_name` is used only to pick the format (see [`detect_kind`]).
/// * `max_chars` truncates the returned text; it bounds index size and
///   memory use for very large documents.
///
/// Returns `Ok(None)` when the format is not supported or the file turns out
/// not to be text (for example an extension-less binary).
pub fn extract_text(
    file_name: &str,
    bytes: &[u8],
    max_chars: usize,
) -> Result<Option<String>, ExtractError> {
    let Some(kind) = detect_kind(file_name) else {
        return Ok(None);
    };
    let text = match kind {
        DocKind::Text => match decode_text(bytes) {
            Some(t) => t,
            None => return Ok(None),
        },
        DocKind::Pdf => extract_pdf(bytes)?,
        DocKind::Docx => extract_ooxml(bytes, kind, |n| n == "word/document.xml")?,
        DocKind::Pptx => extract_ooxml(bytes, kind, |n| {
            n.starts_with("ppt/slides/slide") && n.ends_with(".xml")
        })?,
        DocKind::OpenDocument => extract_ooxml(bytes, kind, |n| n == "content.xml")?,
        DocKind::Spreadsheet => extract_spreadsheet(bytes, max_chars)?,
    };
    Ok(Some(truncate_chars(normalise_whitespace(&text), max_chars)))
}

/// Decodes `bytes` as text, or returns `None` if it looks binary.
///
/// A NUL byte in the first 8 KiB marks the file as binary. Invalid UTF-8 is
/// replaced rather than rejected so Latin-1 files are still mostly
/// searchable.
fn decode_text(bytes: &[u8]) -> Option<String> {
    let head = &bytes[..bytes.len().min(8192)];
    if head.contains(&0) {
        return None;
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    Some(String::from_utf8_lossy(bytes).into_owned())
}

/// Extracts text from a PDF, containing any panic from the parser.
fn extract_pdf(bytes: &[u8]) -> Result<String, ExtractError> {
    match catch_unwind(AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem(bytes)
    })) {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(ExtractError::parse(DocKind::Pdf, e)),
        Err(_) => Err(ExtractError::Panicked(DocKind::Pdf)),
    }
}

/// Reads the XML members of a zip container selected by `want` (in archive
/// order) and returns the concatenated character data.
fn extract_ooxml(
    bytes: &[u8],
    kind: DocKind,
    want: impl Fn(&str) -> bool,
) -> Result<String, ExtractError> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| ExtractError::parse(kind, e))?;
    let mut names: Vec<String> = archive
        .file_names()
        .filter_map(Result::ok)
        .filter(|n| want(n))
        .map(|n| n.into_owned())
        .collect();
    // Slides are named slide1.xml, slide2.xml, … slide10.xml: sort numerically.
    names.sort_by_key(|n| {
        let digits: String = n.chars().filter(char::is_ascii_digit).collect();
        (digits.parse::<u64>().unwrap_or(0), n.clone())
    });
    let mut out = String::new();
    let mut total = 0u64;
    for name in names {
        let member = archive
            .by_name(&name)
            .map_err(|e| ExtractError::parse(kind, e))?;
        let mut xml = Vec::new();
        let read = member
            .take(MAX_ZIP_MEMBER_BYTES + 1)
            .read_to_end(&mut xml)
            .map_err(|e| ExtractError::parse(kind, e))? as u64;
        total += read;
        if read > MAX_ZIP_MEMBER_BYTES || total > MAX_ZIP_TOTAL_BYTES {
            return Err(ExtractError::TooLarge);
        }
        xml_text(&xml, &mut out).map_err(|e| ExtractError::parse(kind, e))?;
        out.push('\n');
    }
    Ok(out)
}

/// Appends the character data of an XML document to `out`.
///
/// Paragraph-like elements (`w:p`, `a:p`, `text:p`, `text:h`, table cells,
/// line breaks and tabs) become whitespace so words from neighbouring
/// paragraphs are not glued together.
fn xml_text(xml: &[u8], out: &mut String) -> Result<(), quick_xml::Error> {
    // Office XML is UTF-8; lossy decoding keeps a stray bad byte from
    // losing the whole document.
    let xml = String::from_utf8_lossy(xml);
    let mut reader = quick_xml::Reader::from_str(&xml);
    loop {
        match reader.read_event()? {
            Event::Text(t) => out.push_str(&t.xml10_content()),
            Event::CData(t) => out.push_str(&t),
            Event::GeneralRef(r) => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    out.push(c);
                } else {
                    match &*r {
                        "amp" => out.push('&'),
                        "lt" => out.push('<'),
                        "gt" => out.push('>'),
                        "quot" => out.push('"'),
                        "apos" => out.push('\''),
                        _ => {}
                    }
                }
            }
            Event::End(e) => {
                if is_block(e.local_name().as_ref()) {
                    out.push('\n');
                }
            }
            Event::Empty(e) => {
                let n = e.local_name();
                if matches!(n.as_ref(), "br" | "tab" | "s" | "line-break" | "cr") {
                    out.push(' ');
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

/// Whether an element's end should be treated as a paragraph break.
fn is_block(local_name: &str) -> bool {
    matches!(
        local_name,
        "p" | "h" | "tc" | "tr" | "table-cell" | "list-item" | "txBody"
    )
}

/// Extracts the cell values of every sheet, one row per line, cells
/// separated by tabs. Stops early once `max_chars` is reached.
fn extract_spreadsheet(bytes: &[u8], max_chars: usize) -> Result<String, ExtractError> {
    use calamine::Reader as _;
    let kind = DocKind::Spreadsheet;
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<String, ExtractError> {
        let mut wb = calamine::open_workbook_auto_from_rs(Cursor::new(bytes))
            .map_err(|e| ExtractError::parse(kind, e))?;
        let mut out = String::new();
        for name in wb.sheet_names().to_vec() {
            out.push_str(&name);
            out.push('\n');
            let Ok(range) = wb.worksheet_range(&name) else {
                continue;
            };
            for row in range.rows() {
                for (i, cell) in row.iter().enumerate() {
                    if i > 0 {
                        out.push('\t');
                    }
                    out.push_str(&cell.to_string());
                }
                out.push('\n');
                if out.len() > max_chars.saturating_mul(4) {
                    return Ok(out);
                }
            }
        }
        Ok(out)
    }));
    result.unwrap_or(Err(ExtractError::Panicked(kind)))
}

/// Collapses runs of whitespace so the stored text (used for snippets) stays
/// compact. Single newlines are kept as paragraph hints.
fn normalise_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending: Option<char> = None;
    for c in text.chars() {
        if c.is_whitespace() || c.is_control() {
            if c == '\n' || pending == Some('\n') {
                pending = Some('\n');
            } else {
                pending = Some(' ');
            }
        } else {
            if let Some(p) = pending.take()
                && !out.is_empty()
            {
                out.push(p);
            }
            out.push(c);
        }
    }
    out
}

/// Truncates `text` to at most `max_chars` characters, on a char boundary.
fn truncate_chars(mut text: String, max_chars: usize) -> String {
    if let Some((idx, _)) = text.char_indices().nth(max_chars) {
        text.truncate(idx);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Builds an in-memory zip with the given members.
    pub(crate) fn zip_of(members: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, body) in members {
                w.start_file(*name, opts).unwrap();
                w.write_all(body.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn detects_kinds() {
        assert_eq!(detect_kind("a/b/notes.TXT"), Some(DocKind::Text));
        assert_eq!(detect_kind("main.rs"), Some(DocKind::Text));
        assert_eq!(detect_kind("README"), Some(DocKind::Text));
        assert_eq!(detect_kind(".bashrc"), Some(DocKind::Text));
        assert_eq!(detect_kind("x.pdf"), Some(DocKind::Pdf));
        assert_eq!(detect_kind("x.docx"), Some(DocKind::Docx));
        assert_eq!(detect_kind("x.pptx"), Some(DocKind::Pptx));
        assert_eq!(detect_kind("x.xlsx"), Some(DocKind::Spreadsheet));
        assert_eq!(detect_kind("x.ods"), Some(DocKind::Spreadsheet));
        assert_eq!(detect_kind("x.odt"), Some(DocKind::OpenDocument));
        assert_eq!(detect_kind("photo.jpg"), None);
        assert_eq!(detect_kind("movie.mkv"), None);
    }

    #[test]
    fn plain_text_and_binary() {
        let t = extract_text("a.txt", b"\xEF\xBB\xBFhello   world\n\n\nbye", 100)
            .unwrap()
            .unwrap();
        assert_eq!(t, "hello world\nbye");
        assert!(extract_text("blob", b"ELF\0\0\x01", 100).unwrap().is_none());
        assert!(extract_text("a.png", b"hello", 100).unwrap().is_none());
    }

    #[test]
    fn truncates_on_char_boundary() {
        let t = extract_text("a.txt", "héllo wörld".as_bytes(), 4)
            .unwrap()
            .unwrap();
        assert_eq!(t, "héll");
    }

    #[test]
    fn docx() {
        let doc = r#"<?xml version="1.0"?><w:document xmlns:w="w"><w:body>
            <w:p><w:r><w:t>Quarterly</w:t></w:r><w:r><w:t xml:space="preserve"> report</w:t></w:r></w:p>
            <w:p><w:r><w:t>Fish &amp; chips</w:t></w:r></w:p></w:body></w:document>"#;
        let bytes = zip_of(&[
            ("word/document.xml", doc),
            ("word/styles.xml", "<x>NOPE</x>"),
        ]);
        let t = extract_text("r.docx", &bytes, 1000).unwrap().unwrap();
        assert_eq!(t, "Quarterly report\nFish & chips");
    }

    #[test]
    fn pptx_slides_in_numeric_order() {
        let s = |w: &str| {
            format!(
                r#"<p:sld xmlns:a="a" xmlns:p="p"><a:p><a:r><a:t>{w}</a:t></a:r></a:p></p:sld>"#
            )
        };
        let (s1, s2, s10) = (s("first"), s("second"), s("tenth"));
        let bytes = zip_of(&[
            ("ppt/slides/slide10.xml", &s10),
            ("ppt/slides/slide2.xml", &s2),
            ("ppt/slides/slide1.xml", &s1),
        ]);
        let t = extract_text("deck.pptx", &bytes, 1000).unwrap().unwrap();
        assert_eq!(t, "first\nsecond\ntenth");
    }

    #[test]
    fn odt() {
        let c = r#"<office:document-content xmlns:office="o" xmlns:text="t"><office:body><office:text>
            <text:h>Title</text:h><text:p>Body<text:s/>text</text:p></office:text></office:body></office:document-content>"#;
        let bytes = zip_of(&[("content.xml", c), ("styles.xml", "<x>NOPE</x>")]);
        let t = extract_text("a.odt", &bytes, 1000).unwrap().unwrap();
        assert_eq!(t, "Title\nBody text");
    }

    #[test]
    fn corrupt_documents_are_errors_not_panics() {
        assert!(extract_text("a.docx", b"not a zip", 100).is_err());
        assert!(extract_text("a.pdf", b"%PDF-1.4 garbage", 100).is_err());
        assert!(extract_text("a.xlsx", b"garbage", 100).is_err());
    }

    #[test]
    fn zip_bomb_member_is_refused() {
        // 70 MiB of zeros compresses to ~70 KiB.
        let mut buf = Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            w.start_file("word/document.xml", opts).unwrap();
            let chunk = vec![b' '; 1024 * 1024];
            for _ in 0..70 {
                w.write_all(&chunk).unwrap();
            }
            w.finish().unwrap();
        }
        let err = extract_text("bomb.docx", &buf.into_inner(), 100).unwrap_err();
        assert!(matches!(err, ExtractError::TooLarge));
    }

    /// Builds a minimal one-page PDF showing `text` with a standard font.
    fn tiny_pdf(text: &str) -> Vec<u8> {
        let stream = format!("BT /F1 12 Tf 72 712 Td ({text}) Tj ET");
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_owned(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_owned(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (n, body) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", n + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes(),
        );
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objs.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    #[test]
    fn pdf() {
        let t = extract_text("doc.pdf", &tiny_pdf("Invoice number 4711"), 1000)
            .unwrap()
            .unwrap();
        assert!(t.contains("Invoice number 4711"), "{t:?}");
    }

    #[test]
    fn xlsx() {
        let bytes = zip_of(&[
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Budget" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Rent</t></is></c><c r="B1"><v>1200</v></c></row></sheetData></worksheet>"#,
            ),
        ]);
        let t = extract_text("b.xlsx", &bytes, 1000).unwrap().unwrap();
        assert_eq!(t, "Budget\nRent 1200");
    }
}
