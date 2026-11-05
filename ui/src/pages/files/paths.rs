//! Paths, names, sizes, dates, sorting and selection: the pure rules the browser follows, tested natively.
//!
//! Paths inside a source are absolute and `/`-separated. These helpers only shape paths for display and requests;
//! the server does the real normalising and safety checks.

use crate::api::Entry;
use std::collections::BTreeSet;

/// Joins path parts into one absolute path, dropping empty and `.` segments (`join("/a", "b/c")` → `/a/b/c`).
pub fn join(a: &str, b: &str) -> String {
    let segs: Vec<&str> = a.split('/').chain(b.split('/')).filter(|s| !s.is_empty() && *s != ".").collect();
    format!("/{}", segs.join("/"))
}

/// The parent folder of `path` (`/` stays `/`).
pub fn parent(path: &str) -> String {
    let mut segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    segs.pop();
    format!("/{}", segs.join("/"))
}

/// The last segment of `path` ("" for `/`).
pub fn basename(path: &str) -> &str {
    path.split('/').rfind(|s| !s.is_empty()).unwrap_or("")
}

/// The breadcrumbs of `path`: `(name, path)` from the root (`/`, then each folder).
pub fn crumbs(path: &str) -> Vec<(String, String)> {
    let mut out = vec![("/".to_string(), "/".to_string())];
    let mut acc = String::new();
    for s in path.split('/').filter(|s| !s.is_empty()) {
        acc.push('/');
        acc.push_str(s);
        out.push((s.to_string(), acc.clone()));
    }
    out
}

/// Why a file or folder name can't be used, or `None` when it is fine.
pub fn name_problem(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        return Some("Type a name.");
    }
    if name == "." || name == ".." {
        return Some("A name can't be . or ..");
    }
    if name.contains('/') || name.contains('\\') {
        return Some("A name can't contain slashes.");
    }
    if name.chars().any(|c| c.is_control()) {
        return Some("A name can't contain control characters.");
    }
    if name.len() > 255 {
        return Some("That name is too long.");
    }
    None
}

/// The lower-case extension of a file name, without the dot ("" when there is none; `.bashrc` has none).
pub fn extension(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => name[i + 1..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// Where the name ends and its extension begins, for selecting the name part when renaming.
pub fn stem_len(name: &str) -> usize {
    match name.rfind('.') {
        Some(i) if i > 0 => name[..i].encode_utf16().count(),
        _ => name.encode_utf16().count(),
    }
}

/// A byte count for people (`1.5 MB`, decimal units like the React UI).
pub fn format_size(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let units = ["kB", "MB", "GB", "TB", "PB"];
    let mut v = bytes as f64;
    let mut i = 0;
    v /= 1000.0;
    while v >= 1000.0 && i < units.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if v < 10.0 {
        format!("{v:.1} {}", units[i])
    } else {
        format!("{} {}", v.round() as u64, units[i])
    }
}

/// An RFC 3339 time as a short local date and time ("" when it can't be read).
pub fn format_date(iso: &str) -> String {
    if iso.is_empty() {
        return String::new();
    }
    #[cfg(target_arch = "wasm32")]
    {
        let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(iso));
        if d.get_time().is_nan() {
            return String::new();
        }
        let opts = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&opts, &"dateStyle".into(), &"medium".into());
        let _ = js_sys::Reflect::set(&opts, &"timeStyle".into(), &"short".into());
        d.to_locale_string("default", &opts).as_string().unwrap_or_default()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        iso.get(..16).map(|s| s.replace('T', " ")).unwrap_or_default()
    }
}

/// Unix seconds as a short local date and time.
pub fn format_unix(secs: i64) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(secs as f64 * 1000.0));
        d.to_iso_string().as_string().map(|s| format_date(&s)).unwrap_or_default()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        secs.to_string()
    }
}

/// `n` and a word, with an `s` when `n` is not 1 ("1 file", "3 files").
pub fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// What the listing can be sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    /// By name (numbers in names compare as numbers).
    Name,
    /// By size.
    Size,
    /// By last change.
    Modified,
}

impl SortKey {
    /// The key's word in the sort menu and its `<option>` value.
    pub fn id(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Size => "size",
            SortKey::Modified => "modified",
        }
    }

    /// The key of an `<option>` value (name for anything else).
    pub fn parse(s: &str) -> SortKey {
        match s {
            "size" => SortKey::Size,
            "modified" => SortKey::Modified,
            _ => SortKey::Name,
        }
    }
}

/// Compares names the way people expect: case-insensitive, with runs of digits compared as numbers
/// (`file2` before `file10`).
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (n, m) = (take(&mut x), take(&mut y));
                let (nt, mt) = (n.trim_start_matches('0'), m.trim_start_matches('0'));
                let o = nt.len().cmp(&mt.len()).then_with(|| nt.cmp(mt));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// Sorts entries with folders first, then by `key` (ties by name), `asc` or descending.
pub fn sort_entries(entries: &mut [Entry], key: SortKey, asc: bool) {
    entries.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            return b.is_dir.cmp(&a.is_dir);
        }
        let o = match key {
            SortKey::Name => std::cmp::Ordering::Equal,
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Modified => a.modified.cmp(&b.modified),
        }
        .then_with(|| natural_cmp(&a.name, &b.name));
        if asc {
            o
        } else {
            o.reverse()
        }
    });
}

/// The entries whose name holds `filter` (any case); all of them for an empty filter.
pub fn filter_entries(entries: &[Entry], filter: &str) -> Vec<Entry> {
    let f = filter.trim().to_lowercase();
    entries.iter().filter(|e| f.is_empty() || e.name.to_lowercase().contains(&f)).cloned().collect()
}

/// How a click changes the selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    /// Shift: select the range from the anchor.
    pub shift: bool,
    /// Ctrl/Cmd (or a tap while selecting): toggle one item.
    pub toggle: bool,
}

/// The selection and anchor after a click on `target`: a plain click selects only it, toggle flips it, shift
/// selects the range from `anchor` (added to the selection when toggle is held too).
pub fn next_selection(
    order: &[String],
    current: &BTreeSet<String>,
    anchor: Option<&str>,
    target: &str,
    mods: Mods,
) -> (BTreeSet<String>, Option<String>) {
    if mods.shift {
        if let (Some(a), Some(b)) = (anchor.and_then(|a| order.iter().position(|p| p == a)), order.iter().position(|p| p == target)) {
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            let mut next = if mods.toggle { current.clone() } else { BTreeSet::new() };
            next.extend(order[lo..=hi].iter().cloned());
            return (next, anchor.map(str::to_string));
        }
    }
    if mods.toggle {
        let mut next = current.clone();
        if !next.remove(target) {
            next.insert(target.to_string());
        }
        return (next, Some(target.to_string()));
    }
    (BTreeSet::from([target.to_string()]), Some(target.to_string()))
}

/// The index the keyboard moves to: `delta` items from `at` (the first or last when nothing is selected),
/// kept inside `0..len`.
pub fn step_index(at: Option<usize>, delta: i64, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match at {
        None if delta > 0 => 0,
        None => len - 1,
        Some(i) => (i as i64 + delta).clamp(0, len as i64 - 1) as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, dir: bool, size: u64, modified: &str) -> Entry {
        Entry { name: name.into(), path: format!("/{name}"), is_dir: dir, size, modified: modified.into(), mime: None }
    }

    #[test]
    fn paths() {
        assert_eq!(join("/", "a"), "/a");
        assert_eq!(join("/a/", "b/./c"), "/a/b/c");
        assert_eq!(join("/", ""), "/");
        assert_eq!(parent("/a/b"), "/a");
        assert_eq!(parent("/a"), "/");
        assert_eq!(parent("/"), "/");
        assert_eq!(basename("/a/b.txt"), "b.txt");
        assert_eq!(basename("/"), "");
        assert_eq!(crumbs("/a/b"), vec![("/".into(), "/".into()), ("a".into(), "/a".into()), ("b".into(), "/a/b".into())]);
        assert_eq!(extension("Photo.JPG"), "jpg");
        assert_eq!(extension(".bashrc"), "");
        assert_eq!(extension("a.tar.gz"), "gz");
        assert_eq!(stem_len("notes.txt"), 5);
        assert_eq!(stem_len("README"), 6);
    }

    #[test]
    fn names() {
        assert!(name_problem("ok.txt").is_none());
        assert!(name_problem(" ").is_some());
        assert!(name_problem("..").is_some());
        assert!(name_problem("a/b").is_some());
        assert!(name_problem("a\u{7}").is_some());
        assert!(name_problem(&"x".repeat(256)).is_some());
    }

    #[test]
    fn sizes_and_words() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1500), "1.5 kB");
        assert_eq!(format_size(25_000_000), "25 MB");
        assert_eq!(format_size(3_000_000_000_000), "3.0 TB");
        assert_eq!(plural(1, "file"), "1 file");
        assert_eq!(plural(2, "file"), "2 files");
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["file10", "File2", "file1", "a"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["a", "file1", "File2", "file10"]);
    }

    #[test]
    fn sorting_keeps_folders_first() {
        let mut v = vec![e("b.txt", false, 5, "2026-01-02"), e("z", true, 0, "2026-01-01"), e("a.txt", false, 9, "2026-01-03")];
        sort_entries(&mut v, SortKey::Name, true);
        assert_eq!(v.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["z", "a.txt", "b.txt"]);
        // folders stay first even when sorting down
        sort_entries(&mut v, SortKey::Size, false);
        assert_eq!(v.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["z", "a.txt", "b.txt"]);
        sort_entries(&mut v, SortKey::Modified, true);
        assert_eq!(v.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["z", "b.txt", "a.txt"]);
        assert_eq!(filter_entries(&v, "TXT").len(), 2);
        assert_eq!(filter_entries(&v, "").len(), 3);
        assert_eq!(SortKey::parse(SortKey::Modified.id()), SortKey::Modified);
    }

    #[test]
    fn selection_follows_desktop_rules() {
        let order: Vec<String> = ["/a", "/b", "/c", "/d"].iter().map(|s| s.to_string()).collect();
        let none = BTreeSet::new();
        let (s, a) = next_selection(&order, &none, None, "/b", Mods::default());
        assert_eq!(s, BTreeSet::from(["/b".to_string()]));
        let (s, a) = next_selection(&order, &s, a.as_deref(), "/d", Mods { shift: true, toggle: false });
        assert_eq!(s.len(), 3);
        assert_eq!(a.as_deref(), Some("/b"));
        let (s, _) = next_selection(&order, &s, a.as_deref(), "/c", Mods { shift: false, toggle: true });
        assert_eq!(s, BTreeSet::from(["/b".to_string(), "/d".to_string()]));
        let (s, _) = next_selection(&order, &s, Some("/a"), "/a", Mods { shift: true, toggle: true });
        assert!(s.contains("/a") && s.contains("/d"));
    }

    #[test]
    fn keyboard_steps_stay_inside() {
        assert_eq!(step_index(None, 1, 3), Some(0));
        assert_eq!(step_index(None, -1, 3), Some(2));
        assert_eq!(step_index(Some(2), 4, 3), Some(2));
        assert_eq!(step_index(Some(1), -4, 3), Some(0));
        assert_eq!(step_index(Some(0), 1, 0), None);
    }
}
