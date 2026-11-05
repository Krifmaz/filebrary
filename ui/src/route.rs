//! The app's addresses: real paths (`/files/home/Photos`), not `#` routes, so links, share addresses and the
//! browser's Back button work the way people expect.
//!
//! The app may live under a base path (`/` by default). The base comes from the address of the page's own script
//! (`/sub/filebrary-web-<hash>.js` → `/sub`), so a server that mounts it elsewhere needs no change here.

use crate::api::{decode, encode};
use leptos::prelude::*;

/// A page of the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// `/`: the overview.
    Home,
    /// `/login?next=…`: sign in, then go to `next`.
    Login {
        /// Where to go after signing in.
        next: String,
    },
    /// `/setup`: first-run setup.
    Setup,
    /// `/files/<source>/<path…>`: a folder (or file) in a source.
    Files {
        /// The source's id.
        source: String,
        /// The path inside it, starting with `/`.
        path: String,
    },
    /// `/starred`.
    Starred,
    /// `/shares`: the account's share links.
    Shares,
    /// `/search?q=…&type=…`.
    Search {
        /// The words.
        q: String,
        /// The kind filter (`image`, `document`, …), "" for all.
        kind: String,
    },
    /// `/collections`.
    Collections,
    /// `/settings/<tab>`: `account`, `security`, `preferences`, `appearance`.
    Settings {
        /// The tab.
        tab: String,
    },
    /// `/whats-new`: the changelog (the kit's).
    WhatsNew,
    /// `/bug-reports`: the account's bug reports (the kit's).
    BugReports,
    /// `/admin/<page>`: `users` and the other admin pages.
    Admin {
        /// The page.
        page: String,
    },
    /// `/s/<token>/<path…>`: a public share link (no sign-in).
    Public {
        /// The link's token.
        token: String,
        /// The path inside a folder share.
        path: String,
    },
    /// Anything else.
    NotFound,
}

/// The settings tabs, in order.
pub const SETTINGS_TABS: [&str; 4] = ["account", "security", "preferences", "appearance"];

impl Route {
    /// The route of an app path (without the base) and query (with or without `?`).
    pub fn parse(path: &str, query: &str) -> Route {
        let segs: Vec<String> = path.split('/').filter(|s| !s.is_empty()).map(decode).collect();
        let param = |k: &str| param(query, k);
        let rest = |from: usize| format!("/{}", segs.get(from..).unwrap_or(&[]).join("/"));
        let s: Vec<&str> = segs.iter().map(String::as_str).collect();
        match s.as_slice() {
            [] => Route::Home,
            ["login"] => Route::Login {
                next: Some(param("next")).filter(|n| n.starts_with('/') && !n.starts_with("//")).unwrap_or_else(|| "/".into()),
            },
            ["setup"] => Route::Setup,
            ["starred"] => Route::Starred,
            ["shares"] => Route::Shares,
            ["collections"] => Route::Collections,
            ["search"] => Route::Search { q: param("q"), kind: param("type") },
            ["settings"] => Route::Settings { tab: "account".into() },
            ["settings", tab] if SETTINGS_TABS.contains(tab) => Route::Settings { tab: tab.to_string() },
            ["whats-new"] => Route::WhatsNew,
            ["bug-reports"] => Route::BugReports,
            ["admin"] => Route::Admin { page: "users".into() },
            ["admin", page] => Route::Admin { page: page.to_string() },
            ["files", source, ..] => Route::Files { source: source.to_string(), path: rest(2) },
            ["s", token, ..] => Route::Public { token: token.to_string(), path: rest(2) },
            _ => Route::NotFound,
        }
    }

    /// The app path of this route (without the base).
    pub fn href(&self) -> String {
        match self {
            Route::Home => "/".into(),
            Route::Login { next } if next == "/" => "/login".into(),
            Route::Login { next } => format!("/login{}", crate::api::query(&[("next", next)])),
            Route::Setup => "/setup".into(),
            Route::Files { source, path } => format!("/files/{}{}", encode(source), encode_path(path)),
            Route::Starred => "/starred".into(),
            Route::Shares => "/shares".into(),
            Route::Search { q, kind } => format!("/search{}", crate::api::query(&[("q", q), ("type", kind)])),
            Route::Collections => "/collections".into(),
            Route::Settings { tab } => format!("/settings/{tab}"),
            Route::WhatsNew => crate::kit::CHANGELOG_PATH.into(),
            Route::BugReports => "/bug-reports".into(),
            Route::Admin { page } => format!("/admin/{page}"),
            Route::Public { token, path } => format!("/s/{}{}", encode(token), encode_path(path)),
            Route::NotFound => "/404".into(),
        }
    }
}

/// A path inside a source as URL segments (`/a b/c` → `/a%20b/c`), "" for the root.
pub fn encode_path(path: &str) -> String {
    path.split('/').filter(|s| !s.is_empty()).map(|s| format!("/{}", encode(s))).collect()
}

/// The value of `key` in a query string, decoded, "" when it is not there.
pub fn param(query: &str, key: &str) -> String {
    query
        .trim_start_matches('?')
        .split('&')
        .filter_map(|p| p.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| decode(&v.replace('+', " ")))
        .unwrap_or_default()
}

/// The base path of a script address (`/next/filebrary-web-abc.js` → `/next`, `/filebrary-web-abc.js` → "").
pub fn base_of(script_src: &str) -> String {
    let path = script_src.find("://").and_then(|i| script_src[i + 3..].find('/').map(|j| &script_src[i + 3 + j..])).unwrap_or(script_src);
    path.rsplit_once('/').map(|(dir, _)| dir.trim_end_matches('/').to_string()).unwrap_or_default()
}

/// An address path without the base (`/next/files/x` → `/files/x`); `/` when it is the base itself.
pub fn strip_base<'a>(path: &'a str, base: &str) -> &'a str {
    if base.is_empty() {
        return path;
    }
    match path.strip_prefix(base) {
        Some("") => "/",
        Some(rest) if rest.starts_with('/') => rest,
        _ => path,
    }
}

thread_local! {
    static BASE: std::cell::OnceCell<String> = const { std::cell::OnceCell::new() };
    static CURRENT: std::cell::OnceCell<(ReadSignal<Route>, WriteSignal<Route>)> = const { std::cell::OnceCell::new() };
}

/// The app's base path ("" at the root), from the page's script.
pub fn base() -> String {
    BASE.with(|b| {
        b.get_or_init(|| {
            let src = kit_leptos::rt::qs(&format!(r#"script[src*="{}"], link[href*="{}"][rel="modulepreload"]"#, crate::NAME, crate::NAME))
                .and_then(|e| e.get_attribute("src").or_else(|| e.get_attribute("href")))
                .unwrap_or_default();
            base_of(&src)
        })
        .clone()
    })
}

/// The full address of an app path (`/files/x` → `/next/files/x`).
pub fn url(path: &str) -> String {
    let b = base();
    if path == "/" && !b.is_empty() {
        format!("{b}/")
    } else {
        format!("{b}{path}")
    }
}

/// The route of the address bar now.
pub fn from_location() -> Route {
    let loc = kit_leptos::rt::window().map(|w| w.location());
    let path = loc.as_ref().and_then(|l| l.pathname().ok()).unwrap_or_default();
    let query = loc.as_ref().and_then(|l| l.search().ok()).unwrap_or_default();
    Route::parse(strip_base(&path, &base()), &query)
}

/// The current route as a signal, kept in step with the address bar (Back and Forward included).
pub fn current() -> ReadSignal<Route> {
    CURRENT.with(|c| {
        c.get_or_init(|| {
            let (r, w) = signal(from_location());
            kit_leptos::rt::on_window("popstate", move |_| w.set(from_location()));
            (r, w)
        })
        .0
    })
}

fn set_current(r: Route) {
    current();
    CURRENT.with(|c| {
        if let Some((_, w)) = c.get() {
            w.set(r);
        }
    });
}

/// Opens `route`, adding a history entry (`replace` swaps the current one instead).
pub fn go(route: Route, replace: bool) {
    let target = url(&route.href());
    if let Some(h) = kit_leptos::rt::window().and_then(|w| w.history().ok()) {
        let _ = if replace {
            h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&target))
        } else {
            h.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&target))
        };
    }
    set_current(route);
}

/// A click handler for an in-app `<a href>`: plain left clicks go through [`go`] without reloading; clicks with
/// a modifier key (new tab, new window) are left to the browser.
pub fn on_link(route: Route) -> impl Fn(leptos::ev::MouseEvent) + Clone + 'static {
    move |e: leptos::ev::MouseEvent| {
        if e.button() != 0 || e.ctrl_key() || e.meta_key() || e.shift_key() || e.alt_key() {
            return;
        }
        e.prevent_default();
        go(route.clone(), false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_route_parses_back_from_its_href() {
        let all = [
            Route::Home,
            Route::Login { next: "/".into() },
            Route::Login { next: "/files/home/a b".into() },
            Route::Setup,
            Route::Files { source: "home".into(), path: "/".into() },
            Route::Files { source: "my s3".into(), path: "/Photos/2026/ü.jpg".into() },
            Route::Starred,
            Route::Shares,
            Route::Search { q: "tax 2025".into(), kind: "document".into() },
            Route::Search { q: String::new(), kind: String::new() },
            Route::Collections,
            Route::Settings { tab: "security".into() },
            Route::Admin { page: "users".into() },
            Route::Admin { page: "diagnostics".into() },
            Route::WhatsNew,
            Route::BugReports,
            Route::Public { token: "abc".into(), path: "/sub/x.txt".into() },
        ];
        for r in all {
            let href = r.href();
            let (p, q) = href.split_once('?').unwrap_or((&href, ""));
            assert_eq!(Route::parse(p, q), r, "{href}");
        }
    }

    #[test]
    fn odd_addresses() {
        assert_eq!(Route::parse("/nope", ""), Route::NotFound);
        assert_eq!(Route::parse("/settings/bogus", ""), Route::NotFound);
        assert_eq!(Route::parse("/settings", ""), Route::Settings { tab: "account".into() });
        assert_eq!(Route::parse("//files//home//a/", ""), Route::Files { source: "home".into(), path: "/a".into() });
        // only same-site paths are followed after signing in
        assert_eq!(Route::parse("/login", "?next=https://evil.example"), Route::Login { next: "/".into() });
        assert_eq!(Route::parse("/login", "?next=//evil.example"), Route::Login { next: "/".into() });
        assert_eq!(Route::parse("/search", "?q=a+b"), Route::Search { q: "a b".into(), kind: String::new() });
        assert_eq!(Route::parse("/s/t", ""), Route::Public { token: "t".into(), path: "/".into() });
    }

    #[test]
    fn the_base_comes_from_the_script() {
        assert_eq!(base_of("/next/filebrary-web-abc.js"), "/next");
        assert_eq!(base_of("https://host:8080/next/filebrary-web-abc.js"), "/next");
        assert_eq!(base_of("/filebrary-web-abc.js"), "");
        assert_eq!(base_of(""), "");
        assert_eq!(strip_base("/next/files/a", "/next"), "/files/a");
        assert_eq!(strip_base("/next", "/next"), "/");
        assert_eq!(strip_base("/next/", "/next"), "/");
        assert_eq!(strip_base("/nextish", "/next"), "/nextish");
        assert_eq!(strip_base("/files", ""), "/files");
    }
}
