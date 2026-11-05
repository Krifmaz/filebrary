//! The search page's rules, kept apart from the drawing so they are tested off the browser: the filters and how
//! they read from and write to the address, the `GET /api/search` query they make, and the words a result shows.

use crate::api::query;
use crate::route::param;

/// Results asked for per page ("Show more" asks for the next page).
pub const PAGE: u32 = 50;

/// The type chips: `type=` value and label ("" is everything).
pub const TYPES: [(&str, &str); 9] = [
    ("", "Everything"),
    ("dir", "Folders"),
    ("document", "Documents"),
    ("image", "Images"),
    ("video", "Videos"),
    ("audio", "Audio"),
    ("text", "Text"),
    ("code", "Code"),
    ("archive", "Archives"),
];

/// The "Modified" choices: address value, label and how many days back ("" is any time).
pub const WHEN: [(&str, &str, u64); 5] = [
    ("", "Any time", 0),
    ("day", "Last 24 hours", 1),
    ("week", "Last 7 days", 7),
    ("month", "Last 30 days", 30),
    ("year", "Last year", 365),
];

/// The "Size" choices: address value, label, smallest and largest (server size words, "" for no bound).
pub const SIZES: [(&str, &str, &str, &str); 5] = [
    ("", "Any size", "", ""),
    ("small", "Under 1 MB", "", "1048575"),
    ("medium", "1 MB to 100 MB", "1MiB", "100MiB"),
    ("large", "100 MB to 1 GB", "100MiB", "1GiB"),
    ("huge", "Over 1 GB", "1GiB", ""),
];

/// The "Sort" choices: address value, label, the server's `sort` and `order` ("" leaves the server's default).
pub const SORTS: [(&str, &str, &str, &str); 6] = [
    ("", "Best match", "", ""),
    ("name", "Name", "name", "asc"),
    ("new", "Newest first", "modified", "desc"),
    ("old", "Oldest first", "modified", "asc"),
    ("big", "Largest first", "size", "desc"),
    ("path", "Folder", "path", "asc"),
];

/// The "Sort" choices for a mode (value, label). Names have no relevance, so there the default reads as the
/// name order it is, and the separate "Name" choice is left out.
pub fn sort_choices(mode: Mode) -> Vec<(String, String)> {
    SORTS
        .iter()
        .filter(|s| mode == Mode::Content || s.0 != "name")
        .map(|s| (s.0.to_string(), if s.0.is_empty() && mode == Mode::Names { "Name" } else { s.1 }.to_string()))
        .collect()
}

/// Where the words are looked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// File and folder names (the name index; always there).
    #[default]
    Names,
    /// The text inside files (the content index, when the server has it on).
    Content,
}

/// Everything one search asks for. `q` and `kind` live in the app's route; the rest are the page's own address
/// parameters (`in`, `src`, `when`, `size`, `sort`, `ext`), so a search with filters is a link that can be shared.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Search {
    /// The words.
    pub q: String,
    /// The type chip (`type=`), "" for everything.
    pub kind: String,
    /// Names or text inside files.
    pub mode: Mode,
    /// One source's id, "" for all.
    pub source: String,
    /// A [`WHEN`] value.
    pub when: String,
    /// A [`SIZES`] value.
    pub size: String,
    /// A [`SORTS`] value.
    pub sort: String,
    /// Extensions, comma or space separated (`jpg png`), "" for any.
    pub ext: String,
}

fn known<const N: usize, T>(table: &[T; N], key: impl Fn(&T) -> &str, v: String) -> String {
    if table.iter().any(|t| key(t) == v) {
        v
    } else {
        String::new()
    }
}

impl Search {
    /// The page's own filters from an address query (`?in=content&size=large`); `q` and `kind` come from the route.
    /// Values the page does not know are dropped.
    pub fn from_query(q: &str, kind: &str, query: &str) -> Search {
        Search {
            q: q.to_string(),
            kind: known(&TYPES, |t| t.0, kind.to_string()),
            mode: if param(query, "in") == "content" { Mode::Content } else { Mode::Names },
            source: param(query, "src"),
            when: known(&WHEN, |t| t.0, param(query, "when")),
            size: known(&SIZES, |t| t.0, param(query, "size")),
            sort: known(&SORTS, |t| t.0, param(query, "sort")),
            ext: clean_ext(&param(query, "ext")),
        }
    }

    /// The address query of this search (`?q=…&type=…&in=…`), the route's two first.
    pub fn address_query(&self) -> String {
        query(&[
            ("q", &self.q),
            ("type", &self.kind),
            ("in", if self.mode == Mode::Content { "content" } else { "" }),
            ("src", &self.source),
            ("when", &self.when),
            ("size", &self.size),
            ("sort", &self.sort),
            ("ext", &self.ext),
        ])
    }

    /// Whether there is anything to search for: words or at least one filter.
    pub fn is_empty(&self) -> bool {
        self.q.trim().is_empty()
            && self.kind.is_empty()
            && self.source.is_empty()
            && self.when.is_empty()
            && self.size.is_empty()
            && self.ext.is_empty()
    }

    /// How many filters past the words and the type are set (the "Filters (2)" count, and "Clear filters").
    pub fn filter_count(&self) -> usize {
        [!self.source.is_empty(), !self.when.is_empty(), !self.size.is_empty(), !self.ext.is_empty()].into_iter().filter(|b| *b).count()
    }

    /// The same search with no filters but the words, the mode and the sort.
    pub fn cleared(&self) -> Search {
        Search { q: self.q.clone(), mode: self.mode, sort: self.sort.clone(), ..Search::default() }
    }

    /// The `GET /api/search` path for one page of results; `now` is the time in Unix seconds (for "Modified").
    pub fn api_path(&self, offset: u32, now: u64) -> String {
        let words = self.q.trim();
        let (q, content) = match self.mode {
            Mode::Names => (words, ""),
            Mode::Content => ("", words),
        };
        let days = WHEN.iter().find(|w| w.0 == self.when).map_or(0, |w| w.2);
        let after = if days > 0 { now.saturating_sub(days * 86_400).to_string() } else { String::new() };
        let (_, _, min, max) = SIZES.iter().find(|s| s.0 == self.size).copied().unwrap_or(SIZES[0]);
        let (_, _, mut sort, mut order) = SORTS.iter().find(|s| s.0 == self.sort).copied().unwrap_or(SORTS[0]);
        // "Best match" is relevance for text searches; names have no relevance, so they sort by name.
        if sort.is_empty() && self.mode == Mode::Names {
            (sort, order) = ("name", "asc");
        }
        let (limit, offset) = (PAGE.to_string(), offset.to_string());
        format!(
            "/search{}",
            query(&[
                ("q", q),
                ("content", content),
                ("type", &self.kind),
                ("source", &self.source),
                ("ext", &self.ext.replace(' ', ",")),
                ("min_size", min),
                ("max_size", max),
                ("after", &after),
                ("sort", sort),
                ("order", order),
                ("limit", &limit),
                ("offset", if offset == "0" { "" } else { &offset }),
            ])
        )
    }

    /// The page's heading.
    pub fn title(&self) -> String {
        match (self.q.trim(), self.mode) {
            ("", _) => "Search".into(),
            (q, Mode::Names) => format!("Results for “{q}”"),
            (q, Mode::Content) => format!("Files containing “{q}”"),
        }
    }
}

/// Extensions as the server takes them: lower case, no dots, separated by single spaces (`.JPG, png` → `jpg png`).
pub fn clean_ext(s: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for e in s.split([',', ' ', ';']).map(|e| e.trim().trim_start_matches('.').to_lowercase()).filter(|e| !e.is_empty()) {
        if !out.contains(&e) {
            out.push(e);
        }
    }
    out.truncate(16);
    out.join(" ")
}

/// The line under the heading: how many were found, and whether a source is still being indexed.
pub fn count_line(shown: usize, more: bool, indexing: bool) -> String {
    let n = match (shown, more) {
        (0, _) => "Nothing found".to_string(),
        (1, false) => "1 result".to_string(),
        (n, false) => format!("{n} results"),
        (n, true) => format!("{n}+ results"),
    };
    if indexing {
        format!("{n} · still indexing, so some files may be missing")
    } else {
        n
    }
}

/// A size as people read it (`0 B`, `12 KB`, `1.5 MB`), in powers of 1024 like the server's size filter.
pub fn size_words(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v < 10.0 {
        format!("{:.1} {}", v, UNITS[u]).replace(".0 ", " ")
    } else {
        format!("{:.0} {}", v, UNITS[u])
    }
}

/// A change date as people read it: the time for today (`14:03`), else the day (`5 Nov 2026`). `today` is
/// `YYYY-MM-DD`; `rfc3339` is the server's time (UTC). "" when there is none.
pub fn date_words(rfc3339: &str, today: &str) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (Some(day), Some(time)) = (rfc3339.get(..10), rfc3339.get(11..16)) else { return String::new() };
    if day == today {
        return time.to_string();
    }
    let mut it = day.split('-');
    match (it.next(), it.next().and_then(|m| m.parse::<usize>().ok()), it.next().and_then(|d| d.parse::<u32>().ok())) {
        (Some(y), Some(m @ 1..=12), Some(d)) => format!("{d} {} {y}", MONTHS[m - 1]),
        _ => String::new(),
    }
}

/// The folder a path is in (`/a/b.txt` → `/a`, `/b.txt` → `/`).
pub fn parent(path: &str) -> String {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) | None => "/".into(),
        Some((dir, _)) => dir.to_string(),
    }
}

/// A content excerpt made safe to draw as HTML. The server sends it escaped, with matches in `<b>…</b>`; this lets
/// through only `<b>`, `</b>` and entities (`&lt;`, `&#39;`) and escapes every other `<`, `>`, `"` and `&`, so a
/// server that forgot to escape cannot inject markup.
pub fn safe_snippet(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if let Some(tag) = ["<b>", "</b>"].into_iter().find(|t| rest.starts_with(t)) {
            out.push_str(tag);
            rest = &rest[tag.len()..];
            continue;
        }
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '&' if is_entity(rest) => out.push('&'),
            '&' => out.push_str("&amp;"),
            c => out.push(c),
        }
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Whether `s` starts with an HTML entity (`&amp;`, `&#39;`, `&#x27;`).
fn is_entity(s: &str) -> bool {
    let Some(end) = s.get(..12).unwrap_or(s).find(';') else { return false };
    let body = &s[1..end];
    match body.strip_prefix('#') {
        Some(n) => n
            .strip_prefix(['x', 'X'])
            .map_or(!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()), |h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit())),
        None => !body.is_empty() && body.bytes().all(|b| b.is_ascii_alphanumeric()),
    }
}

/// A name as HTML with the searched words marked (`<mark>`), case-insensitive; everything else escaped.
pub fn marked(name: &str, words: &str) -> String {
    let lower = name.to_lowercase();
    // byte ranges to mark; only when lower-casing keeps the byte offsets (ASCII-safe names and most others)
    let mut spans: Vec<(usize, usize)> = Vec::new();
    if lower.len() == name.len() {
        for w in words.split_whitespace().map(str::to_lowercase).filter(|w| !w.is_empty()) {
            let mut from = 0;
            while let Some(i) = lower[from..].find(&w) {
                spans.push((from + i, from + i + w.len()));
                from += i + w.len();
            }
        }
    }
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (a, b) in spans {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    let mut out = String::new();
    let mut at = 0;
    for (a, b) in merged {
        if !name.is_char_boundary(a) || !name.is_char_boundary(b) {
            continue;
        }
        out.push_str(&kit_leptos::html::esc(&name[at..a]));
        out.push_str("<mark>");
        out.push_str(&kit_leptos::html::esc(&name[a..b]));
        out.push_str("</mark>");
        at = b;
    }
    out.push_str(&kit_leptos::html::esc(&name[at..]));
    out
}

/// The icon name for a result's coarse type.
pub fn kind_icon(kind: &str, is_dir: bool) -> &'static str {
    if is_dir {
        return "folder";
    }
    match kind {
        "image" => "image",
        "video" => "video",
        "audio" => "audio",
        "document" | "text" => "document",
        "code" => "code",
        "archive" => "archive",
        _ => "file",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(q: &str) -> Search {
        Search { q: q.into(), ..Search::default() }
    }

    #[test]
    fn filters_round_trip_through_the_address() {
        let full = Search {
            q: "tax 2025".into(),
            kind: "document".into(),
            mode: Mode::Content,
            source: "my s3".into(),
            when: "month".into(),
            size: "large".into(),
            sort: "new".into(),
            ext: "pdf docx".into(),
        };
        let q = full.address_query();
        let back = Search::from_query(&crate::route::param(&q, "q"), &crate::route::param(&q, "type"), &q);
        assert_eq!(back, full);
        assert_eq!(s("").address_query(), "");
    }

    #[test]
    fn unknown_address_values_are_dropped() {
        let got = Search::from_query("x", "bogus", "?in=nope&when=decade&size=xl&sort=random&ext=.JPG,,png,jpg");
        assert_eq!(got, Search { q: "x".into(), ext: "jpg png".into(), ..Search::default() });
    }

    #[test]
    fn names_search_sorts_by_name_and_pages() {
        assert_eq!(s("a b").api_path(0, 0), "/search?q=a%20b&sort=name&order=asc&limit=50");
        assert_eq!(s("a").api_path(100, 0), "/search?q=a&sort=name&order=asc&limit=50&offset=100");
    }

    #[test]
    fn content_search_keeps_relevance() {
        let c = Search { q: " invoice ".into(), mode: Mode::Content, ..Search::default() };
        assert_eq!(c.api_path(0, 0), "/search?content=invoice&limit=50");
        let sorted = Search { sort: "big".into(), ..c };
        assert_eq!(sorted.api_path(0, 0), "/search?content=invoice&sort=size&order=desc&limit=50");
    }

    #[test]
    fn filters_become_server_parameters() {
        let f = Search {
            q: "x".into(),
            kind: "image".into(),
            source: "home".into(),
            when: "week".into(),
            size: "medium".into(),
            ext: "jpg png".into(),
            ..Search::default()
        };
        let p = f.api_path(0, 1_000_000);
        assert!(p.contains("type=image&source=home&ext=jpg%2Cpng&min_size=1MiB&max_size=100MiB&after=395200&"), "{p}");
        let small = Search { size: "small".into(), ..s("") };
        assert!(small.api_path(0, 0).contains("max_size=1048575") && !small.api_path(0, 0).contains("min_size"));
    }

    #[test]
    fn empty_and_counts() {
        assert!(s("  ").is_empty());
        assert!(!Search { kind: "image".into(), ..s("") }.is_empty());
        let f = Search { when: "day".into(), ext: "md".into(), sort: "new".into(), ..s("q") };
        assert_eq!(f.filter_count(), 2);
        assert_eq!(f.cleared(), Search { sort: "new".into(), ..s("q") });
    }

    #[test]
    fn sort_choices_follow_the_mode() {
        let names = sort_choices(Mode::Names);
        assert_eq!(names[0], (String::new(), "Name".to_string()));
        assert!(!names.iter().any(|c| c.0 == "name"));
        assert_eq!(sort_choices(Mode::Content).len(), SORTS.len());
    }

    #[test]
    fn titles() {
        assert_eq!(s("").title(), "Search");
        assert_eq!(s("cat").title(), "Results for “cat”");
        assert_eq!(Search { mode: Mode::Content, ..s("cat") }.title(), "Files containing “cat”");
    }

    #[test]
    fn words_for_counts_sizes_and_dates() {
        assert_eq!(count_line(0, false, false), "Nothing found");
        assert_eq!(count_line(1, false, false), "1 result");
        assert_eq!(count_line(50, true, true), "50+ results · still indexing, so some files may be missing");
        assert_eq!(size_words(0), "0 B");
        assert_eq!(size_words(1536), "1.5 KB");
        assert_eq!(size_words(1024 * 1024), "1 MB");
        assert_eq!(size_words(250 * 1024 * 1024), "250 MB");
        assert_eq!(date_words("2026-11-05T14:03:00Z", "2026-11-05"), "14:03");
        assert_eq!(date_words("2026-01-09T14:03:00Z", "2026-11-05"), "9 Jan 2026");
        assert_eq!(date_words("", "2026-11-05"), "");
        assert_eq!(date_words("garbage-and-more", "x"), "");
    }

    #[test]
    fn names_mark_the_words() {
        assert_eq!(marked("Tax Return 2025.pdf", "tax 2025"), "<mark>Tax</mark> Return <mark>2025</mark>.pdf");
        assert_eq!(marked("aaa", "aa a"), "<mark>aaa</mark>");
        assert_eq!(marked("<x>.txt", "x"), "&lt;<mark>x</mark>&gt;.t<mark>x</mark>t");
        assert_eq!(marked("ünï.txt", "nï"), "ü<mark>nï</mark>.txt");
        assert_eq!(marked("plain", ""), "plain");
    }

    #[test]
    fn parents() {
        assert_eq!(parent("/a/b.txt"), "/a");
        assert_eq!(parent("/b.txt"), "/");
        assert_eq!(parent("/a/b/"), "/a");
        assert_eq!(parent("/"), "/");
    }

    #[test]
    fn snippets_keep_only_bold() {
        assert_eq!(safe_snippet("a <b>tax</b> form"), "a <b>tax</b> form");
        assert_eq!(safe_snippet("1 &lt; 2 &amp; <b>x</b>"), "1 &lt; 2 &amp; <b>x</b>");
        assert_eq!(safe_snippet("<img src=x onerror=1>"), "&lt;img src=x onerror=1&gt;");
        assert_eq!(safe_snippet("<b onclick=1>x</b>"), "&lt;b onclick=1&gt;x</b>");
        assert_eq!(safe_snippet("AT&T & co &#39;q&#x27; &;"), "AT&amp;T &amp; co &#39;q&#x27; &amp;;");
    }
}
