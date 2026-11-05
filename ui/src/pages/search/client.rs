//! The search calls: `GET /api/search` and its answer. They sit with the page rather than in `api.rs` so the
//! search change touches only its own folder.

use kit_leptos::api::{self, ApiError};
use serde::Deserialize;

/// One result.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Hit {
    /// The source's id.
    pub source: String,
    /// The path inside it, starting with `/`.
    pub path: String,
    /// The file or folder name.
    pub name: String,
    /// The coarse type: `dir`, `image`, `video`, `audio`, `document`, `text`, `code`, `archive` or `other`.
    #[serde(rename = "type", default)]
    pub kind: String,
    /// Whether it is a folder.
    #[serde(default)]
    pub is_dir: bool,
    /// Size in bytes (0 for folders).
    #[serde(default)]
    pub size: u64,
    /// Last change, RFC 3339.
    #[serde(default)]
    pub modified: Option<String>,
    /// For text searches: an escaped excerpt with the matches in `<b>…</b>`.
    #[serde(default)]
    pub snippet: Option<String>,
}

/// One page of results.
#[derive(Clone, Debug, PartialEq, Deserialize, Default)]
pub struct Results {
    /// The matches, in the asked order.
    #[serde(default)]
    pub hits: Vec<Hit>,
    /// More matches exist past this page.
    #[serde(default)]
    pub has_more: bool,
    /// A source is still being indexed, so results may be incomplete.
    #[serde(default)]
    pub indexing: bool,
}

/// Runs one search; `path` comes from [`super::query::Search::api_path`].
pub async fn search(path: &str) -> Result<Results, ApiError> {
    let v = api::get(path).await?;
    serde_json::from_value(v).map_err(|e| ApiError::new(format!("The server's answer could not be read: {e}")))
}

/// Whether a failed search failed because the server has no text index (the server's sentence for it).
pub fn content_off(e: &ApiError) -> bool {
    e.status == 400 && e.message.contains("content search is not enabled")
}

/// Whether a failed search failed because search is off on the server.
pub fn search_off(e: &ApiError) -> bool {
    e.status == 400 && e.message.contains("search is disabled")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn answers_decode() {
        let r: Results = serde_json::from_value(json!({
            "hits": [
                {"source": "home", "path": "/a/b.pdf", "name": "b.pdf", "type": "document", "is_dir": false, "size": 10,
                 "modified": "2026-01-01T00:00:00Z", "mime": "application/pdf", "snippet": "the <b>tax</b>"},
                {"source": "home", "path": "/a", "name": "a", "type": "dir", "is_dir": true, "size": 0, "modified": null, "mime": null}
            ],
            "has_more": true, "indexing": false
        }))
        .unwrap();
        assert_eq!(r.hits.len(), 2);
        assert_eq!(r.hits[0].kind, "document");
        assert!(r.hits[1].is_dir && r.hits[1].modified.is_none() && r.hits[1].snippet.is_none());
        assert!(r.has_more);
    }
}
