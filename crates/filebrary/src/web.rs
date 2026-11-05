//! The embedded web UI: the Leptos app built from `ui/` into `ui/dist` by trunk.
//!
//! Hashed build files (`filebrary-web-<hash>.js`, `…_bg.wasm`, `app-<hash>.css`)
//! are cached for a year. trunk writes every file at the top level, so a
//! missing top-level name with a dot (an old script) is a 404; every other
//! path falls back to `index.html`, so the app's own routes work, deep links
//! to files (`/s/<token>/report.pdf`) included. The page loads WebAssembly from an inline module script, so the UI's
//! answers carry their own Content-Security-Policy: the server's, plus
//! `'wasm-unsafe-eval'` and the SHA-256 of each inline script.

use axum::{
    body::Body,
    http::{HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use rust_embed::RustEmbed;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

/// Files from `ui/dist`, compiled into the binary in release builds.
#[derive(RustEmbed)]
#[folder = "../../ui/dist/"]
#[allow_missing = true]
struct Assets;

/// The build of the web UI: the hash in the name of its script
/// (`filebrary-web-<hash>.js`), `None` when `ui/dist` was not built.
pub fn build() -> Option<String> {
    let mut builds: Vec<String> = Assets::iter()
        .filter_map(|name| {
            kit_server_health::web_build_of(&name, "filebrary-web-").map(str::to_owned)
        })
        .collect();
    builds.sort();
    builds.pop()
}

/// The bytes of an embedded UI file, if present (used to patch the PWA manifest).
pub fn embedded(path: &str) -> Option<Vec<u8>> {
    Assets::get(path).map(|a| a.data.into_owned())
}

/// Serves an embedded file, or `index.html` for an app address.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path == "api" || path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let name = if path.is_empty() { "index.html" } else { path };
    let (data, mime, page) = match Assets::get(name) {
        Some(a) => (
            a.data.into_owned(),
            a.metadata.mimetype().to_owned(),
            name == "index.html",
        ),
        None if is_build_file(name) => return StatusCode::NOT_FOUND.into_response(),
        None => match Assets::get("index.html") {
            Some(a) => (a.data.into_owned(), "text/html".to_owned(), true),
            None => {
                return (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                    "Filebrary is running, but the web UI was not built into this binary.\n\
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
    if page && let Ok(v) = HeaderValue::from_str(page_csp()) {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    resp
}

/// Whether `path` looks like one of trunk's files: a top-level name with a dot.
fn is_build_file(path: &str) -> bool {
    !path.contains('/') && path.contains('.')
}

/// Whether a trunk output name carries a content hash (`app-0123456789abcdef.css`; trunk drops leading
/// zeros, so the hash can be shorter than 16 digits).
fn hashed(name: &str) -> bool {
    let stem = name.rsplit('/').next().unwrap_or(name);
    let stem = stem.split('.').next().unwrap_or(stem);
    let stem = stem.strip_suffix("_bg").unwrap_or(stem);
    stem.rsplit_once('-').is_some_and(|(_, h)| {
        (12..=16).contains(&h.len()) && h.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

/// The UI page's Content-Security-Policy, for its embedded `index.html`.
fn page_csp() -> &'static str {
    static CSP: OnceLock<String> = OnceLock::new();
    CSP.get_or_init(|| {
        let page = Assets::get("index.html")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_files_are_top_level_names_with_a_dot() {
        assert!(is_build_file("filebrary-web-abc.js"));
        assert!(is_build_file("app-0123456789abcdef.css"));
        assert!(!is_build_file("s/abc123/report.pdf"));
        assert!(!is_build_file("files/home/v1.2"));
        assert!(!is_build_file("search"));
    }

    #[test]
    fn hashed_names() {
        assert!(hashed("app-6ecd585ddbbbd1c2.css"));
        assert!(hashed("filebrary-web-e44db03bad17ba6a_bg.wasm"));
        assert!(hashed("filebrary-web-f7135f924d3b6947.js"));
        assert!(hashed("filebrary-a535c2830d3092f.css"));
        assert!(!hashed("icon.svg"));
        assert!(!hashed("index.html"));
        assert!(!hashed("my-file.css"));
    }

    #[test]
    fn inline_scripts_are_hashed_into_the_policy() {
        let body = "\nimport init from '/x.js';\n";
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
