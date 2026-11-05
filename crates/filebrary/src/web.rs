//! The embedded web UI (built from `web/` into `web/dist`).
//!
//! Hashed assets under `/assets/` are cached for a year; every other path that
//! is not a file falls back to `index.html` so client-side routes work.
//!
//! The new Leptos UI (built from `ui/` into `ui/dist` by trunk) is served at
//! `/next/` while `FILEBRARY_NEXT_UI` is on (`1`, `true`, `yes` or `on`), so
//! it can be tried next to the React UI until it replaces it. Its page loads
//! WebAssembly from an inline module script, so its answers carry their own
//! Content-Security-Policy: the server's, plus `'wasm-unsafe-eval'` and the
//! SHA-256 of each inline script.

use axum::{
    body::Body,
    http::{HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use rust_embed::RustEmbed;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

/// Files from `web/dist`, compiled into the binary in release builds.
#[derive(RustEmbed)]
#[folder = "../../web/dist/"]
#[allow_missing = true]
struct Assets;

/// Files from `ui/dist` (the Leptos UI), compiled into the binary in release builds.
#[derive(RustEmbed)]
#[folder = "../../ui/dist/"]
#[allow_missing = true]
struct NextAssets;

/// The build of the Leptos UI: the hash in the name of its script
/// (`filebrary-web-<hash>.js`), `None` when `ui/dist` was not built.
pub fn next_build() -> Option<String> {
    let mut builds: Vec<String> = NextAssets::iter()
        .filter_map(|name| {
            kit_server_health::web_build_of(&name, "filebrary-web-").map(str::to_owned)
        })
        .collect();
    builds.sort();
    builds.pop()
}

/// The environment variable that turns the new UI at `/next/` on.
pub const NEXT_UI_ENV: &str = "FILEBRARY_NEXT_UI";

/// Serves an embedded file, or `index.html` for unknown non-API paths.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Some(rest) = next_path(path)
        && next_enabled()
    {
        return next(rest);
    }
    if let Some(resp) = file(path) {
        return resp;
    }
    file("index.html").unwrap_or_else(|| {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "Filebrary is running, but the web UI was not built into this binary.\n\
             Build it with `npm --prefix web ci && npm --prefix web run build`, then rebuild.\n",
        )
            .into_response()
    })
}

/// The bytes of an embedded UI file, if present (used to patch the PWA manifest).
pub fn embedded(path: &str) -> Option<Vec<u8>> {
    Assets::get(path).map(|a| a.data.into_owned())
}

/// The part of `path` (without its leading `/`) inside `/next/`, if it is there.
fn next_path(path: &str) -> Option<&str> {
    match path.strip_prefix("next") {
        Some("") => Some(""),
        Some(rest) => rest.strip_prefix('/'),
        None => None,
    }
}

/// Whether `FILEBRARY_NEXT_UI` turns the new UI on.
fn next_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| switch_on(std::env::var(NEXT_UI_ENV).ok().as_deref()))
}

/// Whether a switch's value means on.
fn switch_on(v: Option<&str>) -> bool {
    v.is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Serves a file of the new UI, or its `index.html` for an app address. A
/// missing file (a name with a dot) is a 404, so an old script is not
/// answered with a page.
fn next(path: &str) -> Response {
    let name = if path.is_empty() { "index.html" } else { path };
    let (data, mime, page) = match NextAssets::get(name) {
        Some(a) => (
            a.data.into_owned(),
            a.metadata.mimetype().to_owned(),
            name == "index.html",
        ),
        None if name
            .rsplit('/')
            .next()
            .is_some_and(|last| last.contains('.')) =>
        {
            return StatusCode::NOT_FOUND.into_response();
        }
        None => match NextAssets::get("index.html") {
            Some(a) => (a.data.into_owned(), "text/html".to_owned(), true),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                    "The new web UI was not built into this binary.\n\
                     Build it with `trunk build --release` in ui/, then rebuild.\n",
                )
                    .into_response();
            }
        },
    };
    let cache = if !page && hashed(name) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let mut resp = Response::new(Body::from(data));
    let h = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&mime) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if let Ok(v) = HeaderValue::from_str(next_csp()) {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    resp
}

/// Whether a trunk output name carries a content hash (`app-0123456789abcdef.css`).
fn hashed(name: &str) -> bool {
    let stem = name.rsplit('/').next().unwrap_or(name);
    let stem = stem.split('.').next().unwrap_or(stem);
    let stem = stem.strip_suffix("_bg").unwrap_or(stem);
    stem.rsplit_once('-')
        .is_some_and(|(_, h)| h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The new UI's Content-Security-Policy, for its embedded `index.html`.
fn next_csp() -> &'static str {
    static CSP: OnceLock<String> = OnceLock::new();
    CSP.get_or_init(|| {
        let page = NextAssets::get("index.html")
            .map(|a| String::from_utf8_lossy(&a.data).into_owned())
            .unwrap_or_default();
        csp_for(&page)
    })
}

/// The SHA-256 sources (`'sha256-…'`) of the inline scripts of `page`.
fn inline_script_hashes(page: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = page;
    while let Some(start) = rest.find("<script") {
        let after = &rest[start..];
        let Some(open_end) = after.find('>') else {
            break;
        };
        let tag = &after[..open_end];
        let body_and_more = &after[open_end + 1..];
        let Some(close) = body_and_more.find("</script>") else {
            break;
        };
        if !tag.contains("src=") {
            let body = &body_and_more[..close];
            out.push(format!(
                "'sha256-{}'",
                STANDARD.encode(Sha256::digest(body.as_bytes()))
            ));
        }
        rest = &body_and_more[close + "</script>".len()..];
    }
    out
}

/// The server's policy with WebAssembly and `page`'s inline scripts allowed.
fn csp_for(page: &str) -> String {
    let mut script = vec!["'self'".to_string(), "'wasm-unsafe-eval'".to_string()];
    script.extend(inline_script_hashes(page));
    format!(
        "default-src 'self'; script-src {}; img-src 'self' data: blob:; media-src 'self' blob:; \
         style-src 'self' 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        script.join(" ")
    )
}

fn file(path: &str) -> Option<Response> {
    let asset = Assets::get(if path.is_empty() { "index.html" } else { path })?;
    let mime = asset.metadata.mimetype().to_owned();
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let mut resp = Response::new(Body::from(asset.data.into_owned()));
    let h = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&mime) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    Some(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_paths() {
        assert_eq!(next_path("next"), Some(""));
        assert_eq!(next_path("next/"), Some(""));
        assert_eq!(next_path("next/files/home"), Some("files/home"));
        assert_eq!(next_path("nextcloud"), None);
        assert_eq!(next_path("files/next"), None);
    }

    #[test]
    fn the_switch_reads_plain_values() {
        for on in ["1", "true", "YES", " on "] {
            assert!(switch_on(Some(on)), "{on}");
        }
        for off in ["0", "false", "", "maybe"] {
            assert!(!switch_on(Some(off)), "{off}");
        }
        assert!(!switch_on(None));
    }

    #[test]
    fn hashed_names() {
        assert!(hashed("app-6ecd585ddbbbd1c2.css"));
        assert!(hashed("filebrary-web-e44db03bad17ba6a_bg.wasm"));
        assert!(hashed("filebrary-web-f7135f924d3b6947.js"));
        assert!(!hashed("icon.svg"));
        assert!(!hashed("index.html"));
        assert!(!hashed("my-file.css"));
    }

    #[test]
    fn inline_scripts_are_hashed_into_the_policy() {
        let body = "\nimport init from '/next/x.js';\n";
        let page = format!(
            r#"<head><script type="module">{body}</script><script src="/x.js"></script></head>"#
        );
        let hashes = inline_script_hashes(&page);
        let want = format!(
            "'sha256-{}'",
            STANDARD.encode(Sha256::digest(body.as_bytes()))
        );
        assert_eq!(hashes, vec![want.clone()]);
        let csp = csp_for(&page);
        assert!(csp.contains(&format!("script-src 'self' 'wasm-unsafe-eval' {want};")));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(inline_script_hashes("<p>no scripts</p>").is_empty());
    }
}
