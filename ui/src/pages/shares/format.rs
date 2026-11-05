//! Words and numbers for the sharing pages: what a link points at, its state, sizes, dates, expiry choices and
//! paths inside a share. Pure, so it is tested natively; the browser's clock and locale come in only through
//! [`now_ms`], [`date`] and the `datetime-local` conversions.

use super::api::Share;

/// A link's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Works.
    Active,
    /// Past its expiry.
    Expired,
    /// Out of downloads.
    UsedUp,
}

impl State {
    /// The badge words, "" when it works.
    pub fn label(self) -> &'static str {
        match self {
            State::Active => "",
            State::Expired => "Expired",
            State::UsedUp => "Limit reached",
        }
    }
}

/// The state of `s`.
pub fn state(s: &Share) -> State {
    if s.max_downloads.is_some_and(|m| s.downloads >= m) {
        State::UsedUp
    } else if s.expired {
        State::Expired
    } else {
        State::Active
    }
}

/// The last part of a path (`/a/b.txt` → `b.txt`), "" for the root.
pub fn base_name(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// What a link points at, in words.
pub fn target_words(s: &Share) -> String {
    if s.collection_id.is_some() {
        return "A collection".into();
    }
    match s.paths.as_slice() {
        [] => "Nothing".into(),
        [p] if base_name(p).is_empty() => format!("All of {}", s.source),
        [p] => base_name(p).to_string(),
        many => format!("{} items", many.len()),
    }
}

/// "1 download", "3 of 10 downloads", "No downloads".
pub fn downloads_words(downloads: u64, max: Option<u64>) -> String {
    match max {
        Some(m) => format!("{downloads} of {m} downloads"),
        None if downloads == 0 => "No downloads".into(),
        None if downloads == 1 => "1 download".into(),
        None => format!("{downloads} downloads"),
    }
}

/// Whether `s` matches the words of a filter (its target, source or paths), case aside.
pub fn matches(s: &Share, words: &str) -> bool {
    let w = words.trim().to_lowercase();
    w.is_empty()
        || target_words(s).to_lowercase().contains(&w)
        || s.source.to_lowercase().contains(&w)
        || s.paths.iter().any(|p| p.to_lowercase().contains(&w))
}

/// A size in bytes for people (`0 B`, `1.5 KB`, `12 MB`).
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else if v < 10.0 {
        format!("{v:.1} {}", UNITS[u])
    } else {
        format!("{v:.0} {}", UNITS[u])
    }
}

/// The share-relative form of a path (`/a/b` → `a/b`, `/` → "").
pub fn rel(path: &str) -> String {
    path.split('/').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("/")
}

/// The folders above and including `path` inside a share, as (name, path) with `/`-led paths, the top first
/// (named `top`).
pub fn crumbs(top: &str, path: &str) -> Vec<(String, String)> {
    let mut out = vec![(top.to_string(), "/".to_string())];
    let mut acc = String::new();
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        acc.push('/');
        acc.push_str(seg);
        out.push((seg.to_string(), acc.clone()));
    }
    out
}

/// How a visitor's list is sorted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    /// By name, folders first.
    Name,
    /// Largest first.
    Size,
    /// Newest first.
    Modified,
}

impl Sort {
    /// From a `<select>` value.
    pub fn parse(s: &str) -> Sort {
        match s {
            "size" => Sort::Size,
            "modified" => Sort::Modified,
            _ => Sort::Name,
        }
    }
}

/// Sorts a shared folder's entries: folders first, then by `by`.
pub fn sort_entries(list: &mut [super::api::PublicEntry], by: Sort) {
    list.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then_with(|| match by {
            Sort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            Sort::Size => b.size.cmp(&a.size),
            Sort::Modified => b.modified.cmp(&a.modified),
        })
    });
}

/// What the browser can show of a file, by its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A picture.
    Image,
    /// A video.
    Video,
    /// Sound.
    Audio,
    /// A PDF.
    Pdf,
    /// Plain text.
    Text,
    /// Nothing to show (download only).
    Other,
}

/// The kind of file `name` is, by its extension.
pub fn kind(name: &str) -> Kind {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "svg" | "bmp" => Kind::Image,
        "mp4" | "webm" | "mov" | "m4v" | "ogv" => Kind::Video,
        "mp3" | "ogg" | "oga" | "wav" | "flac" | "m4a" | "opus" | "aac" => Kind::Audio,
        "pdf" => Kind::Pdf,
        "txt" | "md" | "log" | "csv" | "json" | "yaml" | "yml" | "toml" | "xml" | "ini" | "conf" | "rs" | "py" | "js" | "ts" | "sh"
        | "go" | "c" | "h" | "cpp" | "java" | "kt" | "css" => Kind::Text,
        _ => Kind::Other,
    }
}

/// The icon of an entry (`folder`, or one by kind).
pub fn icon_name(name: &str, is_dir: bool) -> &'static str {
    if is_dir {
        return "folder";
    }
    match kind(name) {
        Kind::Image => "image",
        Kind::Video => "video",
        Kind::Audio => "audio",
        _ => "file",
    }
}

/// The expiry choices of the share dialog.
pub const EXPIRY_CHOICES: [(&str, &str); 6] =
    [("keep", "Keep as it is"), ("never", "Never"), ("1", "In 1 day"), ("7", "In 7 days"), ("30", "In 30 days"), ("custom", "On a date…")];

/// A day in milliseconds.
const DAY_MS: f64 = 86_400_000.0;

/// RFC 3339 (UTC) of `ms` since 1970.
pub fn iso_of_ms(ms: f64) -> String {
    let secs = (ms / 1000.0).floor() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Days since 1970-01-01 → (year, month, day) (Howard Hinnant's civil-from-days).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// The expiry a choice asks for, at `now_ms`: `None` is no change, `Some(None)` no expiry, `Some(Some(t))` the
/// time `t`. `custom` is the value of the date box, already RFC 3339 ("" when it is empty).
pub fn expiry_of(choice: &str, custom: &str, now_ms: f64) -> Result<Option<Option<String>>, &'static str> {
    match choice {
        "keep" => Ok(None),
        "never" => Ok(Some(None)),
        "custom" if custom.is_empty() => Err("Pick the date the link stops working."),
        "custom" => Ok(Some(Some(custom.to_string()))),
        days => {
            let n: f64 = days.parse().map_err(|_| "Pick when the link stops working.")?;
            Ok(Some(Some(iso_of_ms(now_ms + n * DAY_MS))))
        }
    }
}

/// The download limit in a box: "" is unlimited.
pub fn limit_of(text: &str) -> Result<Option<u64>, &'static str> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    match t.parse::<u64>() {
        Ok(n) if n >= 1 => Ok(Some(n)),
        _ => Err("The download limit is a whole number, 1 or more."),
    }
}

/// The full address of a link to give out: this site, the app's base, `/s/<token>`.
pub fn link_of(origin: &str, base: &str, token: &str) -> String {
    format!("{}{}/s/{}", origin.trim_end_matches('/'), base, crate::api::encode(token))
}

/// The address of link `token` from this browser.
pub fn link(token: &str) -> String {
    let origin = kit_leptos::rt::window().and_then(|w| w.location().origin().ok()).unwrap_or_default();
    link_of(&origin, &crate::route::base(), token)
}

/// Now, in milliseconds since 1970.
pub fn now_ms() -> f64 {
    js_sys::Date::now()
}

/// An RFC 3339 time as people read it here ("5 Nov 2026, 14:30").
pub fn date(iso: &str) -> String {
    let d = js_sys::Date::new(&iso.into());
    if d.get_time().is_nan() {
        return iso.to_string();
    }
    let opts = js_sys::Object::new();
    for (k, v) in [("day", "numeric"), ("month", "short"), ("year", "numeric"), ("hour", "2-digit"), ("minute", "2-digit")] {
        let _ = js_sys::Reflect::set(&opts, &k.into(), &v.into());
    }
    d.to_locale_string("default", &opts).into()
}

/// The value of a `datetime-local` box as RFC 3339 ("" when it is empty or not a time).
pub fn local_to_iso(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let d = js_sys::Date::new(&value.into());
    if d.get_time().is_nan() {
        return String::new();
    }
    d.to_iso_string().into()
}

/// An RFC 3339 time as a `datetime-local` value, in this browser's time zone.
pub fn iso_to_local(iso: &str) -> String {
    let d = js_sys::Date::new(&iso.into());
    if d.get_time().is_nan() {
        return String::new();
    }
    format!("{:04}-{:02}-{:02}T{:02}:{:02}", d.get_full_year(), d.get_month() + 1, d.get_date(), d.get_hours(), d.get_minutes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::shares::api::PublicEntry;

    fn share(paths: &[&str]) -> Share {
        serde_json::from_value(serde_json::json!({"id": 1, "token": "t", "source": "home", "paths": paths})).unwrap()
    }

    #[test]
    fn a_link_says_what_it_shares() {
        assert_eq!(target_words(&share(&["/Photos/2026"])), "2026");
        assert_eq!(target_words(&share(&["/"])), "All of home");
        assert_eq!(target_words(&share(&["/a", "/b", "/c"])), "3 items");
        let mut c = share(&[]);
        c.collection_id = Some(3);
        assert_eq!(target_words(&c), "A collection");
    }

    #[test]
    fn states_and_downloads() {
        let mut s = share(&["/a"]);
        assert_eq!(state(&s), State::Active);
        s.expired = true;
        assert_eq!(state(&s), State::Expired);
        s.max_downloads = Some(2);
        s.downloads = 2;
        assert_eq!(state(&s), State::UsedUp);
        assert_eq!(downloads_words(0, None), "No downloads");
        assert_eq!(downloads_words(1, None), "1 download");
        assert_eq!(downloads_words(3, Some(10)), "3 of 10 downloads");
    }

    #[test]
    fn filters_match_names_and_sources() {
        let s = share(&["/Photos/Trip"]);
        assert!(matches(&s, ""));
        assert!(matches(&s, "trip"));
        assert!(matches(&s, " HOME "));
        assert!(!matches(&s, "tax"));
    }

    #[test]
    fn sizes() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(1023), "1023 B");
        assert_eq!(size(1536), "1.5 KB");
        assert_eq!(size(50 * 1024 * 1024), "50 MB");
    }

    #[test]
    fn paths_inside_a_share() {
        assert_eq!(rel("/a/b/"), "a/b");
        assert_eq!(rel("/"), "");
        assert_eq!(base_name("/a/b.txt"), "b.txt");
        assert_eq!(base_name("/"), "");
        let c = crumbs("Photos", "/2026/May");
        assert_eq!(c, vec![("Photos".into(), "/".into()), ("2026".into(), "/2026".into()), ("May".into(), "/2026/May".into())]);
        assert_eq!(crumbs("X", "/").len(), 1);
    }

    #[test]
    fn folders_come_first() {
        let e = |name: &str, dir: bool, size: u64, m: Option<&str>| PublicEntry {
            name: name.into(),
            path: name.into(),
            is_dir: dir,
            size,
            modified: m.map(String::from),
        };
        let mut l = vec![e("b.txt", false, 9, Some("2026-01-02")), e("Z", true, 0, None), e("a.txt", false, 1, Some("2026-03-01"))];
        sort_entries(&mut l, Sort::Name);
        assert_eq!(l.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["Z", "a.txt", "b.txt"]);
        sort_entries(&mut l, Sort::Size);
        assert_eq!(l[1].name, "b.txt");
        sort_entries(&mut l, Sort::Modified);
        assert_eq!(l[1].name, "a.txt");
    }

    #[test]
    fn kinds_by_extension() {
        assert_eq!(kind("IMG.JPG"), Kind::Image);
        assert_eq!(kind("a.pdf"), Kind::Pdf);
        assert_eq!(kind("notes.md"), Kind::Text);
        assert_eq!(kind("archive.zip"), Kind::Other);
        assert_eq!(kind("README"), Kind::Other);
        assert_eq!(icon_name("x", true), "folder");
    }

    #[test]
    fn expiry_choices() {
        assert_eq!(iso_of_ms(0.0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_of_ms(1_793_750_400_000.0), "2026-11-04T00:00:00Z");
        assert_eq!(iso_of_ms(951_782_400_000.0), "2000-02-29T00:00:00Z");
        assert_eq!(expiry_of("keep", "", 0.0), Ok(None));
        assert_eq!(expiry_of("never", "", 0.0), Ok(Some(None)));
        assert_eq!(expiry_of("7", "", 0.0), Ok(Some(Some("1970-01-08T00:00:00Z".into()))));
        assert!(expiry_of("custom", "", 0.0).is_err());
        assert_eq!(expiry_of("custom", "2027-01-01T00:00:00Z", 0.0), Ok(Some(Some("2027-01-01T00:00:00Z".into()))));
    }

    #[test]
    fn limits() {
        assert_eq!(limit_of(""), Ok(None));
        assert_eq!(limit_of(" 5 "), Ok(Some(5)));
        assert!(limit_of("0").is_err());
        assert!(limit_of("x").is_err());
    }

    #[test]
    fn links_carry_the_base() {
        assert_eq!(link_of("https://files.example/", "/next", "ab c"), "https://files.example/next/s/ab%20c");
        assert_eq!(link_of("http://h:8080", "", "t"), "http://h:8080/s/t");
    }
}
