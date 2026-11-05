//! Typed client for Filebrary's REST API (`/api`), on the kit's HTTP backend.
//!
//! Every call goes through [`kit_leptos::api`], so the kit's connection watch (the "server isn't answering"
//! banner), the `X-Requested-With: filebrary` header and copyable error sentences come for free. This module adds
//! what is Filebrary's own:
//!
//! - **CSRF.** `POST /api/auth/login`, `POST /api/setup` and `GET /api/me` answer a `csrf_token`; it is kept in
//!   memory and sent as `X-CSRF-Token` on every call that changes something ([`backend`]). The readable
//!   `filebrary_csrf` cookie is the fallback.
//! - **Types.** The answers as Rust structs, so pages never dig through JSON.
//! - **Error codes.** The server answers `{error: code, message: text}`; [`code`] reads the code
//!   (`totp_required`, `invalid_credentials`, …).
//!
//! Paths here are relative to `/api`, the way the kit's calls are.
//!
//! The client covers the API ahead of the pages that use it (browse, uploads, shares come in their own changes).
#![allow(dead_code)]

use kit_leptos::api::{self, ApiError, HttpBackend};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cell::RefCell;

/// A failed call (the kit's error: status, sentence, whole answer).
pub type Error = ApiError;
/// What a typed call answers.
pub type Result<T> = std::result::Result<T, Error>;

/// The readable cookie that carries the CSRF token.
pub const CSRF_COOKIE: &str = "filebrary_csrf";

thread_local! {
    static CSRF: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Keeps the session's CSRF token for the calls that follow.
pub fn set_csrf(token: Option<String>) {
    CSRF.with(|c| *c.borrow_mut() = token);
}

/// The CSRF token in a `document.cookie` string, if there is one.
pub fn csrf_in_cookie(cookie: &str) -> Option<String> {
    cookie.split(';').filter_map(|p| p.trim().split_once('=')).find(|(k, _)| *k == CSRF_COOKIE).map(|(_, v)| decode(v))
}

/// The headers Filebrary adds to a call: `X-CSRF-Token` on everything but `GET`, `HEAD` and `OPTIONS`.
pub fn headers_for(method: &str, token: Option<&str>) -> Vec<(String, String)> {
    let safe = matches!(method.to_ascii_uppercase().as_str(), "GET" | "HEAD" | "OPTIONS");
    match (safe, token) {
        (false, Some(t)) if !t.is_empty() => vec![("X-CSRF-Token".into(), t.to_string())],
        _ => Vec::new(),
    }
}

fn document_cookie() -> String {
    use wasm_bindgen::JsCast;
    kit_leptos::rt::document().and_then(|d| d.dyn_into::<web_sys::HtmlDocument>().ok()).and_then(|d| d.cookie().ok()).unwrap_or_default()
}

/// The kit backend for Filebrary: the project id `filebrary` and the CSRF header.
pub fn backend() -> HttpBackend {
    HttpBackend::new("filebrary").with_headers(|method, _path| {
        let token = CSRF.with(|c| c.borrow().clone()).or_else(|| csrf_in_cookie(&document_cookie()));
        headers_for(method, token.as_deref())
    })
}

/// The server's machine-readable error code (`totp_required`), "" when there is none.
pub fn code(e: &Error) -> &str {
    e.data.get("error").and_then(Value::as_str).unwrap_or("")
}

fn decode_value<T: DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|e| ApiError::new(format!("The server's answer could not be read: {e}")))
}

async fn get<T: DeserializeOwned>(path: &str) -> Result<T> {
    decode_value(api::get(path).await?)
}

async fn post<T: DeserializeOwned>(path: &str, body: Value) -> Result<T> {
    decode_value(api::post(path, body).await?)
}

/// `/api/sources/<id>` with the id encoded as one path segment.
pub fn source_path(id: &str) -> String {
    format!("/sources/{}", encode(id))
}

/// Percent-encodes a string for one URL component (everything but `A–Z a–z 0–9 - _ . ~`).
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Decodes `%XX` escapes; bad escapes stay as they are (`+` stays a plus: file names may have one).
pub fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(v) => {
                    out.push(v);
                    i += 3;
                    continue;
                }
                None => out.push(b'%'),
            },
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A query string (`?a=1&b=2`) from pairs; pairs with an empty value are left out, "" when none remain.
pub fn query(pairs: &[(&str, &str)]) -> String {
    let q: Vec<String> = pairs.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect();
    if q.is_empty() {
        String::new()
    } else {
        format!("?{}", q.join("&"))
    }
}

// ---------- types ----------

/// An account's role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Manages users, sources and settings.
    Admin,
    /// Works with files as their permissions allow.
    #[default]
    User,
    /// Can look and download, never change.
    ReadOnly,
}

impl Role {
    /// The role's name as people read it.
    pub fn label(self) -> &'static str {
        match self {
            Role::Admin => "Administrator",
            Role::User => "Member",
            Role::ReadOnly => "Read only",
        }
    }
}

/// The signed-in account.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct User {
    /// The account's id.
    pub id: i64,
    /// The sign-in name.
    pub username: String,
    /// Whether the account administers the server.
    #[serde(default)]
    pub is_admin: bool,
    /// The role (older servers leave it out: then it follows `is_admin`).
    #[serde(default)]
    pub role: Option<Role>,
}

impl User {
    /// The account's role, from `role` or, when the server leaves it out, `is_admin`.
    pub fn role(&self) -> Role {
        self.role.unwrap_or(if self.is_admin { Role::Admin } else { Role::User })
    }
}

/// What a sign-in, setup or "who am I" call answers.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Session {
    /// The account.
    pub user: User,
    /// The token to send as `X-CSRF-Token`.
    pub csrf_token: String,
}

/// What the current account may do inside a source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Permissions {
    /// See the files.
    #[serde(default)]
    pub view: bool,
    /// Download them.
    #[serde(default)]
    pub download: bool,
    /// Upload new files.
    #[serde(default)]
    pub upload: bool,
    /// Rename, move and edit.
    #[serde(default)]
    pub edit: bool,
    /// Delete.
    #[serde(default)]
    pub delete: bool,
    /// Make share links.
    #[serde(default)]
    pub share: bool,
}

/// A storage source (a local folder, an S3 bucket, …) the account can reach.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    /// The source's id (a URL segment).
    pub id: String,
    /// Its name as people read it.
    pub name: String,
    /// What the account may do in it.
    #[serde(default)]
    pub permissions: Permissions,
}

/// One file or folder of a listing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// The name.
    pub name: String,
    /// The path inside the source, starting with `/`.
    pub path: String,
    /// Whether it is a folder.
    pub is_dir: bool,
    /// Size in bytes.
    #[serde(default)]
    pub size: u64,
    /// Last change, RFC 3339.
    #[serde(default)]
    pub modified: String,
    /// The media type, when known.
    #[serde(default)]
    pub mime: Option<String>,
}

/// The contents of one folder.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Listing {
    /// The folder's path.
    pub path: String,
    /// What it holds.
    pub entries: Vec<Entry>,
}

/// What the preview service can do (`GET /api/previews`).
#[derive(Clone, Debug, PartialEq, Deserialize, Default)]
pub struct PreviewCapabilities {
    /// Whether previews are on.
    #[serde(default)]
    pub enabled: bool,
    /// Longest side of a thumbnail, in pixels.
    #[serde(default)]
    pub thumb_size: Option<u32>,
    /// Longest side of a large preview, in pixels.
    #[serde(default)]
    pub large_size: Option<u32>,
    /// File kind (`image`, `video`, `pdf`, …) → supported.
    #[serde(default)]
    pub kinds: std::collections::BTreeMap<String, bool>,
    /// Whether video is transcoded.
    #[serde(default)]
    pub transcode: bool,
}

/// A chunked upload on the server.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct UploadSession {
    /// The upload's id.
    pub id: String,
    /// Bytes the server has stored.
    pub offset: u64,
    /// The whole size, when known.
    #[serde(default)]
    pub size: Option<u64>,
}

/// A share link.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Share {
    /// The link's id.
    pub id: String,
    /// The secret part of its address.
    pub token: String,
    /// Its full public address.
    pub url: String,
    /// The source it shares from ("" for a collection link).
    #[serde(default)]
    pub source: String,
    /// The shared paths.
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
    /// Set for a link to a whole collection.
    #[serde(default)]
    pub collection_id: Option<i64>,
    /// When it was made, RFC 3339.
    #[serde(default)]
    pub created_at: String,
}

/// What a new or changed share link asks for.
#[derive(Clone, Debug, PartialEq, Serialize, Default)]
pub struct ShareInput {
    /// The source.
    pub source: String,
    /// The paths to share.
    pub paths: Vec<String>,
    /// A password (`None` leaves it out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// When it stops working, RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// How many downloads it allows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_downloads: Option<u64>,
    /// A drop link.
    pub upload_only: bool,
}

/// What a visitor of a public share link sees.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PublicShare {
    /// The shared item's name.
    pub name: String,
    /// Whether it is a folder.
    pub is_dir: bool,
    /// A drop link.
    #[serde(default)]
    pub upload_only: bool,
    /// Whether the password is still needed.
    #[serde(default)]
    pub needs_password: bool,
    /// The folder's contents.
    #[serde(default)]
    pub entries: Option<Vec<Entry>>,
}

/// The ways to sign in (`GET /api/auth/methods`).
#[derive(Clone, Debug, PartialEq, Deserialize, Default)]
pub struct AuthMethods {
    /// Whether password sign-in is on.
    #[serde(default = "yes")]
    pub password: bool,
    /// Single sign-on, with the button's words.
    #[serde(default)]
    pub oidc: Option<Oidc>,
    /// Whether an LDAP directory is set up.
    #[serde(default)]
    pub ldap: bool,
}

fn yes() -> bool {
    true
}

/// The single sign-on button.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Oidc {
    /// Its words ("Sign in with Authentik").
    pub label: String,
}

// ---------- calls ----------

/// Whether first-run setup is waiting (`GET /api/setup`).
pub async fn setup_needed() -> Result<bool> {
    let v = api::get("/setup").await?;
    Ok(v.get("needed").and_then(Value::as_bool).unwrap_or(false))
}

/// Creates the first administrator with the setup token from the server log, and signs in.
pub async fn setup(token: &str, username: &str, password: &str, remember: bool) -> Result<User> {
    let s: Session = post("/setup", json!({ "token": token, "username": username, "password": password, "remember": remember })).await?;
    set_csrf(Some(s.csrf_token));
    Ok(s.user)
}

/// Signs in; `totp` is the second-factor code when the server asked for one (`totp_required`).
pub async fn login(username: &str, password: &str, totp: Option<&str>, remember: bool) -> Result<User> {
    let mut body = json!({ "username": username, "password": password, "remember": remember });
    if let Some(t) = totp.filter(|t| !t.is_empty()) {
        body["totp"] = json!(t);
    }
    let s: Session = post("/auth/login", body).await?;
    set_csrf(Some(s.csrf_token));
    Ok(s.user)
}

/// Ends the session.
pub async fn logout() -> Result<()> {
    api::post_empty("/auth/logout").await?;
    set_csrf(None);
    Ok(())
}

/// The signed-in account (status 401 when nobody is signed in).
pub async fn me() -> Result<User> {
    let s: Session = get("/me").await?;
    set_csrf(Some(s.csrf_token));
    Ok(s.user)
}

/// The ways to sign in.
pub async fn auth_methods() -> Result<AuthMethods> {
    get("/auth/methods").await
}

/// The address that starts single sign-on and comes back to `return_to`.
pub fn oidc_login_url(return_to: &str) -> String {
    format!("/api/auth/oidc/login{}", query(&[("return_to", return_to)]))
}

/// The sources the account can see.
pub async fn sources() -> Result<Vec<Source>> {
    get("/sources").await
}

/// One folder's contents.
pub async fn list(source: &str, path: &str) -> Result<Listing> {
    get(&format!("{}/list{}", source_path(source), query(&[("path", path)]))).await
}

/// What the preview service can do.
pub async fn preview_capabilities() -> Result<PreviewCapabilities> {
    get("/previews").await
}

/// The account's share links.
pub async fn shares() -> Result<Vec<Share>> {
    get("/shares").await
}

/// Makes a share link.
pub async fn create_share(input: &ShareInput) -> Result<Share> {
    post("/shares", serde_json::to_value(input).unwrap_or_default()).await
}

/// A public share link's contents; `path` lists a subfolder of a folder share.
pub async fn public_share(token: &str, path: &str) -> Result<PublicShare> {
    get(&format!("/public/shares/{}{}", encode(token), query(&[("path", path)]))).await
}

/// The address that downloads one file (`inline` shows it in the browser instead).
pub fn download_url(source: &str, path: &str, inline: bool) -> String {
    format!("/api{}/download{}", source_path(source), query(&[("path", path), ("inline", if inline { "1" } else { "" })]))
}

/// The address of a preview image (`variant` `thumb`, `large` or `pdf`); `version` only busts the cache.
pub fn preview_url(source: &str, path: &str, variant: &str, version: &str) -> String {
    format!("/api{}/preview{}", source_path(source), query(&[("path", path), ("variant", variant), ("v", version)]))
}

/// The address of a share link's download (or one path inside a folder share).
pub fn public_download_url(token: &str, path: &str) -> String {
    format!("/api/public/shares/{}/download{}", encode(token), query(&[("path", path)]))
}

/// A short reason a password can't be used, or `None` when it is fine (the server's own rule: 8 characters).
pub fn password_problem(pw: &str, confirm: Option<&str>) -> Option<&'static str> {
    if pw.chars().count() < 8 {
        return Some("Use at least 8 characters.");
    }
    if confirm.is_some_and(|c| c != pw) {
        return Some("The passwords do not match.");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csrf_goes_only_on_changes() {
        assert!(headers_for("GET", Some("t")).is_empty());
        assert!(headers_for("head", Some("t")).is_empty());
        assert!(headers_for("POST", None).is_empty());
        assert_eq!(headers_for("post", Some("t")), vec![("X-CSRF-Token".to_string(), "t".to_string())]);
        assert_eq!(headers_for("DELETE", Some("abc")).len(), 1);
    }

    #[test]
    fn csrf_cookie_is_read() {
        assert_eq!(csrf_in_cookie("a=1; filebrary_csrf=x%2By; b=2").as_deref(), Some("x+y"));
        assert_eq!(csrf_in_cookie("a=1"), None);
    }

    #[test]
    fn encoding_round_trips() {
        let s = "my files/ünï code & 100%";
        assert_eq!(decode(&encode(s)), s);
        assert_eq!(encode("a b/c"), "a%20b%2Fc");
        assert_eq!(decode("50%"), "50%");
        assert_eq!(decode("%zz"), "%zz");
    }

    #[test]
    fn queries_skip_empty_values() {
        assert_eq!(query(&[]), "");
        assert_eq!(query(&[("path", "/a b"), ("inline", "")]), "?path=%2Fa%20b");
        assert_eq!(download_url("s 1", "/x", true), "/api/sources/s%201/download?path=%2Fx&inline=1");
    }

    #[test]
    fn users_decode_from_old_and_new_servers() {
        let new: Session =
            serde_json::from_value(json!({"user": {"id": 1, "username": "a", "is_admin": false, "role": "read_only"}, "csrf_token": "t"}))
                .unwrap();
        assert_eq!(new.user.role(), Role::ReadOnly);
        let old: User = serde_json::from_value(json!({"id": 2, "username": "b", "is_admin": true})).unwrap();
        assert_eq!(old.role(), Role::Admin);
        assert_eq!(Role::User.label(), "Member");
    }

    #[test]
    fn sources_and_listings_decode() {
        let s: Vec<Source> =
            serde_json::from_value(json!([{"id": "home", "name": "Home", "permissions": {"view": true, "upload": true}}])).unwrap();
        assert!(s[0].permissions.upload && !s[0].permissions.delete);
        let l: Listing = serde_json::from_value(
            json!({"path": "/", "entries": [{"name": "a", "path": "/a", "is_dir": true, "size": 0, "modified": "2026-01-01T00:00:00Z"}]}),
        )
        .unwrap();
        assert!(l.entries[0].is_dir);
        let m: AuthMethods = serde_json::from_value(json!({"oidc": {"label": "SSO"}})).unwrap();
        assert!(m.password && m.oidc.is_some());
    }

    #[test]
    fn passwords_follow_the_server_rule() {
        assert_eq!(password_problem("short", None), Some("Use at least 8 characters."));
        assert_eq!(password_problem("longenough", Some("other")), Some("The passwords do not match."));
        assert_eq!(password_problem("longenough", Some("longenough")), None);
    }
}
