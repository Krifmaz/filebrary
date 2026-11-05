//! The share API (`/api/shares`, `/api/public/shares`), typed the way the server answers it.
//!
//! The scaffold's [`crate::api`] has a first guess at these types; this module follows the server's own shapes
//! (`crates/filebrary/src/shares/model.rs`): ids are numbers, a public entry's `modified` may be `null`, paths
//! inside a share have no leading `/`, and a change (`PATCH`) tells "leave it" apart from "remove it".

use kit_leptos::api::{self, ApiError};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use wasm_bindgen::prelude::*;

/// What a call answers.
pub type Result<T> = std::result::Result<T, ApiError>;

/// The header that carries a password grant for clients without the cookie.
pub const ACCESS_HEADER: &str = "X-Share-Access";

/// A share link, as its owner sees it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Share {
    /// The link's id.
    pub id: i64,
    /// The secret part of its address.
    pub token: String,
    /// Its address on the server (`/s/<token>`).
    #[serde(default)]
    pub url: String,
    /// The account that made it (administrators see everyone's links).
    #[serde(default)]
    pub owner_id: i64,
    /// The source it shares from ("" for a collection link).
    #[serde(default)]
    pub source: String,
    /// The shared paths, as the owner sees them.
    #[serde(default)]
    pub paths: Vec<String>,
    /// When it stops working, RFC 3339.
    #[serde(default)]
    pub expires_at: Option<String>,
    /// How many downloads it allows.
    #[serde(default)]
    pub max_downloads: Option<u64>,
    /// Downloads so far.
    #[serde(default)]
    pub downloads: u64,
    /// A drop link: visitors can only upload.
    #[serde(default)]
    pub upload_only: bool,
    /// Whether it needs a password.
    #[serde(default)]
    pub has_password: bool,
    /// Past its expiry or out of downloads.
    #[serde(default)]
    pub expired: bool,
    /// When it was made, RFC 3339.
    #[serde(default)]
    pub created_at: String,
    /// Set for a link to a whole collection.
    #[serde(default)]
    pub collection_id: Option<i64>,
}

/// What a link shares: files of a source, or a whole collection.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Paths in one source.
    Paths {
        /// The source's id.
        source: String,
        /// The paths.
        paths: Vec<String>,
        /// Exactly one folder (a drop link is possible).
        single_folder: bool,
    },
    /// A collection.
    Collection {
        /// Its id.
        id: i64,
        /// Its name.
        name: String,
    },
}

/// One field of a change: leave it, remove it, or set it.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Change<T> {
    /// Leave it as it is (not sent).
    #[default]
    Keep,
    /// Remove it (`null`).
    Clear,
    /// Set it.
    Set(T),
}

impl<T: serde::Serialize> Change<T> {
    fn put(&self, key: &str, into: &mut Map<String, Value>) {
        match self {
            Change::Keep => {}
            Change::Clear => {
                into.insert(key.into(), Value::Null);
            }
            Change::Set(v) => {
                into.insert(key.into(), json!(v));
            }
        }
    }
}

/// A link's settings, for a new link or a change to one.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Options {
    /// The password.
    pub password: Change<String>,
    /// The expiry, RFC 3339.
    pub expires_at: Change<String>,
    /// The download limit.
    pub max_downloads: Change<u64>,
    /// A drop link (`None`: leave it).
    pub upload_only: Option<bool>,
}

impl Options {
    /// The body of `PATCH /api/shares/<id>`: only what changes; `null` removes.
    pub fn patch_body(&self) -> Value {
        let mut m = Map::new();
        self.password.put("password", &mut m);
        self.expires_at.put("expires_at", &mut m);
        self.max_downloads.put("max_downloads", &mut m);
        if let Some(u) = self.upload_only {
            m.insert("upload_only".into(), json!(u));
        }
        Value::Object(m)
    }

    /// The body of `POST /api/shares` for `target` (a new link has nothing to remove).
    pub fn create_body(&self, target: &Target) -> Value {
        let mut m = Map::new();
        match target {
            Target::Paths { source, paths, single_folder } => {
                m.insert("source".into(), json!(source));
                m.insert("paths".into(), json!(paths));
                m.insert("upload_only".into(), json!(*single_folder && self.upload_only.unwrap_or(false)));
            }
            Target::Collection { id, .. } => {
                m.insert("collection_id".into(), json!(id));
            }
        }
        if let Change::Set(p) = &self.password {
            m.insert("password".into(), json!(p));
        }
        if let Change::Set(e) = &self.expires_at {
            m.insert("expires_at".into(), json!(e));
        }
        if let Change::Set(n) = &self.max_downloads {
            m.insert("max_downloads".into(), json!(n));
        }
        Value::Object(m)
    }
}

/// One file or folder of a shared folder.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PublicEntry {
    /// The name.
    pub name: String,
    /// The path inside the share, without a leading `/`.
    pub path: String,
    /// Whether it is a folder.
    pub is_dir: bool,
    /// Size in bytes (0 for folders).
    #[serde(default)]
    pub size: u64,
    /// Last change, RFC 3339, when known.
    #[serde(default)]
    pub modified: Option<String>,
}

/// What a visitor of a share link sees.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PublicShare {
    /// The shared item's (or the browsed folder's) name; "" while locked.
    #[serde(default)]
    pub name: String,
    /// Whether it is a folder.
    #[serde(default)]
    pub is_dir: bool,
    /// The size of a shared file.
    #[serde(default)]
    pub size: Option<u64>,
    /// A drop link.
    #[serde(default)]
    pub upload_only: bool,
    /// The password is still needed.
    #[serde(default)]
    pub needs_password: bool,
    /// When the link stops working.
    #[serde(default)]
    pub expires_at: Option<String>,
    /// A folder's contents.
    #[serde(default)]
    pub entries: Option<Vec<PublicEntry>>,
}

fn read<T: DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|e| ApiError::new(format!("The server's answer could not be read: {e}")))
}

/// The account's links (an administrator's: everyone's).
pub async fn list() -> Result<Vec<Share>> {
    read(api::get("/shares").await?)
}

/// Makes a link to `target`.
pub async fn create(target: &Target, o: &Options) -> Result<Share> {
    read(api::post("/shares", o.create_body(target)).await?)
}

/// Changes link `id`.
pub async fn update(id: i64, o: &Options) -> Result<Share> {
    read(api::patch(&format!("/shares/{id}"), o.patch_body()).await?)
}

/// Revokes link `id`.
pub async fn revoke(id: i64) -> Result<()> {
    api::del(&format!("/shares/{id}")).await.map(|_| ())
}

thread_local! {
    /// Password grants by token, for browsers that keep no cookie (the cookie is the usual way).
    static GRANTS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// The password grant this page holds for link `token` ("" when none).
pub(super) fn grant(token: &str) -> String {
    GRANTS.with(|g| g.borrow().get(token).cloned()).unwrap_or_default()
}

/// The `?path=&access=` part of a public address.
fn public_query(token: &str, path: &str, inline: bool) -> String {
    let g = grant(token);
    crate::api::query(&[("path", path), ("access", &g), ("inline", if inline { "true" } else { "" })])
}

/// What a visitor sees at `path` (without a leading `/`) of link `token`.
pub async fn open(token: &str, path: &str) -> Result<PublicShare> {
    read(api::get(&format!("/public/shares/{}{}", crate::api::encode(token), public_query(token, path, false))).await?)
}

/// Tries `password` on link `token`; the grant is kept for this page.
pub async fn unlock(token: &str, password: &str) -> Result<()> {
    let v = api::post(&format!("/public/shares/{}/unlock", crate::api::encode(token)), json!({ "password": password })).await?;
    if let Some(a) = v.get("access").and_then(Value::as_str) {
        GRANTS.with(|g| g.borrow_mut().insert(token.to_string(), a.to_string()));
    }
    Ok(())
}

/// The address that downloads `path` of link `token` ("" is all of it, a folder as a zip); `inline` shows a
/// file in the browser instead.
pub fn download_url(token: &str, path: &str, inline: bool) -> String {
    format!("/api/public/shares/{}/download{}", crate::api::encode(token), public_query(token, path, inline))
}

// ---------- raw requests (drop-link chunks, text previews) ----------

#[wasm_bindgen]
extern "C" {
    /// The browser's `XMLHttpRequest`: the one request that reports upload progress.
    #[wasm_bindgen(js_name = XMLHttpRequest)]
    #[derive(Clone)]
    pub type Xhr;
    #[wasm_bindgen(constructor, js_class = "XMLHttpRequest")]
    pub(super) fn new() -> Xhr;
    #[wasm_bindgen(method)]
    pub(super) fn open(this: &Xhr, method: &str, url: &str);
    #[wasm_bindgen(method, js_name = setRequestHeader)]
    pub(super) fn set_request_header(this: &Xhr, k: &str, v: &str);
    #[wasm_bindgen(method)]
    pub(super) fn send(this: &Xhr, body: &web_sys::Blob);
    #[wasm_bindgen(method, js_name = send)]
    pub(super) fn send_text(this: &Xhr, body: &str);
    #[wasm_bindgen(method, js_name = send)]
    pub(super) fn send_nothing(this: &Xhr);
    /// Stops the upload.
    #[wasm_bindgen(method)]
    pub fn abort(this: &Xhr);
    #[wasm_bindgen(method, getter)]
    pub(super) fn status(this: &Xhr) -> u16;
    #[wasm_bindgen(method, getter, js_name = responseText)]
    pub(super) fn response_text(this: &Xhr) -> String;
    #[wasm_bindgen(method, getter)]
    pub(super) fn upload(this: &Xhr) -> XhrUpload;
    #[wasm_bindgen(method, setter)]
    pub(super) fn set_onload(this: &Xhr, f: &js_sys::Function);
    #[wasm_bindgen(method, setter)]
    pub(super) fn set_onerror(this: &Xhr, f: &js_sys::Function);
    #[wasm_bindgen(method, setter)]
    pub(super) fn set_onabort(this: &Xhr, f: &js_sys::Function);

    /// The upload side of a request, which reports progress.
    pub(super) type XhrUpload;
    #[wasm_bindgen(method, setter)]
    pub(super) fn set_onprogress(this: &XhrUpload, f: &js_sys::Function);
}

/// The longest text shown in a preview; longer files are cut there.
pub const TEXT_PREVIEW_BYTES: u64 = 512 * 1024;

/// Reads the text at `url` (a share's inline download) and hands it to `done`, or the sentence for why not. The
/// server forbids framing its answers, so text is shown in the page rather than in a frame.
pub fn fetch_text(url: &str, done: impl FnOnce(std::result::Result<String, String>) + 'static) {
    let x = Xhr::new();
    x.open("GET", url);
    x.set_request_header("Range", &format!("bytes=0-{}", TEXT_PREVIEW_BYTES - 1));
    let xl = x.clone();
    let done = std::rc::Rc::new(RefCell::new(Some(done)));
    let d2 = done.clone();
    let on_load = Closure::once_into_js(move || {
        let (status, body) = (xl.status(), xl.response_text());
        let res = if (200..300).contains(&status) { Ok(body) } else { Err(upload_failure_for_view(status, &body)) };
        if let Some(f) = done.borrow_mut().take() {
            f(res);
        }
    });
    x.set_onload(on_load.unchecked_ref());
    let on_error = Closure::once_into_js(move || {
        if let Some(f) = d2.borrow_mut().take() {
            f(Err(kit_leptos::api::NOT_CONNECTED.to_string()));
        }
    });
    x.set_onerror(on_error.unchecked_ref());
    x.send_nothing();
}

/// What a failed view says.
fn upload_failure_for_view(status: u16, body: &str) -> String {
    match status {
        410 => "This link has reached its download limit.".into(),
        404 => "This file isn't there any more.".into(),
        _ => kit_leptos::api::error_message(&serde_json::from_str(body).unwrap_or(Value::Null), ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_decode_from_the_server() {
        let s: Vec<Share> = serde_json::from_value(json!([{
            "id": 7, "token": "abc", "url": "/s/abc", "owner_id": 1, "source": "home", "paths": ["/Photos"],
            "expires_at": null, "max_downloads": 3, "downloads": 1, "upload_only": false, "has_password": true,
            "expired": false, "created_at": "2026-11-01T10:00:00Z", "collection_id": null
        }]))
        .unwrap();
        assert_eq!((s[0].id, s[0].max_downloads, s[0].has_password), (7, Some(3), true));
        let p: PublicShare = serde_json::from_value(json!({
            "name": "Photos", "is_dir": true, "size": null, "upload_only": false, "needs_password": false,
            "expires_at": null, "entries": [{"name": "a.jpg", "path": "a.jpg", "is_dir": false, "size": 5, "modified": null}]
        }))
        .unwrap();
        assert_eq!(p.entries.unwrap()[0].modified, None);
        let locked: PublicShare = serde_json::from_value(json!({"name": "", "is_dir": false, "size": null, "upload_only": false,
            "needs_password": true, "expires_at": null}))
        .unwrap();
        assert!(locked.needs_password && locked.entries.is_none());
    }

    #[test]
    fn a_change_sends_only_what_changes() {
        let o = Options { password: Change::Clear, max_downloads: Change::Set(5), ..Default::default() };
        assert_eq!(o.patch_body(), json!({"password": null, "max_downloads": 5}));
        assert_eq!(Options::default().patch_body(), json!({}));
        let o = Options { expires_at: Change::Set("2027-01-01T00:00:00Z".into()), upload_only: Some(true), ..Default::default() };
        assert_eq!(o.patch_body(), json!({"expires_at": "2027-01-01T00:00:00Z", "upload_only": true}));
    }

    #[test]
    fn a_new_link_names_its_target() {
        let o = Options { password: Change::Set("pw".into()), upload_only: Some(true), ..Default::default() };
        let folder = Target::Paths { source: "home".into(), paths: vec!["/In".into()], single_folder: true };
        assert_eq!(o.create_body(&folder), json!({"source": "home", "paths": ["/In"], "upload_only": true, "password": "pw"}));
        // a drop link needs exactly one folder: never asked for otherwise
        let files = Target::Paths { source: "home".into(), paths: vec!["/a".into(), "/b".into()], single_folder: false };
        assert_eq!(o.create_body(&files)["upload_only"], json!(false));
        let c = Target::Collection { id: 4, name: "Trip".into() };
        assert_eq!(Options::default().create_body(&c), json!({"collection_id": 4}));
    }
}
