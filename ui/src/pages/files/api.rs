//! The calls browsing makes beyond the shared client in `crate::api`: folders, moves and copies, the trash,
//! stars and tags, the text editor, uploads, casting, comments and collections. JSON calls go through the kit's
//! backend; raw bodies go through [`super::http`].

use super::http::{Abort, Request};
use crate::api::{encode, query, source_path, Entry, Error, Listing, PreviewCapabilities, Result, UploadSession};
use kit_leptos::api;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::rc::Rc;

fn decode<T: DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|e| Error::new(format!("The server's answer could not be read: {e}")))
}

/// One folder's contents.
pub async fn list(source: &str, path: &str) -> Result<Listing> {
    crate::api::list(source, path).await
}

/// Details of one path.
pub async fn stat(source: &str, path: &str) -> Result<Entry> {
    decode(api::get(&format!("{}/stat{}", source_path(source), query(&[("path", path)]))).await?)
}

/// Makes a folder (and any missing parents).
pub async fn mkdir(source: &str, path: &str) -> Result<()> {
    api::post(&format!("{}/mkdir", source_path(source)), json!({ "path": path })).await.map(drop)
}

/// Renames or moves one path.
pub async fn move_to(source: &str, from: &str, to: &str) -> Result<()> {
    api::post(&format!("{}/move", source_path(source)), json!({ "from": from, "to": to })).await.map(drop)
}

/// Copies one path.
pub async fn copy_to(source: &str, from: &str, to: &str) -> Result<()> {
    api::post(&format!("{}/copy", source_path(source)), json!({ "from": from, "to": to })).await.map(drop)
}

/// The address that streams a zip of `paths`.
pub fn zip_url(source: &str, paths: &[String]) -> String {
    let q: Vec<String> = paths.iter().map(|p| format!("path={}", encode(p))).collect();
    format!("/api{}/zip?{}", source_path(source), q.join("&"))
}

/// The preview capabilities, asked once per page.
pub async fn preview_capabilities() -> PreviewCapabilities {
    thread_local! {
        static CAPS: std::cell::RefCell<Option<PreviewCapabilities>> = const { std::cell::RefCell::new(None) };
    }
    if let Some(c) = CAPS.with(|c| c.borrow().clone()) {
        return c;
    }
    let c = crate::api::preview_capabilities().await.unwrap_or_default();
    CAPS.with(|x| *x.borrow_mut() = Some(c.clone()));
    c
}

// ---------- trash ----------

/// An item in the trash.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct TrashItem {
    /// Its id, for restore and purge.
    pub id: String,
    /// The source it came from.
    pub source: String,
    /// Where it was.
    pub path: String,
    /// Its name.
    pub name: String,
    /// `file` or `dir`.
    #[serde(default)]
    pub kind: String,
    /// Bytes (0 for folders).
    #[serde(default)]
    pub size: i64,
    /// Who deleted it.
    #[serde(default)]
    pub deleted_by: Option<String>,
    /// When, Unix seconds.
    #[serde(default)]
    pub deleted_at: i64,
    /// When it goes for good, Unix seconds.
    #[serde(default)]
    pub purge_at: i64,
}

/// What a delete did: items moved to the trash, and paths deleted for good (the trash is off).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Removed {
    /// Moved to the trash (restorable).
    #[serde(default)]
    pub trashed: Vec<TrashItem>,
    /// Deleted for good.
    #[serde(default)]
    pub deleted: Vec<String>,
}

/// Deletes `paths`: into the trash when it is on, for good otherwise.
pub async fn trash(source: &str, paths: &[String]) -> Result<Removed> {
    decode(api::post("/trash", json!({ "source": source, "paths": paths })).await?)
}

/// The trash of one source.
pub async fn trash_list(source: &str) -> Result<Vec<TrashItem>> {
    decode(api::get(&format!("/trash{}", query(&[("source", source)]))).await?)
}

/// Puts an item back where it was (`rename` keeps both when something took its place).
pub async fn restore(id: &str, rename: bool) -> Result<()> {
    let body = if rename { json!({ "on_conflict": "rename" }) } else { json!({}) };
    api::post(&format!("/trash/{}/restore", encode(id)), body).await.map(drop)
}

/// Deletes one item of the trash for good.
pub async fn purge(id: &str) -> Result<()> {
    api::del(&format!("/trash/{}", encode(id))).await.map(drop)
}

/// Empties the trash of one source.
pub async fn empty_trash(source: &str) -> Result<()> {
    api::del(&format!("/trash{}", query(&[("source", source)]))).await.map(drop)
}

// ---------- stars and tags ----------

/// A tag on a file.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct TagRef {
    /// Its id.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// `#rrggbb`, when set.
    #[serde(default)]
    pub color: Option<String>,
}

/// The star and tags of one entry of a listing.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Annotation {
    /// Starred by the account.
    #[serde(default)]
    pub starred: bool,
    /// Its tags.
    #[serde(default)]
    pub tags: Vec<TagRef>,
}

/// The stars and tags of a folder's entries, by name.
pub async fn annotations(source: &str, path: &str) -> Result<BTreeMap<String, Annotation>> {
    decode(api::get(&format!("/annotations{}", query(&[("source", source), ("path", path)]))).await?)
}

/// A starred file or folder.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Star {
    /// The source.
    pub source: String,
    /// The path.
    pub path: String,
    /// Its name.
    pub name: String,
    /// When, Unix seconds.
    #[serde(default)]
    pub starred_at: i64,
}

/// The account's stars, newest first.
pub async fn stars() -> Result<Vec<Star>> {
    decode(api::get("/stars").await?)
}

/// Stars or unstars one path.
pub async fn set_star(source: &str, path: &str, on: bool) -> Result<()> {
    if on {
        api::put("/stars", json!({ "source": source, "path": path })).await.map(drop)
    } else {
        api::del(&format!("/stars{}", query(&[("source", source), ("path", path)]))).await.map(drop)
    }
}

/// Every tag (for suggestions).
pub async fn all_tags() -> Result<Vec<TagRef>> {
    decode(api::get("/tags").await?)
}

/// Sets the tags of one path (unknown names are made).
pub async fn set_tags(source: &str, path: &str, tags: &[String]) -> Result<Vec<TagRef>> {
    decode(api::put("/tags/file", json!({ "source": source, "path": path, "tags": tags })).await?)
}

// ---------- text ----------

/// A text file opened in the editor.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct TextFile {
    /// The text.
    pub content: String,
    /// The tag to send back on save.
    pub etag: String,
}

/// What saving answered.
#[derive(Clone, Debug, PartialEq)]
pub enum Saved {
    /// Saved; the new tag.
    Ok(String),
    /// The file changed since it was opened (its tag now, `None` when it is gone).
    Conflict(Option<String>),
}

fn text_url(source: &str, path: &str) -> String {
    format!("/api{}/text{}", source_path(source), query(&[("path", path)]))
}

/// Opens a text file (`not_text` and `payload_too_large` come back as errors).
pub async fn load_text(source: &str, path: &str) -> Result<TextFile> {
    let a = Request::new("GET", text_url(source, path)).send().await?;
    decode(a.json())
}

/// Saves `content` over the version tagged `etag` (`None` makes a new file and fails if one exists).
pub async fn save_text(source: &str, path: &str, content: &str, etag: Option<&str>) -> Result<Saved> {
    let r = Request::new("PUT", text_url(source, path)).header("Content-Type", "text/plain; charset=utf-8").body(content.into());
    let r = match etag {
        Some(t) => r.header("If-Match", t),
        None => r.header("If-None-Match", "*"),
    };
    match r.send().await {
        Ok(a) => Ok(Saved::Ok(a.json().get("etag").and_then(Value::as_str).unwrap_or_default().to_string())),
        Err(e) if e.status == 412 => Ok(Saved::Conflict(e.data.get("etag").and_then(Value::as_str).map(str::to_string))),
        Err(e) => Err(e),
    }
}

// ---------- uploads ----------

/// Starts a chunked upload of `size` bytes to `path` (`overwrite` replaces a file that is there; without it a
/// file in the way answers 409).
pub async fn upload_start(source: &str, path: &str, size: u64, overwrite: bool) -> Result<UploadSession> {
    decode(api::post(&format!("{}/uploads", source_path(source)), json!({ "path": path, "size": size, "overwrite": overwrite })).await?)
}

/// How many bytes of an upload the server holds.
pub async fn upload_status(id: &str) -> Result<UploadSession> {
    decode(api::get(&format!("/uploads/{}", encode(id))).await?)
}

/// Sends one chunk starting at `offset`; `progress` hears the bytes of this chunk sent so far.
pub async fn upload_chunk(id: &str, offset: u64, chunk: web_sys::Blob, progress: Rc<dyn Fn(f64)>, abort: Abort) -> Result<UploadSession> {
    let a = Request::new("PATCH", format!("/api/uploads/{}", encode(id)))
        .header("Upload-Offset", &offset.to_string())
        .header("Content-Type", "application/octet-stream")
        .body(chunk.into())
        .progress(progress)
        .abort(abort)
        .send()
        .await?;
    decode(a.json())
}

/// Cancels an upload; the server drops what it holds.
pub async fn upload_cancel(id: &str) -> Result<()> {
    api::del(&format!("/uploads/{}", encode(id))).await.map(drop)
}

// ---------- casting ----------

/// A short-lived media address a cast device can fetch without the session.
pub async fn cast_url(source: &str, path: &str) -> Result<String> {
    let v = api::post("/cast", json!({ "source": source, "path": path })).await?;
    v.get("url").and_then(Value::as_str).map(str::to_string).ok_or_else(|| Error::new("The server sent no cast address"))
}

// ---------- comments ----------

/// One comment on a file or folder.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Comment {
    /// Its id.
    pub id: i64,
    /// Who wrote it (`None`: the account is gone).
    #[serde(default)]
    pub username: Option<String>,
    /// The text.
    pub body: String,
    /// When, Unix seconds.
    #[serde(default)]
    pub created_at: i64,
    /// When it was last changed.
    #[serde(default)]
    pub edited_at: Option<i64>,
    /// The account may change it.
    #[serde(default)]
    pub can_edit: bool,
    /// The account may delete it.
    #[serde(default)]
    pub can_delete: bool,
}

/// The comments on one path.
pub async fn comments(source: &str, path: &str) -> Result<Vec<Comment>> {
    decode(api::get(&format!("/comments{}", query(&[("source", source), ("path", path)]))).await?)
}

/// Adds a comment.
pub async fn add_comment(source: &str, path: &str, body: &str) -> Result<()> {
    api::post("/comments", json!({ "source": source, "path": path, "body": body })).await.map(drop)
}

/// Changes a comment.
pub async fn edit_comment(id: i64, body: &str) -> Result<()> {
    api::patch(&format!("/comments/{id}"), json!({ "body": body })).await.map(drop)
}

/// Deletes a comment.
pub async fn delete_comment(id: i64) -> Result<()> {
    api::del(&format!("/comments/{id}")).await.map(drop)
}

// ---------- collections ----------

/// A collection in the "Add to collection" list.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct CollectionSummary {
    /// Its id.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// How many items it holds.
    #[serde(default)]
    pub item_count: i64,
}

/// The account's collections.
pub async fn collections() -> Result<Vec<CollectionSummary>> {
    decode(api::get("/collections").await?)
}

/// Adds paths to a collection.
pub async fn add_to_collection(id: i64, source: &str, paths: &[String]) -> Result<()> {
    let items: Vec<Value> = paths.iter().map(|p| json!({ "source": source, "path": p })).collect();
    api::post(&format!("/collections/{id}/items"), json!({ "items": items })).await.map(drop)
}

/// Makes a collection holding paths.
pub async fn new_collection(name: &str, source: &str, paths: &[String]) -> Result<()> {
    let items: Vec<Value> = paths.iter().map(|p| json!({ "source": source, "path": p })).collect();
    api::post("/collections", json!({ "name": name, "items": items })).await.map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_decode() {
        let r: Removed = decode(json!({"trashed": [{"id": "t1", "source": "s", "path": "/a", "name": "a", "kind": "file", "size": 3, "deleted_at": 1, "purge_at": 2}], "deleted": []})).unwrap();
        assert_eq!(r.trashed[0].id, "t1");
        let a: BTreeMap<String, Annotation> =
            decode(json!({"a.txt": {"starred": true, "tags": [{"id": 1, "name": "x", "color": null}]}})).unwrap();
        assert!(a["a.txt"].starred && a["a.txt"].tags[0].name == "x");
        let s: Vec<Star> = decode(json!([{"source": "s", "path": "/a", "name": "a", "starred_at": 5}])).unwrap();
        assert_eq!(s[0].starred_at, 5);
        let c: Vec<Comment> = decode(json!([{"id": 1, "user_id": 2, "username": "x", "body": "hi", "created_at": 1, "edited_at": null, "can_edit": true, "can_delete": true}])).unwrap();
        assert!(c[0].can_edit);
    }

    #[test]
    fn zip_addresses_repeat_the_path() {
        assert_eq!(zip_url("s", &["/a b".into(), "/c".into()]), "/api/sources/s/zip?path=%2Fa%20b&path=%2Fc");
    }
}
