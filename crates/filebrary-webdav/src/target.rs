//! Request targets: decoding URLs into sources and [`VPath`]s, and back.
//!
//! The WebDAV URL space is `<prefix>/<source>/<path...>`, where `<prefix>` is
//! the mount point (normally `/dav`). Percent-decoding happens exactly once,
//! here; each decoded segment then goes through [`VPath::join`], which refuses
//! `..`, backslashes, NUL bytes and the other things the storage layer never
//! accepts. A decoded segment that contains `/` (from `%2F`) is refused too, so
//! an encoded separator can never introduce a parent reference.

use filebrary_storage::VPath;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

/// Characters percent-encoded when a segment is written into an href:
/// everything except RFC 3986 unreserved characters and a few sub-delimiters
/// that clients (Windows Explorer in particular) leave alone.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'!')
    .remove(b'$')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')')
    .remove(b'*')
    .remove(b'+')
    .remove(b',')
    .remove(b';')
    .remove(b'=')
    .remove(b'@');

/// Why a request URL was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TargetError {
    /// A segment was not valid percent-encoded UTF-8.
    #[error("path is not valid UTF-8")]
    BadEncoding,
    /// A segment was refused by path validation (`..`, `\`, NUL, `%2F`, ...).
    #[error("path segment is not allowed")]
    Forbidden,
    /// The path does not start with the WebDAV mount prefix.
    #[error("path is outside the WebDAV mount")]
    OutsideMount,
}

/// A request target resolved against the mount.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    /// The mount root (`/dav/`): a read-only collection listing the sources.
    Mount,
    /// A path in a named source, as the user sees it (before their scope is
    /// applied). `VPath::root()` is the source itself.
    Source {
        /// The source name: the first segment of the URL.
        source: String,
        /// The user-visible path inside the source.
        path: VPath,
    },
}

/// Decodes the path part of a request URI (still percent-encoded, without the
/// query string) relative to `prefix`.
///
/// Empty segments (from `//` or a trailing slash) and `.` are ignored, the way
/// [`VPath::parse`] ignores them.
///
/// # Errors
///
/// See [`TargetError`].
pub fn parse_target(prefix: &str, raw_path: &str) -> Result<Target, TargetError> {
    let prefix = prefix.trim_end_matches('/');
    let rest = raw_path
        .strip_prefix(prefix)
        .ok_or(TargetError::OutsideMount)?;
    if !(rest.is_empty() || rest.starts_with('/')) {
        return Err(TargetError::OutsideMount);
    }
    let mut source: Option<String> = None;
    let mut path = VPath::root();
    for raw in rest.split('/') {
        if raw.is_empty() {
            continue;
        }
        let decoded = percent_decode_str(raw)
            .decode_utf8()
            .map_err(|_| TargetError::BadEncoding)?;
        if decoded == "." {
            continue;
        }
        if decoded.contains('/') {
            return Err(TargetError::Forbidden);
        }
        match source {
            None => {
                // The source name must pass the same rules as a path segment.
                VPath::root()
                    .join(&decoded)
                    .map_err(|_| TargetError::Forbidden)?;
                source = Some(decoded.into_owned());
            }
            Some(_) => {
                path = path.join(&decoded).map_err(|_| TargetError::Forbidden)?;
            }
        }
    }
    Ok(match source {
        None => Target::Mount,
        Some(source) => Target::Source { source, path },
    })
}

/// Extracts the path of a `Destination` header or a tagged `If` resource,
/// which may be an absolute URI (`http://host/dav/x`) or an absolute path.
///
/// Returns the raw (still encoded) path and, for an absolute URI, its
/// authority (`host[:port]`) so the caller can check it names this server.
pub fn split_uri(uri: &str) -> Option<(Option<&str>, &str)> {
    let (authority, path) = if let Some(rest) = uri
        .strip_prefix("http://")
        .or_else(|| uri.strip_prefix("https://"))
    {
        match rest.find('/') {
            Some(i) => (Some(&rest[..i]), &rest[i..]),
            None => (Some(rest), "/"),
        }
    } else if uri.starts_with('/') {
        (None, uri)
    } else {
        return None;
    };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    Some((authority, path))
}

/// Encodes one decoded segment for use in an href.
pub fn encode_segment(s: &str) -> String {
    utf8_percent_encode(s, SEGMENT).to_string()
}

/// Builds the href of a path in a source. Collections get a trailing slash.
pub fn href(prefix: &str, source: &str, path: &VPath, is_dir: bool) -> String {
    let mut s = prefix.trim_end_matches('/').to_owned();
    s.push('/');
    s.push_str(&encode_segment(source));
    for seg in path.components() {
        s.push('/');
        s.push_str(&encode_segment(seg));
    }
    if is_dir {
        s.push('/');
    }
    s
}

/// The href of the mount root.
pub fn mount_href(prefix: &str) -> String {
    format!("{}/", prefix.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(source: &str, path: &str) -> Target {
        Target::Source {
            source: source.into(),
            path: VPath::parse(path).unwrap(),
        }
    }

    #[test]
    fn parses_mount_and_sources() {
        assert_eq!(parse_target("/dav", "/dav").unwrap(), Target::Mount);
        assert_eq!(parse_target("/dav", "/dav/").unwrap(), Target::Mount);
        assert_eq!(parse_target("/dav/", "/dav/home").unwrap(), src("home", ""));
        assert_eq!(
            parse_target("/dav", "/dav/home/a%20b//c/").unwrap(),
            src("home", "a b/c")
        );
        assert_eq!(
            parse_target("/dav", "/dav/home/%E2%9C%93").unwrap(),
            src("home", "✓")
        );
        assert_eq!(
            parse_target("/dav", "/dav/home/./a").unwrap(),
            src("home", "a")
        );
    }

    #[test]
    fn refuses_traversal() {
        for p in [
            "/dav/home/..",
            "/dav/home/../etc",
            "/dav/home/%2e%2e/etc",
            "/dav/home/%2E%2E",
            "/dav/home/a%2f..%2fb",
            "/dav/home/a%2F",
            "/dav/home/..%5c..%5cetc",
            "/dav/home/a%00b",
            "/dav/..",
            "/dav/%2e%2e/x",
        ] {
            assert_eq!(parse_target("/dav", p), Err(TargetError::Forbidden), "{p}");
        }
        assert_eq!(
            parse_target("/dav", "/dav/home/%ff"),
            Err(TargetError::BadEncoding)
        );
        assert_eq!(
            parse_target("/dav", "/davx/a"),
            Err(TargetError::OutsideMount)
        );
        assert_eq!(
            parse_target("/dav", "/other"),
            Err(TargetError::OutsideMount)
        );
    }

    #[test]
    fn double_encoding_stays_literal() {
        // %252e decodes once to the literal name "%2e", never to ".".
        assert_eq!(
            parse_target("/dav", "/dav/home/%252e%252e").unwrap(),
            src("home", "%2e%2e")
        );
    }

    #[test]
    fn builds_hrefs() {
        let p = VPath::parse("a b/ü#?").unwrap();
        assert_eq!(
            href("/dav", "home", &p, false),
            "/dav/home/a%20b/%C3%BC%23%3F"
        );
        assert_eq!(
            href("/dav/", "my src", &VPath::root(), true),
            "/dav/my%20src/"
        );
        assert_eq!(mount_href("/dav/"), "/dav/");
    }

    #[test]
    fn splits_uris() {
        assert_eq!(
            split_uri("http://h:8080/dav/a?x#y"),
            Some((Some("h:8080"), "/dav/a"))
        );
        assert_eq!(split_uri("https://h"), Some((Some("h"), "/")));
        assert_eq!(split_uri("/dav/a"), Some((None, "/dav/a")));
        assert_eq!(split_uri("ftp://h/x"), None);
    }
}
