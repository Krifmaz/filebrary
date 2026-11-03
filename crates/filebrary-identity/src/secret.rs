//! A string that never shows up in `Debug` output or logs.

use serde::Deserialize;

/// A secret from the configuration (a client secret or bind password).
///
/// `Debug` prints `"<redacted>"`; read the value with [`SecretString::expose`].
#[derive(Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct SecretString(String);

impl SecretString {
    /// Wraps a value.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// The secret itself.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("\"<redacted>\"")
    }
}

/// Reads a secret from `value` or `file` (trailing newlines trimmed).
pub(crate) fn read(
    value: Option<&SecretString>,
    file: Option<&std::path::Path>,
) -> std::io::Result<Option<String>> {
    if let Some(v) = value {
        return Ok(Some(v.expose().to_owned()));
    }
    match file {
        Some(f) => {
            std::fs::read_to_string(f).map(|s| Some(s.trim_end_matches(['\r', '\n']).to_owned()))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_is_redacted() {
        let s = SecretString::new("hunter2");
        assert!(!format!("{s:?}").contains("hunter2"));
        assert_eq!(s.expose(), "hunter2");
    }
}
