//! What a file is: its view mode in the listing, the kind the preview service makes thumbnails for, which
//! viewer opens it and which icon it gets. Judged by extension and MIME type the same way the server's
//! `FileKind::detect` does.

use super::paths::extension;
use crate::api::{Entry, PreviewCapabilities};

/// One way of laying out the listing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    /// A table with sortable columns.
    Details,
    /// A compact multi-column list.
    Compact,
    /// Small tiles.
    Small,
    /// Medium tiles.
    Medium,
    /// Large tiles.
    Large,
}

impl ViewMode {
    /// Every mode, in the order the switcher shows them (keys `1`–`5`).
    pub const ALL: [ViewMode; 5] = [ViewMode::Details, ViewMode::Compact, ViewMode::Small, ViewMode::Medium, ViewMode::Large];

    /// The id stored in the browser and used as a CSS class.
    pub fn id(self) -> &'static str {
        match self {
            ViewMode::Details => "details",
            ViewMode::Compact => "compact",
            ViewMode::Small => "small",
            ViewMode::Medium => "medium",
            ViewMode::Large => "large",
        }
    }

    /// The mode's name in the switcher.
    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Details => "Details",
            ViewMode::Compact => "Compact list",
            ViewMode::Small => "Small icons",
            ViewMode::Medium => "Medium icons",
            ViewMode::Large => "Large icons",
        }
    }

    /// The switcher's icon.
    pub fn icon(self) -> &'static str {
        match self {
            ViewMode::Details => "list",
            ViewMode::Compact => "compact",
            ViewMode::Small => "grid-small",
            ViewMode::Medium => "grid",
            ViewMode::Large => "grid-large",
        }
    }

    /// The mode with this id.
    pub fn parse(s: &str) -> Option<ViewMode> {
        Self::ALL.into_iter().find(|m| m.id() == s)
    }

    /// The mode bound to a shortcut key (`1`–`5`).
    pub fn for_key(key: &str) -> Option<ViewMode> {
        let n: usize = key.parse().ok()?;
        Self::ALL.get(n.checked_sub(1)?).copied()
    }

    /// The mode `step` places on, wrapping around (the `V` and `Shift+V` shortcuts).
    pub fn step(self, step: i64) -> ViewMode {
        let i = Self::ALL.iter().position(|m| *m == self).unwrap_or(0) as i64;
        let n = Self::ALL.len() as i64;
        Self::ALL[(((i + step) % n + n) % n) as usize]
    }

    /// Whether entries are tiles with thumbnails.
    pub fn is_grid(self) -> bool {
        matches!(self, ViewMode::Small | ViewMode::Medium | ViewMode::Large)
    }
}

/// How many folder-specific choices are kept per account.
pub const MAX_FOLDERS: usize = 200;

/// What is remembered for one account: the default mode and the mode of recent folders (oldest first).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ViewMemory {
    /// The account's default.
    pub default: Option<ViewMode>,
    /// `source:path` → mode, oldest first.
    pub folders: Vec<(String, ViewMode)>,
}

impl ViewMemory {
    /// Reads the stored JSON (`filebrary.view.<user>`, the shape the old UI stored too), tolerating anything broken.
    pub fn parse(raw: &str) -> ViewMemory {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else { return ViewMemory::default() };
        let default = v.get("default").and_then(|d| d.as_str()).and_then(ViewMode::parse);
        let folders = v
            .get("folders")
            .and_then(|f| f.as_object())
            .map(|o| o.iter().filter_map(|(k, m)| Some((k.clone(), ViewMode::parse(m.as_str()?)?))).collect())
            .unwrap_or_default();
        ViewMemory { default, folders }
    }

    /// The JSON to store.
    pub fn to_json(&self) -> String {
        let mut folders = serde_json::Map::new();
        for (k, m) in &self.folders {
            folders.insert(k.clone(), m.id().into());
        }
        serde_json::json!({ "default": self.default.unwrap_or(ViewMode::Details).id(), "folders": folders }).to_string()
    }

    /// The mode to open a folder in: its own, else the account's default, else details.
    pub fn mode_for(&self, source: &str, path: &str) -> ViewMode {
        let key = format!("{source}:{path}");
        self.folders.iter().rev().find(|(k, _)| *k == key).map(|(_, m)| *m).or(self.default).unwrap_or(ViewMode::Details)
    }

    /// Remembers `mode` for one folder (most recent last, at most [`MAX_FOLDERS`]) and makes it the default.
    pub fn remember(&mut self, source: &str, path: &str, mode: ViewMode) {
        let key = format!("{source}:{path}");
        self.folders.retain(|(k, _)| *k != key);
        self.folders.push((key, mode));
        let extra = self.folders.len().saturating_sub(MAX_FOLDERS);
        self.folders.drain(..extra);
        self.default = Some(mode);
    }
}

fn storage_key(user: i64) -> String {
    format!("filebrary.view.{user}")
}

fn local_storage() -> Option<web_sys::Storage> {
    kit_leptos::rt::window().and_then(|w| w.local_storage().ok().flatten())
}

/// The mode to open `source:path` in for `user`.
pub fn load_mode(user: i64, source: &str, path: &str) -> ViewMode {
    let raw = local_storage().and_then(|s| s.get_item(&storage_key(user)).ok().flatten()).unwrap_or_default();
    ViewMemory::parse(&raw).mode_for(source, path)
}

/// Remembers `mode` for `source:path` (storage errors are ignored: the mode still applies until a reload).
pub fn save_mode(user: i64, source: &str, path: &str, mode: ViewMode) {
    let Some(s) = local_storage() else { return };
    let key = storage_key(user);
    let mut m = ViewMemory::parse(&s.get_item(&key).ok().flatten().unwrap_or_default());
    m.remember(source, path, mode);
    let _ = s.set_item(&key, &m.to_json());
}

/// The kinds the preview service makes thumbnails for.
const KIND_EXT: [(&str, &[&str]); 7] = [
    ("image", &["jpg", "jpeg", "jpe", "jfif", "png", "apng", "gif", "webp", "tif", "tiff", "bmp", "ico"]),
    (
        "raw",
        &[
            "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "dng", "raf", "orf", "rw2", "pef", "srw", "x3f", "3fr", "iiq", "erf",
            "kdc", "mrw", "mos", "rwl",
        ],
    ),
    ("heif", &["heic", "heif", "hif", "avif"]),
    ("video", &["mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv", "mpg", "mpeg", "m2ts", "mts", "ts", "3gp", "ogv"]),
    ("audio", &["mp3", "flac", "ogg", "oga", "opus", "m4a", "aac", "wav", "wma", "aiff", "aif", "alac", "ape", "wv"]),
    ("pdf", &["pdf"]),
    (
        "office",
        &["doc", "docx", "odt", "rtf", "xls", "xlsx", "ods", "csv", "ppt", "pptx", "odp", "odg", "vsd", "vsdx", "pages", "numbers", "key"],
    ),
];

/// The preview kind of a file (`image`, `raw`, `heif`, `video`, `audio`, `pdf`, `office`); `None` for folders and
/// unknown files.
pub fn preview_kind(name: &str, is_dir: bool) -> Option<&'static str> {
    if is_dir {
        return None;
    }
    let ext = extension(name);
    if ext.is_empty() {
        return None;
    }
    KIND_EXT.iter().find(|(_, exts)| exts.contains(&ext.as_str())).map(|(k, _)| *k)
}

/// Whether to ask the server for a thumbnail of this entry.
pub fn wants_thumbnail(e: &Entry, caps: Option<&PreviewCapabilities>) -> bool {
    match (preview_kind(&e.name, e.is_dir), caps) {
        (Some(k), Some(c)) => c.enabled && c.kinds.get(k).copied().unwrap_or(false),
        _ => false,
    }
}

/// Largest file the text editor opens (the server's limit, 5 MiB).
pub const MAX_TEXT_BYTES: u64 = 5 * 1024 * 1024;

const TEXT_MIMES: [&str; 21] = [
    "application/json",
    "application/ld+json",
    "application/manifest+json",
    "application/javascript",
    "application/x-javascript",
    "application/ecmascript",
    "application/typescript",
    "application/xml",
    "application/xhtml+xml",
    "application/x-sh",
    "application/x-shellscript",
    "application/x-httpd-php",
    "application/sql",
    "application/toml",
    "application/yaml",
    "application/x-yaml",
    "application/graphql",
    "application/x-tex",
    "application/x-latex",
    "application/x-subrip",
    "image/svg+xml",
];

const TEXT_EXTENSIONS: &str = "txt text md markdown mdx rst adoc asciidoc org log csv tsv \
    json jsonc json5 ndjson yaml yml toml ini cfg conf config env properties editorconfig \
    xml xsd xsl svg html htm xhtml css scss sass less vue svelte astro \
    js mjs cjs jsx ts mts cts tsx \
    rs go c h cc cpp cxx hpp hh hxx m mm java kt kts scala groovy gradle cs fs fsx vb swift dart \
    py pyi rb php pl pm lua r jl ex exs erl hrl hs elm clj cljs edn lisp el scm ml mli nim zig v d \
    sh bash zsh fish ps1 psm1 bat cmd sql graphql gql proto thrift tf hcl nix \
    tex bib sty cls srt vtt diff patch gitignore gitattributes dockerignore lock";

const TEXT_NAMES: [&str; 17] = [
    "dockerfile",
    "containerfile",
    "makefile",
    "gnumakefile",
    "justfile",
    "rakefile",
    "gemfile",
    "procfile",
    "vagrantfile",
    "readme",
    "license",
    "licence",
    "copying",
    "authors",
    "changelog",
    "notice",
    "todo",
];

/// Whether the text editor should open this entry (the server decides in the end: `415 not_text`).
pub fn is_text(e: &Entry) -> bool {
    if e.is_dir || e.size > MAX_TEXT_BYTES {
        return false;
    }
    let mime = e.mime.as_deref().unwrap_or("").to_ascii_lowercase();
    let mime = mime.split(';').next().unwrap_or("").trim();
    if mime.starts_with("text/") || TEXT_MIMES.contains(&mime) || mime.ends_with("+json") || mime.ends_with("+xml") {
        return true;
    }
    let name = e.name.to_ascii_lowercase();
    if TEXT_NAMES.contains(&name.as_str()) {
        return true;
    }
    let ext = match name.rfind('.') {
        Some(i) => &name[i + 1..],
        None => "",
    };
    !ext.is_empty() && TEXT_EXTENSIONS.split_whitespace().any(|t| t == ext)
}

/// Which viewer opens a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Viewer {
    /// A picture (the large preview, or the original when previews are off).
    Image,
    /// A video player.
    Video,
    /// An audio player.
    Audio,
    /// A PDF in the browser's own viewer.
    Pdf,
    /// The text editor.
    Text,
    /// An office document's first page as a picture.
    Office,
    /// Nothing in the page can show it: a card with its details and Download.
    Other,
}

/// The viewer for an entry (`None` for a folder).
pub fn viewer_for(e: &Entry) -> Option<Viewer> {
    if e.is_dir {
        return None;
    }
    let mime = e.mime.as_deref().unwrap_or("");
    Some(match preview_kind(&e.name, false) {
        Some("image" | "raw" | "heif") => Viewer::Image,
        Some("video") => Viewer::Video,
        Some("audio") => Viewer::Audio,
        Some("pdf") => Viewer::Pdf,
        _ if is_text(e) => Viewer::Text,
        Some("office") => Viewer::Office,
        _ if mime.starts_with("image/") => Viewer::Image,
        _ if mime.starts_with("video/") => Viewer::Video,
        _ if mime.starts_with("audio/") => Viewer::Audio,
        _ => Viewer::Other,
    })
}

/// The icon of a file or folder in the listing.
pub fn file_icon(name: &str, is_dir: bool) -> &'static str {
    if is_dir {
        return "folder";
    }
    match preview_kind(name, false) {
        Some("image" | "raw" | "heif") => return "file-image",
        Some("video") => return "file-video",
        Some("audio") => return "file-audio",
        Some("pdf") => return "file-text",
        _ => {}
    }
    match extension(name).as_str() {
        "zip" | "tar" | "gz" | "tgz" | "7z" | "rar" | "xz" | "bz2" | "zst" => "file-archive",
        "doc" | "docx" | "odt" | "rtf" | "md" | "txt" => "file-text",
        "xls" | "xlsx" | "ods" | "csv" => "file-sheet",
        "rs" | "js" | "ts" | "py" | "go" | "c" | "cpp" | "h" | "java" | "kt" | "sh" | "json" | "toml" | "yaml" | "yml" | "html" | "css" => {
            "file-code"
        }
        _ => "file",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str, mime: Option<&str>, size: u64) -> Entry {
        Entry { name: name.into(), path: format!("/{name}"), is_dir: false, size, modified: String::new(), mime: mime.map(Into::into) }
    }

    #[test]
    fn modes_round_trip_and_step() {
        for m in ViewMode::ALL {
            assert_eq!(ViewMode::parse(m.id()), Some(m));
        }
        assert_eq!(ViewMode::for_key("1"), Some(ViewMode::Details));
        assert_eq!(ViewMode::for_key("5"), Some(ViewMode::Large));
        assert_eq!(ViewMode::for_key("0"), None);
        assert_eq!(ViewMode::for_key("6"), None);
        assert_eq!(ViewMode::Large.step(1), ViewMode::Details);
        assert_eq!(ViewMode::Details.step(-1), ViewMode::Large);
        assert!(ViewMode::Small.is_grid() && !ViewMode::Compact.is_grid());
    }

    #[test]
    fn view_memory_reads_the_react_shape() {
        let m = ViewMemory::parse(r#"{"default":"medium","folders":{"home:/Photos":"large","x:/":"bogus"}}"#);
        assert_eq!(m.mode_for("home", "/Photos"), ViewMode::Large);
        assert_eq!(m.mode_for("home", "/Docs"), ViewMode::Medium);
        assert_eq!(ViewMemory::parse("not json").mode_for("a", "/"), ViewMode::Details);
        let mut m = ViewMemory::default();
        for i in 0..(MAX_FOLDERS + 5) {
            m.remember("s", &format!("/{i}"), ViewMode::Compact);
        }
        assert_eq!(m.folders.len(), MAX_FOLDERS);
        m.remember("s", "/a", ViewMode::Small);
        let back = ViewMemory::parse(&m.to_json());
        assert_eq!(back.mode_for("s", "/a"), ViewMode::Small);
        assert_eq!(back.default, Some(ViewMode::Small));
    }

    #[test]
    fn kinds_and_viewers() {
        assert_eq!(preview_kind("a.JPG", false), Some("image"));
        assert_eq!(preview_kind("a.cr3", false), Some("raw"));
        assert_eq!(preview_kind("a.jpg", true), None);
        assert_eq!(preview_kind("noext", false), None);
        assert_eq!(viewer_for(&f("a.mkv", None, 1)), Some(Viewer::Video));
        assert_eq!(viewer_for(&f("a.flac", None, 1)), Some(Viewer::Audio));
        assert_eq!(viewer_for(&f("a.pdf", None, 1)), Some(Viewer::Pdf));
        assert_eq!(viewer_for(&f("main.rs", None, 1)), Some(Viewer::Text));
        assert_eq!(viewer_for(&f("data.csv", None, 1)), Some(Viewer::Text));
        assert_eq!(viewer_for(&f("big.csv", None, MAX_TEXT_BYTES + 1)), Some(Viewer::Office));
        assert_eq!(viewer_for(&f("Dockerfile", None, 1)), Some(Viewer::Text));
        assert_eq!(viewer_for(&f("x.bin", Some("application/json"), 1)), Some(Viewer::Text));
        assert_eq!(viewer_for(&f("x.bin", None, 1)), Some(Viewer::Other));
        assert_eq!(file_icon("a.zip", false), "file-archive");
        assert_eq!(file_icon("x", true), "folder");
    }

    #[test]
    fn thumbnails_follow_the_server() {
        let mut caps = PreviewCapabilities { enabled: true, ..Default::default() };
        caps.kinds.insert("image".into(), true);
        assert!(wants_thumbnail(&f("a.png", None, 1), Some(&caps)));
        assert!(!wants_thumbnail(&f("a.mp4", None, 1), Some(&caps)));
        assert!(!wants_thumbnail(&f("a.png", None, 1), None));
        caps.enabled = false;
        assert!(!wants_thumbnail(&f("a.png", None, 1), Some(&caps)));
    }
}
