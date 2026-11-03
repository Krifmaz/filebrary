//! Virtual paths: the only way a client-supplied path reaches a storage backend.
//!
//! A [`VPath`] is a normalised path *relative to a source root*. It is built
//! from untrusted input by [`VPath::parse`], which refuses anything that could
//! escape the root before a backend ever sees it:
//!
//! * `..` components are rejected outright (never "resolved"), so
//!   `a/../../etc` fails instead of being silently clamped;
//! * backslashes are rejected, so `..\..\x` cannot act as a separator on
//!   Windows and names stay portable between backends;
//! * NUL bytes and over-long components are rejected;
//! * on Windows, drive prefixes and alternate data streams (`:`) and reserved
//!   device names (`CON`, `NUL`, ...) are rejected.
//!
//! Percent-decoding happens exactly once, in the HTTP layer. A string such as
//! `%2e%2e` that reaches this module is therefore a literal file name made of
//! six characters, not a parent reference, and is treated as such.
//!
//! Symlink checks need the real file system and live in the local backend; see
//! [`crate::local`].

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::StorageError;

/// Longest single path component accepted, in bytes (the common file-system limit).
pub const MAX_COMPONENT_LEN: usize = 255;
/// Deepest path accepted, in components.
pub const MAX_DEPTH: usize = 256;

/// A normalised, validated path relative to a source root.
///
/// The empty path (`VPath::root()`) denotes the root of the source itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VPath {
    parts: Vec<String>,
}

impl VPath {
    /// The root of a source.
    pub fn root() -> Self {
        Self::default()
    }

    /// Parses and validates an untrusted path.
    ///
    /// Leading, trailing and repeated slashes and `.` components are ignored.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::InvalidPath`] when the input contains `..`,
    /// a backslash, a NUL byte, an over-long component, or (on Windows) a
    /// drive prefix, stream separator or reserved device name.
    pub fn parse(input: &str) -> Result<Self, StorageError> {
        let mut parts = Vec::new();
        for comp in input.split('/') {
            if comp.is_empty() || comp == "." {
                continue;
            }
            validate_component(comp)?;
            parts.push(comp.to_owned());
            if parts.len() > MAX_DEPTH {
                return Err(StorageError::InvalidPath("path is too deep".into()));
            }
        }
        Ok(Self { parts })
    }

    /// Returns `true` for the source root.
    pub fn is_root(&self) -> bool {
        self.parts.is_empty()
    }

    /// The path components, outermost first.
    pub fn components(&self) -> &[String] {
        &self.parts
    }

    /// The final component, or `None` for the root.
    pub fn name(&self) -> Option<&str> {
        self.parts.last().map(String::as_str)
    }

    /// The parent path, or `None` for the root.
    pub fn parent(&self) -> Option<VPath> {
        if self.parts.is_empty() {
            None
        } else {
            Some(Self {
                parts: self.parts[..self.parts.len() - 1].to_vec(),
            })
        }
    }

    /// Appends one validated component.
    ///
    /// # Errors
    ///
    /// Fails if `name` is not a single valid component (contains `/`, is `..`, ...).
    pub fn join(&self, name: &str) -> Result<VPath, StorageError> {
        if name.contains('/') || name.is_empty() || name == "." {
            return Err(StorageError::InvalidPath("invalid file name".into()));
        }
        validate_component(name)?;
        let mut parts = self.parts.clone();
        parts.push(name.to_owned());
        Ok(Self { parts })
    }

    /// Appends another relative path.
    pub fn join_path(&self, other: &VPath) -> VPath {
        let mut parts = self.parts.clone();
        parts.extend(other.parts.iter().cloned());
        Self { parts }
    }

    /// Returns `true` if `self` equals `other` or lies inside it.
    pub fn starts_with(&self, other: &VPath) -> bool {
        self.parts.len() >= other.parts.len() && self.parts[..other.parts.len()] == other.parts[..]
    }

    /// Strips `prefix` from the front of this path.
    pub fn strip_prefix(&self, prefix: &VPath) -> Option<VPath> {
        if self.starts_with(prefix) {
            Some(Self {
                parts: self.parts[prefix.parts.len()..].to_vec(),
            })
        } else {
            None
        }
    }
}

impl fmt::Display for VPath {
    /// Formats as an absolute-looking path such as `/a/b` (or `/` for the root).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.parts.is_empty() {
            return f.write_str("/");
        }
        for p in &self.parts {
            f.write_str("/")?;
            f.write_str(p)?;
        }
        Ok(())
    }
}

impl Serialize for VPath {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for VPath {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        VPath::parse(&s).map_err(serde::de::Error::custom)
    }
}

fn validate_component(comp: &str) -> Result<(), StorageError> {
    if comp == ".." {
        return Err(StorageError::InvalidPath(
            "parent references are not allowed".into(),
        ));
    }
    if comp.contains('\\') {
        return Err(StorageError::InvalidPath(
            "backslashes are not allowed".into(),
        ));
    }
    if comp.contains('\0') {
        return Err(StorageError::InvalidPath(
            "NUL bytes are not allowed".into(),
        ));
    }
    if comp.len() > MAX_COMPONENT_LEN {
        return Err(StorageError::InvalidPath("file name is too long".into()));
    }
    if cfg!(windows) {
        validate_windows_component(comp)?;
    }
    Ok(())
}

/// Extra rules for Windows, where `:` selects a drive or a data stream and some
/// names refer to devices. Kept separate and always compiled so it is tested on
/// every platform.
fn validate_windows_component(comp: &str) -> Result<(), StorageError> {
    if comp.contains(':') {
        return Err(StorageError::InvalidPath("':' is not allowed".into()));
    }
    if comp.ends_with('.') || comp.ends_with(' ') {
        return Err(StorageError::InvalidPath(
            "names may not end with a dot or space".into(),
        ));
    }
    let stem = comp.split('.').next().unwrap_or("").to_ascii_uppercase();
    const RESERVED: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];
    let is_numbered =
        |p: &str| stem.len() == 4 && stem.starts_with(p) && stem.as_bytes()[3].is_ascii_digit();
    if RESERVED.contains(&stem.as_str()) || is_numbered("COM") || is_numbered("LPT") {
        return Err(StorageError::InvalidPath("reserved device name".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_slashes_and_dots() {
        let p = VPath::parse("//a/./b///c/").unwrap();
        assert_eq!(p.to_string(), "/a/b/c");
        assert!(VPath::parse("").unwrap().is_root());
        assert!(VPath::parse("/").unwrap().is_root());
        assert!(VPath::parse("./.").unwrap().is_root());
    }

    #[test]
    fn rejects_parent_references() {
        for bad in [
            "..",
            "/..",
            "a/..",
            "a/../b",
            "../../etc/passwd",
            "a/b/../../..",
        ] {
            assert!(VPath::parse(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn rejects_backslash_and_nul() {
        for bad in ["..\\..\\x", "a\\b", "a/b\\..", "a\0b", "\0"] {
            assert!(VPath::parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn encoded_dots_are_literal_names() {
        // Decoding happens once in the HTTP layer; what reaches us is literal.
        let p = VPath::parse("%2e%2e/%2E%2E/x").unwrap();
        assert_eq!(p.components(), ["%2e%2e", "%2E%2E", "x"]);
        let p = VPath::parse("%252e%252e").unwrap();
        assert_eq!(p.components(), ["%252e%252e"]);
        // Dots that are not exactly ".." are ordinary names.
        assert_eq!(VPath::parse("...").is_ok(), !cfg!(windows));
        assert!(VPath::parse("..a").is_ok());
    }

    #[test]
    fn rejects_overlong() {
        let long = "x".repeat(MAX_COMPONENT_LEN + 1);
        assert!(VPath::parse(&long).is_err());
        let deep = "a/".repeat(MAX_DEPTH + 1);
        assert!(VPath::parse(&deep).is_err());
    }

    #[test]
    fn windows_rules() {
        for bad in [
            "C:",
            "file.txt:stream",
            "CON",
            "nul.txt",
            "com1",
            "LPT9.log",
            "trail.",
            "sp ",
        ] {
            assert!(
                validate_windows_component(bad).is_err(),
                "{bad} should be rejected"
            );
        }
        for ok in ["CONSOLE", "com", "comx", "normal.txt"] {
            assert!(
                validate_windows_component(ok).is_ok(),
                "{ok} should be accepted"
            );
        }
    }

    #[test]
    fn join_and_prefix() {
        let base = VPath::parse("a/b").unwrap();
        assert!(base.join("..").is_err());
        assert!(base.join("x/y").is_err());
        let c = base.join("c").unwrap();
        assert!(c.starts_with(&base));
        assert!(!base.starts_with(&c));
        assert_eq!(c.strip_prefix(&base).unwrap().to_string(), "/c");
        assert_eq!(c.parent().unwrap(), base);
        assert!(!VPath::parse("a/bc").unwrap().starts_with(&base));
    }

    #[test]
    fn serde_round_trip_validates() {
        let p: VPath = serde_json::from_str("\"/x/y\"").unwrap();
        assert_eq!(p.to_string(), "/x/y");
        assert!(serde_json::from_str::<VPath>("\"/x/../y\"").is_err());
    }
}
