//! Archive support: browse inside zip, tar, tar.gz and 7z files without
//! extracting them, read a single entry, and create zip or tar.gz archives.
//!
//! Everything here is synchronous and works on `Read + Seek` sources so it can
//! run inside `tokio::task::spawn_blocking`. Nothing is written to disk
//! except the output stream the caller hands in, and entry names taken from an
//! archive are never used as filesystem paths: they are cleaned with
//! [`clean_entry_path`] and only ever compared or displayed.

use std::io::{self, Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

/// The archive formats Filebrary can look inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ArchiveKind {
    /// A PKZIP archive (`.zip`, and zip-based containers such as `.jar`).
    Zip,
    /// An uncompressed POSIX tar archive (`.tar`).
    Tar,
    /// A gzip-compressed tar archive (`.tar.gz`, `.tgz`).
    TarGz,
    /// A 7-Zip archive (`.7z`).
    SevenZ,
}

impl ArchiveKind {
    /// Works out the archive kind from a file name, case-insensitively.
    ///
    /// Returns `None` for names that are not a supported archive.
    pub fn from_name(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
            Some(Self::TarGz)
        } else if lower.ends_with(".tar") {
            Some(Self::Tar)
        } else if lower.ends_with(".zip") || lower.ends_with(".jar") || lower.ends_with(".cbz") {
            Some(Self::Zip)
        } else if lower.ends_with(".7z") {
            Some(Self::SevenZ)
        } else {
            None
        }
    }

    /// Works out the kind from the first bytes of a file, so a wrongly named
    /// archive is still handled with the right reader.
    pub fn sniff<R: Read + Seek>(reader: &mut R) -> io::Result<Option<Self>> {
        let mut head = [0u8; 512];
        reader.seek(SeekFrom::Start(0))?;
        let n = read_up_to(reader, &mut head)?;
        reader.seek(SeekFrom::Start(0))?;
        let head = &head[..n];
        Ok(
            if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
                Some(Self::Zip)
            } else if head.starts_with(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]) {
                Some(Self::SevenZ)
            } else if head.starts_with(&[0x1F, 0x8B]) {
                Some(Self::TarGz)
            } else if n >= 262 && &head[257..262] == b"ustar" {
                Some(Self::Tar)
            } else {
                None
            },
        )
    }

    /// The usual file extension for this kind, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::Tar => "tar",
            Self::TarGz => "tar.gz",
            Self::SevenZ => "7z",
        }
    }
}

/// One entry inside an archive, as shown to the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchiveEntry {
    /// Cleaned path inside the archive, `/`-separated, never starting with `/`
    /// and never containing `.` or `..` components.
    pub path: String,
    /// Whether the entry is a directory.
    pub is_dir: bool,
    /// Uncompressed size in bytes (0 for directories).
    pub size: u64,
    /// Compressed size in bytes when the format records one per entry.
    pub compressed_size: Option<u64>,
    /// Last modification time as Unix seconds, when known.
    pub modified: Option<i64>,
}

/// Errors from archive operations.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    /// The file is not an archive this module understands.
    #[error("unsupported or unrecognised archive format")]
    Unsupported,
    /// The archive is damaged or uses a feature we cannot read (for example
    /// encryption).
    #[error("cannot read archive: {0}")]
    Corrupt(String),
    /// The requested entry does not exist in the archive.
    #[error("entry not found in archive")]
    NotFound,
    /// The requested entry is a directory, not a file.
    #[error("entry is a directory")]
    IsDirectory,
    /// The archive has more entries, or an entry is larger, than the
    /// configured limits allow.
    #[error("archive exceeds limit: {0}")]
    TooLarge(&'static str),
    /// The operation was cancelled through its cancel flag.
    #[error("cancelled")]
    Cancelled,
    /// An I/O error from the underlying reader or writer.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Limits that protect the server from hostile archives (zip bombs, archives
/// with millions of entries).
#[derive(Debug, Clone, Copy)]
pub struct ArchiveLimits {
    /// Most entries listed from one archive.
    pub max_entries: usize,
    /// Most bytes read out of a single entry.
    pub max_entry_bytes: u64,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_entries: 100_000,
            max_entry_bytes: 4 * 1024 * 1024 * 1024,
        }
    }
}

/// Cleans an entry name taken from an archive into a safe, relative,
/// `/`-separated path.
///
/// Backslashes are treated as separators, empty and `.` components are
/// dropped, and a leading `/` or drive letter is removed. Returns `None` if
/// the name contains a `..` component, a NUL byte, or is empty after
/// cleaning: such entries are hidden from listings and cannot be read.
pub fn clean_entry_path(name: &str) -> Option<String> {
    if name.contains('\0') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for (i, part) in name.split(['/', '\\']).enumerate() {
        match part {
            "" | "." => {}
            ".." => return None,
            p if i == 0 && p.len() == 2 && p.ends_with(':') => {}
            p => parts.push(p),
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Lists every entry of an archive.
///
/// Entries whose names are unsafe (see [`clean_entry_path`]) are skipped.
/// Returns [`ArchiveError::TooLarge`] when the archive has more than
/// `limits.max_entries` entries.
pub fn list<R: Read + Seek>(
    kind: ArchiveKind,
    mut reader: R,
    limits: ArchiveLimits,
) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let mut out = Vec::new();
    let mut push = |e: ArchiveEntry| -> Result<(), ArchiveError> {
        if out.len() >= limits.max_entries {
            return Err(ArchiveError::TooLarge("too many entries"));
        }
        out.push(e);
        Ok(())
    };
    match kind {
        ArchiveKind::Zip => {
            let mut zip = zip::ZipArchive::new(reader).map_err(corrupt)?;
            for i in 0..zip.len() {
                let f = zip.by_index_raw(i).map_err(corrupt)?;
                let Some(path) = f.name().ok().and_then(|n| clean_entry_path(&n)) else {
                    continue;
                };
                push(ArchiveEntry {
                    path,
                    is_dir: f.is_dir(),
                    size: if f.is_dir() { 0 } else { f.size() },
                    compressed_size: Some(f.compressed_size()),
                    modified: f.last_modified().and_then(zip_time_to_unix),
                })?;
            }
        }
        ArchiveKind::Tar | ArchiveKind::TarGz => {
            reader.seek(SeekFrom::Start(0))?;
            let inner: Box<dyn Read> = if kind == ArchiveKind::TarGz {
                Box::new(flate2::read::GzDecoder::new(reader))
            } else {
                Box::new(reader)
            };
            let mut tar = tar::Archive::new(inner);
            for entry in tar.entries().map_err(corrupt)? {
                let entry = entry.map_err(corrupt)?;
                let header = entry.header();
                let et = header.entry_type();
                if !(et.is_file() || et.is_dir() || et == tar::EntryType::Continuous) {
                    continue;
                }
                let raw = entry.path_bytes();
                let Some(path) = clean_entry_path(&String::from_utf8_lossy(&raw)) else {
                    continue;
                };
                let is_dir = et.is_dir();
                push(ArchiveEntry {
                    path,
                    is_dir,
                    size: if is_dir { 0 } else { entry.size() },
                    compressed_size: None,
                    modified: header.mtime().ok().and_then(|m| i64::try_from(m).ok()),
                })?;
            }
        }
        ArchiveKind::SevenZ => {
            let archive =
                sevenz_rust2::Archive::read(&mut reader, &sevenz_rust2::Password::empty())
                    .map_err(corrupt)?;
            for f in &archive.files {
                if f.is_anti_item() {
                    continue;
                }
                let Some(path) = clean_entry_path(f.name()) else {
                    continue;
                };
                push(ArchiveEntry {
                    path,
                    is_dir: f.is_directory(),
                    size: f.size(),
                    compressed_size: None,
                    modified: if f.has_last_modified_date {
                        nt_time_to_unix(u64::from(f.last_modified_date()))
                    } else {
                        None
                    },
                })?;
            }
        }
    }
    Ok(out)
}

/// Returns the direct children of folder `dir` (`""` for the top level) from
/// a full listing, sorted folders first and then by name.
///
/// Many archives do not store folder entries, so folders implied by deeper
/// paths are added as synthetic directory entries with size 0.
pub fn children(entries: &[ArchiveEntry], dir: &str) -> Vec<ArchiveEntry> {
    let prefix = match clean_entry_path(dir) {
        Some(d) => format!("{d}/"),
        None => String::new(),
    };
    let mut out: std::collections::BTreeMap<String, ArchiveEntry> =
        std::collections::BTreeMap::new();
    for e in entries {
        let Some(rest) = e.path.strip_prefix(&prefix) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        match rest.split_once('/') {
            None => {
                out.insert(rest.to_owned(), e.clone());
            }
            Some((first, _)) => {
                out.entry(first.to_owned()).or_insert_with(|| ArchiveEntry {
                    path: format!("{prefix}{first}"),
                    is_dir: true,
                    size: 0,
                    compressed_size: None,
                    modified: None,
                });
            }
        }
    }
    let mut v: Vec<_> = out.into_values().collect();
    v.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.path.cmp(&b.path)));
    v
}

/// Copies one file entry out of an archive into `out`, returning the number of
/// bytes written.
///
/// `inner` is matched against the cleaned entry paths that [`list`] returns.
/// Reading stops with [`ArchiveError::TooLarge`] once more than
/// `limits.max_entry_bytes` bytes have been produced, which stops zip bombs.
pub fn read_entry<R: Read + Seek, W: Write>(
    kind: ArchiveKind,
    mut reader: R,
    inner: &str,
    limits: ArchiveLimits,
    out: &mut W,
) -> Result<u64, ArchiveError> {
    let wanted = clean_entry_path(inner).ok_or(ArchiveError::NotFound)?;
    match kind {
        ArchiveKind::Zip => {
            let mut zip = zip::ZipArchive::new(reader).map_err(corrupt)?;
            let mut index = None;
            for i in 0..zip.len() {
                let f = zip.by_index_raw(i).map_err(corrupt)?;
                if f.name().ok().and_then(|n| clean_entry_path(&n)).as_deref() == Some(&wanted) {
                    if f.is_dir() {
                        return Err(ArchiveError::IsDirectory);
                    }
                    index = Some(i);
                    break;
                }
            }
            let i = index.ok_or(ArchiveError::NotFound)?;
            let mut f = zip.by_index(i).map_err(corrupt)?;
            copy_limited(&mut f, out, limits.max_entry_bytes)
        }
        ArchiveKind::Tar | ArchiveKind::TarGz => {
            reader.seek(SeekFrom::Start(0))?;
            let inner_reader: Box<dyn Read> = if kind == ArchiveKind::TarGz {
                Box::new(flate2::read::GzDecoder::new(reader))
            } else {
                Box::new(reader)
            };
            let mut tar = tar::Archive::new(inner_reader);
            for entry in tar.entries().map_err(corrupt)? {
                let mut entry = entry.map_err(corrupt)?;
                let raw = entry.path_bytes();
                if clean_entry_path(&String::from_utf8_lossy(&raw)).as_deref() != Some(&wanted) {
                    continue;
                }
                let et = entry.header().entry_type();
                if et.is_dir() {
                    return Err(ArchiveError::IsDirectory);
                }
                if !(et.is_file() || et == tar::EntryType::Continuous) {
                    continue;
                }
                return copy_limited(&mut entry, out, limits.max_entry_bytes);
            }
            Err(ArchiveError::NotFound)
        }
        ArchiveKind::SevenZ => {
            let mut ar = sevenz_rust2::ArchiveReader::new(reader, sevenz_rust2::Password::empty())
                .map_err(corrupt)?;
            let mut result: Option<Result<u64, ArchiveError>> = None;
            ar.for_each_entries(|entry, data| {
                if clean_entry_path(entry.name()).as_deref() != Some(&wanted) {
                    // Returning true keeps going; the reader skips the data.
                    return Ok(true);
                }
                if entry.is_directory() {
                    result = Some(Err(ArchiveError::IsDirectory));
                } else {
                    result = Some(copy_limited(data, out, limits.max_entry_bytes));
                }
                Ok(false)
            })
            .map_err(corrupt)?;
            result.unwrap_or(Err(ArchiveError::NotFound))
        }
    }
}

/// Output formats for [`ArchiveBuilder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CreateFormat {
    /// A deflate-compressed zip archive.
    Zip,
    /// A gzip-compressed tar archive.
    #[serde(rename = "tar.gz", alias = "targz", alias = "tgz")]
    TarGz,
}

impl CreateFormat {
    /// The file extension for this format, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::TarGz => "tar.gz",
        }
    }
}

enum Inner<W: Write + Seek> {
    // Boxed so the builder stays small to move around.
    Zip(Box<zip::ZipWriter<W>>),
    TarGz(Box<tar::Builder<flate2::write::GzEncoder<W>>>),
}

/// Writes a new zip or tar.gz archive entry by entry.
///
/// Entry names are cleaned with [`clean_entry_path`] before they are written,
/// so an archive made here never contains absolute or `..` paths.
pub struct ArchiveBuilder<'c, W: Write + Seek> {
    inner: Inner<W>,
    cancel: &'c AtomicBool,
    bytes: u64,
}

impl<'c, W: Write + Seek> ArchiveBuilder<'c, W> {
    /// Starts a new archive of `format` written to `out`. Setting `cancel` to
    /// `true` makes the next write fail with [`ArchiveError::Cancelled`].
    pub fn new(format: CreateFormat, out: W, cancel: &'c AtomicBool) -> Self {
        let inner = match format {
            CreateFormat::Zip => Inner::Zip(Box::new(zip::ZipWriter::new(out))),
            CreateFormat::TarGz => {
                let gz = flate2::write::GzEncoder::new(out, flate2::Compression::default());
                let mut b = tar::Builder::new(gz);
                b.mode(tar::HeaderMode::Deterministic);
                Inner::TarGz(Box::new(b))
            }
        };
        Self {
            inner,
            cancel,
            bytes: 0,
        }
    }

    /// Total uncompressed bytes added so far.
    pub fn bytes_written(&self) -> u64 {
        self.bytes
    }

    /// Adds an empty directory entry.
    pub fn add_dir(&mut self, name: &str, mtime: Option<i64>) -> Result<(), ArchiveError> {
        self.check_cancel()?;
        let name = clean_entry_path(name).ok_or(ArchiveError::NotFound)?;
        match &mut self.inner {
            Inner::Zip(z) => {
                z.add_directory(format!("{name}/"), zip_options(mtime))
                    .map_err(corrupt)?;
            }
            Inner::TarGz(t) => {
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(tar::EntryType::Directory);
                h.set_size(0);
                h.set_mode(0o755);
                h.set_mtime(mtime.unwrap_or(0).max(0) as u64);
                t.append_data(&mut h, format!("{name}/"), io::empty())?;
            }
        }
        Ok(())
    }

    /// Adds a file entry of `size` bytes whose content is read from `data`.
    ///
    /// `on_progress` is called with the number of bytes copied after each
    /// chunk, so a job can report progress for large files.
    pub fn add_file<R: Read>(
        &mut self,
        name: &str,
        size: u64,
        mtime: Option<i64>,
        data: R,
        on_progress: &mut dyn FnMut(u64),
    ) -> Result<(), ArchiveError> {
        self.check_cancel()?;
        let name = clean_entry_path(name).ok_or(ArchiveError::NotFound)?;
        let mut reader = ProgressReader {
            inner: data,
            cancel: self.cancel,
            on_progress,
            total: 0,
        };
        match &mut self.inner {
            Inner::Zip(z) => {
                z.start_file(name, zip_options(mtime).large_file(size >= u32::MAX as u64))
                    .map_err(corrupt)?;
                io::copy(&mut reader, z).map_err(map_cancel)?;
            }
            Inner::TarGz(t) => {
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(tar::EntryType::Regular);
                h.set_size(size);
                h.set_mode(0o644);
                h.set_mtime(mtime.unwrap_or(0).max(0) as u64);
                // tar needs exactly `size` bytes: cut a file that grew while
                // we read it and zero-pad one that shrank, so the archive
                // stays well-formed either way.
                let exact = (&mut reader).take(size).chain(io::repeat(0)).take(size);
                t.append_data(&mut h, name, exact).map_err(map_cancel)?;
            }
        }
        self.bytes += reader.total;
        Ok(())
    }

    /// Writes the archive trailer and returns the output.
    pub fn finish(self) -> Result<W, ArchiveError> {
        match self.inner {
            Inner::Zip(z) => (*z).finish().map_err(corrupt),
            Inner::TarGz(t) => Ok((*t).into_inner()?.finish()?),
        }
    }

    fn check_cancel(&self) -> Result<(), ArchiveError> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(ArchiveError::Cancelled)
        } else {
            Ok(())
        }
    }
}

struct ProgressReader<'a, R> {
    inner: R,
    cancel: &'a AtomicBool,
    on_progress: &'a mut dyn FnMut(u64),
    total: u64,
}

impl<R: Read> Read for ProgressReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, CancelMarker));
        }
        let n = self.inner.read(buf)?;
        self.total += n as u64;
        (self.on_progress)(n as u64);
        Ok(n)
    }
}

#[derive(Debug)]
struct CancelMarker;
impl std::fmt::Display for CancelMarker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}
impl std::error::Error for CancelMarker {}

fn map_cancel(e: io::Error) -> ArchiveError {
    if e.get_ref().is_some_and(|r| r.is::<CancelMarker>()) {
        ArchiveError::Cancelled
    } else {
        ArchiveError::Io(e)
    }
}

fn zip_options(mtime: Option<i64>) -> zip::write::SimpleFileOptions {
    let mut o = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    if let Some(dt) = mtime.and_then(unix_to_zip_time) {
        o = o.last_modified_time(dt);
    }
    o
}

fn copy_limited<R: Read + ?Sized, W: Write>(
    from: &mut R,
    to: &mut W,
    max: u64,
) -> Result<u64, ArchiveError> {
    let mut limited = from.take(max.saturating_add(1));
    let n = io::copy(&mut limited, to)?;
    if n > max {
        return Err(ArchiveError::TooLarge("entry too large"));
    }
    Ok(n)
}

fn read_up_to<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

fn corrupt(e: impl std::fmt::Display) -> ArchiveError {
    ArchiveError::Corrupt(e.to_string())
}

/// Days from 1970-01-01 to the given civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn zip_time_to_unix(dt: zip::DateTime) -> Option<i64> {
    if !dt.is_valid() {
        return None;
    }
    let days = days_from_civil(
        i64::from(dt.year()),
        u32::from(dt.month()),
        u32::from(dt.day()),
    );
    Some(
        days * 86_400
            + i64::from(dt.hour()) * 3600
            + i64::from(dt.minute()) * 60
            + i64::from(dt.second()),
    )
}

fn unix_to_zip_time(t: i64) -> Option<zip::DateTime> {
    let (y, mo, d) = civil_from_days(t.div_euclid(86_400));
    let secs = t.rem_euclid(86_400);
    zip::DateTime::from_date_and_time(
        u16::try_from(y).ok()?,
        mo as u8,
        d as u8,
        (secs / 3600) as u8,
        ((secs % 3600) / 60) as u8,
        (secs % 60) as u8,
    )
    .ok()
}

/// Converts a Windows FILETIME (100 ns ticks since 1601) to Unix seconds.
fn nt_time_to_unix(ticks: u64) -> Option<i64> {
    const EPOCH_DIFF: i64 = 11_644_473_600;
    let secs = i64::try_from(ticks / 10_000_000).ok()?;
    let t = secs - EPOCH_DIFF;
    (t > 0).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn build(format: CreateFormat) -> Vec<u8> {
        let cancel = AtomicBool::new(false);
        let mut b = ArchiveBuilder::new(format, Cursor::new(Vec::new()), &cancel);
        b.add_dir("docs", Some(1_700_000_000)).unwrap();
        b.add_file(
            "docs/a.txt",
            5,
            Some(1_700_000_000),
            &b"hello"[..],
            &mut |_| {},
        )
        .unwrap();
        b.add_file("/abs/b.txt", 3, None, &b"abc"[..], &mut |_| {})
            .unwrap();
        b.finish().unwrap().into_inner()
    }

    #[test]
    fn clean_paths() {
        assert_eq!(clean_entry_path("a/b/c.txt").as_deref(), Some("a/b/c.txt"));
        assert_eq!(clean_entry_path("/a//./b").as_deref(), Some("a/b"));
        assert_eq!(clean_entry_path("C:\\x\\y").as_deref(), Some("x/y"));
        assert_eq!(clean_entry_path("../etc/passwd"), None);
        assert_eq!(clean_entry_path("a/../../b"), None);
        assert_eq!(clean_entry_path("a\\..\\b"), None);
        assert_eq!(clean_entry_path("a\0b"), None);
        assert_eq!(clean_entry_path("./"), None);
    }

    #[test]
    fn zip_roundtrip() {
        let data = build(CreateFormat::Zip);
        let mut c = Cursor::new(&data);
        assert_eq!(ArchiveKind::sniff(&mut c).unwrap(), Some(ArchiveKind::Zip));
        let entries = list(
            ArchiveKind::Zip,
            Cursor::new(&data),
            ArchiveLimits::default(),
        )
        .unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(names, ["docs", "docs/a.txt", "abs/b.txt"]);
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].size, 5);
        assert_eq!(entries[1].modified, Some(1_700_000_000));
        let mut out = Vec::new();
        read_entry(
            ArchiveKind::Zip,
            Cursor::new(&data),
            "docs/a.txt",
            ArchiveLimits::default(),
            &mut out,
        )
        .unwrap();
        assert_eq!(out, b"hello");
        assert!(matches!(
            read_entry(
                ArchiveKind::Zip,
                Cursor::new(&data),
                "docs",
                ArchiveLimits::default(),
                &mut Vec::new()
            ),
            Err(ArchiveError::IsDirectory)
        ));
        assert!(matches!(
            read_entry(
                ArchiveKind::Zip,
                Cursor::new(&data),
                "nope",
                ArchiveLimits::default(),
                &mut Vec::new()
            ),
            Err(ArchiveError::NotFound)
        ));
    }

    #[test]
    fn targz_roundtrip() {
        let data = build(CreateFormat::TarGz);
        let mut c = Cursor::new(&data);
        assert_eq!(
            ArchiveKind::sniff(&mut c).unwrap(),
            Some(ArchiveKind::TarGz)
        );
        let entries = list(
            ArchiveKind::TarGz,
            Cursor::new(&data),
            ArchiveLimits::default(),
        )
        .unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(names, ["docs", "docs/a.txt", "abs/b.txt"]);
        let mut out = Vec::new();
        read_entry(
            ArchiveKind::TarGz,
            Cursor::new(&data),
            "abs/b.txt",
            ArchiveLimits::default(),
            &mut out,
        )
        .unwrap();
        assert_eq!(out, b"abc");
    }

    #[test]
    fn entry_size_limit_stops_bombs() {
        let cancel = AtomicBool::new(false);
        let mut b = ArchiveBuilder::new(CreateFormat::Zip, Cursor::new(Vec::new()), &cancel);
        let zeros = vec![0u8; 1 << 20];
        b.add_file("big", zeros.len() as u64, None, &zeros[..], &mut |_| {})
            .unwrap();
        let data = b.finish().unwrap().into_inner();
        let limits = ArchiveLimits {
            max_entries: 10,
            max_entry_bytes: 1024,
        };
        assert!(matches!(
            read_entry(
                ArchiveKind::Zip,
                Cursor::new(&data),
                "big",
                limits,
                &mut Vec::new()
            ),
            Err(ArchiveError::TooLarge(_))
        ));
    }

    #[test]
    fn entry_count_limit() {
        let data = build(CreateFormat::Zip);
        let limits = ArchiveLimits {
            max_entries: 2,
            max_entry_bytes: 1024,
        };
        assert!(matches!(
            list(ArchiveKind::Zip, Cursor::new(&data), limits),
            Err(ArchiveError::TooLarge(_))
        ));
    }

    #[test]
    fn hostile_zip_names_are_hidden() {
        let cancel = AtomicBool::new(false);
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        z.start_file("../../evil.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(b"x").unwrap();
        z.start_file("ok.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(b"y").unwrap();
        let data = z.finish().unwrap().into_inner();
        let _ = &cancel;
        let entries = list(
            ArchiveKind::Zip,
            Cursor::new(&data),
            ArchiveLimits::default(),
        )
        .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "ok.txt");
        assert!(matches!(
            read_entry(
                ArchiveKind::Zip,
                Cursor::new(&data),
                "../../evil.txt",
                ArchiveLimits::default(),
                &mut Vec::new()
            ),
            Err(ArchiveError::NotFound)
        ));
    }

    #[test]
    fn cancel_stops_writing() {
        let cancel = AtomicBool::new(true);
        let mut b = ArchiveBuilder::new(CreateFormat::TarGz, Cursor::new(Vec::new()), &cancel);
        assert!(matches!(
            b.add_file("a", 1, None, &b"a"[..], &mut |_| {}),
            Err(ArchiveError::Cancelled)
        ));
    }

    #[test]
    fn time_conversion_roundtrip() {
        for t in [0i64, 315_532_800, 1_700_000_000, 1_709_251_199] {
            let (y, m, d) = civil_from_days(t.div_euclid(86_400));
            assert_eq!(days_from_civil(y, m, d), t.div_euclid(86_400));
        }
        let z = unix_to_zip_time(1_700_000_000).unwrap();
        assert_eq!(zip_time_to_unix(z), Some(1_700_000_000));
    }

    #[test]
    fn children_synthesises_folders() {
        let e = |p: &str, d: bool| ArchiveEntry {
            path: p.into(),
            is_dir: d,
            size: 1,
            compressed_size: None,
            modified: None,
        };
        let all = [
            e("a/b/c.txt", false),
            e("a/d.txt", false),
            e("z.txt", false),
            e("a", true),
        ];
        let top: Vec<_> = children(&all, "")
            .into_iter()
            .map(|e| (e.path, e.is_dir))
            .collect();
        assert_eq!(top, [("a".to_owned(), true), ("z.txt".to_owned(), false)]);
        let a: Vec<_> = children(&all, "/a/")
            .into_iter()
            .map(|e| (e.path, e.is_dir))
            .collect();
        assert_eq!(a, [("a/b".to_owned(), true), ("a/d.txt".to_owned(), false)]);
    }

    #[test]
    fn kind_from_name() {
        assert_eq!(ArchiveKind::from_name("a.TAR.GZ"), Some(ArchiveKind::TarGz));
        assert_eq!(ArchiveKind::from_name("a.tgz"), Some(ArchiveKind::TarGz));
        assert_eq!(ArchiveKind::from_name("a.7z"), Some(ArchiveKind::SevenZ));
        assert_eq!(ArchiveKind::from_name("a.txt"), None);
    }
}

#[cfg(test)]
mod sevenz_tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn sevenz_list_and_read() {
        let mut w = sevenz_rust2::ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
        w.push_archive_entry::<&[u8]>(sevenz_rust2::ArchiveEntry::new_directory("dir"), None)
            .unwrap();
        w.push_archive_entry(
            sevenz_rust2::ArchiveEntry::new_file("dir/one.txt"),
            Some(&b"first"[..]),
        )
        .unwrap();
        w.push_archive_entry(
            sevenz_rust2::ArchiveEntry::new_file("two.txt"),
            Some(&b"second file"[..]),
        )
        .unwrap();
        let data = w.finish().unwrap().into_inner();

        let mut c = Cursor::new(&data);
        assert_eq!(
            ArchiveKind::sniff(&mut c).unwrap(),
            Some(ArchiveKind::SevenZ)
        );
        let entries = list(
            ArchiveKind::SevenZ,
            Cursor::new(&data),
            ArchiveLimits::default(),
        )
        .unwrap();
        let mut names: Vec<_> = entries
            .iter()
            .map(|e| (e.path.as_str(), e.is_dir, e.size))
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                ("dir", true, 0),
                ("dir/one.txt", false, 5),
                ("two.txt", false, 11)
            ]
        );
        let mut out = Vec::new();
        read_entry(
            ArchiveKind::SevenZ,
            Cursor::new(&data),
            "two.txt",
            ArchiveLimits::default(),
            &mut out,
        )
        .unwrap();
        assert_eq!(out, b"second file");
        let mut out = Vec::new();
        read_entry(
            ArchiveKind::SevenZ,
            Cursor::new(&data),
            "dir/one.txt",
            ArchiveLimits::default(),
            &mut out,
        )
        .unwrap();
        assert_eq!(out, b"first");
    }
}
