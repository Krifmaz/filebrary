//! The calls and types of the settings and admin pages: the account's own settings (`/api/me/…`), users,
//! groups, permission rules, quotas, jobs, the search index, branding and collections.
//!
//! The calls go through the kit's API client like the rest of the app ([`kit_leptos::api`]), so the CSRF
//! header, the connection watch and copyable error sentences come with them. The pure helpers (sizes,
//! permissions, passwords, dates) sit at the bottom with their tests.

use crate::api::{encode, query, Error, Result, Role, Share};
use kit_leptos::api;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use wasm_bindgen::JsCast;

fn read<T: DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|e| Error::new(format!("The server's answer could not be read: {e}")))
}

async fn get<T: DeserializeOwned>(path: &str) -> Result<T> {
    read(api::get(path).await?)
}

async fn post<T: DeserializeOwned>(path: &str, body: Value) -> Result<T> {
    read(api::post(path, body).await?)
}

async fn put<T: DeserializeOwned>(path: &str, body: Value) -> Result<T> {
    read(api::put(path, body).await?)
}

// ---------- types ----------

/// The six permissions, in the order people read them.
pub const PERMS: [&str; 6] = ["view", "download", "upload", "edit", "delete", "share"];

/// An account as administrators see it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Account {
    /// The id.
    pub id: i64,
    /// The sign-in name.
    pub username: String,
    /// Where it signs in: `local` (password), `oidc`, `ldap`, …
    #[serde(default = "local")]
    pub auth_source: String,
    /// The role.
    #[serde(default)]
    pub role: Role,
    /// Whether it can't sign in.
    #[serde(default)]
    pub disabled: bool,
    /// When it was made, Unix seconds.
    #[serde(default)]
    pub created_at: i64,
}

fn local() -> String {
    "local".into()
}

/// A group of accounts.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Group {
    /// The id.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// When it was made, Unix seconds.
    #[serde(default)]
    pub created_at: i64,
}

/// Who a rule applies to: `{"type": "user", "id": 3}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "type", content = "id")]
pub enum Subject {
    /// One account.
    User(i64),
    /// Every member of a group.
    Group(i64),
}

/// A permission rule: `perms` on `path` (and everything below it) of `source`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Rule {
    /// The id.
    pub id: i64,
    /// The source's id.
    pub source: String,
    /// The folder, `/` for the whole source.
    pub path: String,
    /// Who it is for.
    pub subject: Subject,
    /// The permissions it grants.
    #[serde(default)]
    pub perms: Vec<String>,
}

/// A storage quota in bytes (`limit` `None`: no limit).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct Quota {
    /// The limit.
    #[serde(default)]
    pub limit: Option<i64>,
    /// In use.
    #[serde(default)]
    pub used: i64,
}

/// Two-factor sign-in's state.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct Totp {
    /// Whether it is on.
    pub enabled: bool,
    /// Recovery codes not used yet.
    #[serde(default)]
    pub recovery_codes_left: i64,
}

/// What two-factor setup hands out: the secret and the `otpauth://` address for the app.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct TotpSetup {
    /// The secret (base32).
    pub secret: String,
    /// The `otpauth://` address.
    pub uri: String,
}

/// An API key (its secret only comes once, in [`NewApiKey`]).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ApiKey {
    /// The id.
    pub id: String,
    /// Its name.
    pub name: String,
    /// What it may do.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Unix seconds.
    #[serde(default)]
    pub created_at: i64,
    /// Unix seconds, `None` for never.
    #[serde(default)]
    pub expires_at: Option<i64>,
    /// Unix seconds, `None` for never.
    #[serde(default)]
    pub last_used_at: Option<i64>,
}

/// A key just made, with its secret.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct NewApiKey {
    /// Its name.
    pub name: String,
    /// The secret, shown once.
    pub token: String,
}

/// What an API key can be allowed to do: the scope and its words.
pub const KEY_SCOPES: [(&str, &str); 5] = [
    ("files:read", "Read files"),
    ("files:write", "Upload and edit files"),
    ("files:delete", "Delete files"),
    ("shares", "Manage share links"),
    ("admin", "Administration"),
];

/// How folders open (the files page's view modes): id and words.
pub const VIEW_MODES: [(&str, &str); 5] =
    [("details", "Details"), ("compact", "Compact"), ("small", "Small tiles"), ("medium", "Medium tiles"), ("large", "Large tiles")];

/// A background job.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Job {
    /// The id.
    pub id: i64,
    /// Its kind (`convert.video`).
    pub kind: String,
    /// What it does, in words.
    #[serde(default)]
    pub title: String,
    /// Whose it is (`None`: the server's own).
    #[serde(default)]
    pub user_id: Option<i64>,
    /// `queued`, `running`, `succeeded`, `failed` or `cancelled`.
    pub status: String,
    /// 0 to 1.
    #[serde(default)]
    pub progress: f64,
    /// The latest progress line.
    #[serde(default)]
    pub message: Option<String>,
    /// Why it failed.
    #[serde(default)]
    pub error: Option<String>,
    /// Times started.
    #[serde(default)]
    pub attempts: i64,
    /// Unix seconds.
    #[serde(default)]
    pub created_at: i64,
    /// Unix seconds.
    #[serde(default)]
    pub finished_at: Option<i64>,
}

/// The search index of one source.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct IndexStatus {
    /// The source's id.
    pub source: String,
    /// Files and folders in the index.
    #[serde(default)]
    pub entries: i64,
    /// A full scan is running.
    #[serde(default)]
    pub scanning: bool,
    /// A watcher keeps it fresh.
    #[serde(default)]
    pub watching: bool,
    /// When the last full scan ended, Unix seconds.
    #[serde(default)]
    pub last_scan: Option<i64>,
    /// How long it took, milliseconds.
    #[serde(default)]
    pub last_scan_ms: Option<u64>,
    /// The last scan or watcher error.
    #[serde(default)]
    pub last_error: Option<String>,
}

/// The server-wide branding. The colour palettes are kept as the server has them (the kit themes draw the
/// new UI; the palettes still style the React UI until cutover).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Branding {
    /// The name people see.
    #[serde(default)]
    pub name: String,
    /// The theme for accounts that have not picked one: `system`, `light`, `dark` or `custom`.
    #[serde(default)]
    pub default_theme: String,
    /// Light theme colours.
    #[serde(default)]
    pub light: Value,
    /// Dark theme colours.
    #[serde(default)]
    pub dark: Value,
    /// Custom theme colours.
    #[serde(default)]
    pub custom: Value,
    /// The logo's address, if there is one.
    #[serde(default, skip_serializing)]
    pub logo_url: Option<String>,
}

/// The default themes branding can name, with their words.
pub const BRAND_THEMES: [(&str, &str); 4] =
    [("system", "Follow the device"), ("light", "Light"), ("dark", "Dark"), ("custom", "Custom (React UI only)")];

/// A collection in the list.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct CollectionSummary {
    /// The id.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// How many items it holds.
    #[serde(default)]
    pub item_count: i64,
}

/// One item of a collection.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct CollectionItem {
    /// The id.
    pub id: i64,
    /// The source's id.
    pub source: String,
    /// The path the owner sees, `None` when it left their reach.
    #[serde(default)]
    pub path: Option<String>,
    /// Its name.
    pub name: String,
    /// `file` or `dir`, `None` when it is missing.
    #[serde(default)]
    pub kind: Option<String>,
    /// Bytes.
    #[serde(default)]
    pub size: Option<u64>,
}

/// A collection with its items.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Collection {
    /// The id.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// The items.
    #[serde(default)]
    pub items: Vec<CollectionItem>,
}

// ---------- the account's own settings ----------

/// Changes the signed-in account's password (its other sessions end).
pub async fn change_password(current: &str, new: &str) -> Result<()> {
    api::post("/me/password", json!({ "current": current, "new": new })).await.map(drop)
}

/// The signed-in account's quota.
pub async fn my_quota() -> Result<Quota> {
    get("/me/quota").await
}

/// The default view mode for folders.
pub async fn view_mode() -> Result<String> {
    let v = api::get("/me/preferences").await?;
    Ok(v.get("view_mode").and_then(Value::as_str).unwrap_or("details").to_string())
}

/// Saves the default view mode.
pub async fn set_view_mode(mode: &str) -> Result<()> {
    api::put("/me/preferences", json!({ "view_mode": mode })).await.map(drop)
}

/// Two-factor sign-in's state.
pub async fn totp() -> Result<Totp> {
    get("/me/totp").await
}

/// Starts two-factor setup.
pub async fn totp_setup() -> Result<TotpSetup> {
    post("/me/totp/setup", json!({})).await
}

fn codes(v: Value) -> Vec<String> {
    v.get("recovery_codes")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// Turns two-factor sign-in on with a code from the app; answers the recovery codes.
pub async fn totp_confirm(code: &str) -> Result<Vec<String>> {
    Ok(codes(api::post("/me/totp/confirm", json!({ "code": code })).await?))
}

/// New recovery codes (the old ones stop working).
pub async fn totp_recovery(code: &str) -> Result<Vec<String>> {
    Ok(codes(api::post("/me/totp/recovery-codes", json!({ "code": code })).await?))
}

/// Turns two-factor sign-in off.
pub async fn totp_disable(code: &str) -> Result<()> {
    api::post("/me/totp/disable", json!({ "code": code })).await.map(drop)
}

/// The account's API keys.
pub async fn api_keys() -> Result<Vec<ApiKey>> {
    get("/me/api-keys").await
}

/// Makes an API key (`days`: it expires after that many days).
pub async fn create_api_key(name: &str, scopes: &[String], days: Option<u32>) -> Result<NewApiKey> {
    let mut body = json!({ "name": name, "scopes": scopes });
    if let Some(d) = days {
        body["expires_in_days"] = json!(d);
    }
    post("/me/api-keys", body).await
}

/// Revokes one of the account's keys.
pub async fn revoke_api_key(id: &str) -> Result<()> {
    api::del(&format!("/me/api-keys/{}", encode(id))).await.map(drop)
}

// ---------- users, groups and rules (administrators) ----------

/// Every account.
pub async fn users() -> Result<Vec<Account>> {
    get("/users").await
}

/// Makes an account; `home` gives it a home folder (source, path) at the same time.
pub async fn create_user(username: &str, password: &str, role: Role, home: Option<(&str, &str)>) -> Result<Account> {
    let mut body = json!({ "username": username, "password": password, "role": role });
    if let Some((source, path)) = home {
        body["home"] = if path.trim().is_empty() { json!({ "source": source }) } else { json!({ "source": source, "path": path.trim() }) };
    }
    post("/users", body).await
}

/// Changes an account's role and whether it is disabled.
pub async fn update_user(id: i64, role: Role, disabled: bool) -> Result<Account> {
    put(&format!("/users/{id}"), json!({ "role": role, "disabled": disabled })).await
}

/// Sets a new password for an account (its sessions end).
pub async fn reset_password(id: i64, password: &str) -> Result<()> {
    api::put(&format!("/users/{id}"), json!({ "password": password })).await.map(drop)
}

/// Deletes an account (its files stay).
pub async fn delete_user(id: i64) -> Result<()> {
    api::del(&format!("/users/{id}")).await.map(drop)
}

/// An account's home folder per source (`/`: the whole source).
pub async fn scopes(id: i64) -> Result<std::collections::BTreeMap<String, String>> {
    get(&format!("/users/{id}/scopes")).await
}

/// Puts an account's home folder at `path` on `source` (made if missing, with every permission there);
/// answers the folder.
pub async fn set_home(id: i64, source: &str, path: &str) -> Result<String> {
    let v = api::put(&format!("/users/{id}/home"), json!({ "source": source, "path": path })).await?;
    Ok(v.get("path").and_then(Value::as_str).unwrap_or(path).to_string())
}

/// Sets an account's scope on a source (`/` clears it).
pub async fn set_scope(id: i64, source: &str, scope: &str) -> Result<()> {
    api::put(&format!("/users/{id}/scopes"), json!({ "source": source, "scope": scope })).await.map(drop)
}

/// An account's quota.
pub async fn quota(id: i64) -> Result<Quota> {
    get(&format!("/users/{id}/quota")).await
}

/// Sets an account's quota in bytes (`None`: no limit).
pub async fn set_quota(id: i64, limit: Option<i64>) -> Result<Quota> {
    put(&format!("/users/{id}/quota"), json!({ "limit": limit })).await
}

/// Counts an account's storage again.
pub async fn rescan_quota(id: i64) -> Result<()> {
    api::post_empty(&format!("/users/{id}/quota/rescan")).await.map(drop)
}

/// Turns two-factor sign-in off for an account that lost its device.
pub async fn reset_totp(id: i64) -> Result<()> {
    api::del(&format!("/users/{id}/totp")).await.map(drop)
}

/// The groups an account is in.
pub async fn user_groups(id: i64) -> Result<Vec<Group>> {
    get(&format!("/users/{id}/groups")).await
}

/// Every group.
pub async fn groups() -> Result<Vec<Group>> {
    get("/groups").await
}

/// Makes a group.
pub async fn create_group(name: &str) -> Result<Group> {
    post("/groups", json!({ "name": name })).await
}

/// Deletes a group (its rules go too).
pub async fn delete_group(id: i64) -> Result<()> {
    api::del(&format!("/groups/{id}")).await.map(drop)
}

/// A group's members.
pub async fn members(id: i64) -> Result<Vec<Account>> {
    get(&format!("/groups/{id}/members")).await
}

/// Puts an account in a group.
pub async fn add_member(group: i64, user: i64) -> Result<()> {
    api::put(&format!("/groups/{group}/members/{user}"), json!({})).await.map(drop)
}

/// Takes an account out of a group.
pub async fn remove_member(group: i64, user: i64) -> Result<()> {
    api::del(&format!("/groups/{group}/members/{user}")).await.map(drop)
}

/// Every permission rule.
pub async fn rules() -> Result<Vec<Rule>> {
    get("/permissions").await
}

/// Makes or replaces the rule for `subject` on `path` of `source`.
pub async fn set_rule(source: &str, path: &str, subject: Subject, perms: &[String]) -> Result<Rule> {
    put("/permissions", json!({ "source": source, "path": path, "subject": subject, "perms": perms })).await
}

/// Deletes a rule.
pub async fn delete_rule(id: i64) -> Result<()> {
    api::del(&format!("/permissions/{id}")).await.map(drop)
}

// ---------- jobs, search index, branding ----------

/// Everyone's jobs, newest first (`status` "" for all).
pub async fn jobs(status: &str) -> Result<Vec<Job>> {
    get(&format!("/jobs{}", query(&[("all", "true"), ("status", status), ("limit", "200")]))).await
}

/// Cancels a queued or running job.
pub async fn cancel_job(id: i64) -> Result<()> {
    api::post_empty(&format!("/jobs/{id}/cancel")).await.map(drop)
}

/// Queues a failed or cancelled job again.
pub async fn retry_job(id: i64) -> Result<()> {
    api::post_empty(&format!("/jobs/{id}/retry")).await.map(drop)
}

/// Takes a finished job off the list.
pub async fn delete_job(id: i64) -> Result<()> {
    api::del(&format!("/jobs/{id}")).await.map(drop)
}

/// The search index per source.
pub async fn index_status() -> Result<Vec<IndexStatus>> {
    get("/search/status").await
}

/// Scans a source again (`None`: every source).
pub async fn reindex(source: Option<&str>) -> Result<()> {
    api::post("/search/reindex", source.map(|s| json!({ "source": s })).unwrap_or_else(|| json!({}))).await.map(drop)
}

/// The branding.
pub async fn branding() -> Result<Branding> {
    get("/branding").await
}

/// Saves the branding (name, default theme, palettes as they were).
pub async fn save_branding(b: &Branding) -> Result<Branding> {
    put("/admin/branding", serde_json::to_value(b).unwrap_or_default()).await
}

/// Takes the logo away.
pub async fn delete_logo() -> Result<()> {
    api::del("/admin/branding/logo").await.map(drop)
}

/// Uploads a logo, then calls `done`: the image itself is the body of `PUT /api/admin/branding/logo` (the kit's
/// client only sends JSON and forms, so this one call uses `fetch` directly, with the same headers; the CSRF
/// token comes fresh from `GET /api/me`).
pub fn upload_logo(file: web_sys::File, done: impl FnOnce(Result<()>) + 'static) {
    kit_leptos::rt::spawn(async move {
        match api::get("/me").await {
            Ok(me) => send_logo(file, me.get("csrf_token").and_then(Value::as_str).unwrap_or_default(), done),
            Err(e) => done(Err(e)),
        }
    });
}

fn send_logo(file: web_sys::File, csrf: &str, done: impl FnOnce(Result<()>) + 'static) {
    use js_sys::{Function, Object, Reflect};
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsValue;
    let Some(win) = kit_leptos::rt::window() else { return done(Err(Error::new("No browser window"))) };
    let set = |o: &Object, k: &str, v: &JsValue| {
        let _ = Reflect::set(o, &k.into(), v);
    };
    let headers = Object::new();
    set(&headers, "X-Requested-With", &crate::project().into());
    set(&headers, "Content-Type", &file.type_().into());
    for (k, v) in crate::api::headers_for("PUT", Some(csrf)) {
        set(&headers, &k, &v.into());
    }
    let init = Object::new();
    set(&init, "method", &"PUT".into());
    set(&init, "credentials", &"same-origin".into());
    set(&init, "headers", &headers);
    set(&init, "body", &file.into());
    let started = Reflect::get(&win, &"fetch".into())
        .ok()
        .and_then(|f| f.dyn_into::<Function>().ok())
        .and_then(|f| f.call2(&win, &"/api/admin/branding/logo".into(), &init).ok());
    let Some(p) = started else { return done(Err(Error::new("The upload could not start"))) };
    let done = std::rc::Rc::new(std::cell::RefCell::new(Some(Box::new(done) as Box<dyn FnOnce(Result<()>)>)));
    let d2 = done.clone();
    let ok = Closure::once_into_js(move |res: JsValue| {
        let status = Reflect::get(&res, &"status".into()).ok().and_then(|s| s.as_f64()).unwrap_or(0.0) as u16;
        if let Some(f) = done.borrow_mut().take() {
            f(logo_answer(status));
        }
    });
    let failed = Closure::once_into_js(move |_: JsValue| {
        if let Some(f) = d2.borrow_mut().take() {
            f(Err(Error::new(api::NOT_CONNECTED)));
        }
    });
    if let Some(then) = Reflect::get(&p, &"then".into()).ok().and_then(|f| f.dyn_into::<Function>().ok()) {
        let _ = then.call2(&p, &ok, &failed);
    }
}

/// What a logo upload's status says.
pub fn logo_answer(status: u16) -> Result<()> {
    match status {
        200..=299 => Ok(()),
        413 => Err(Error::new("The logo is too big. Use an image under 512 KB.")),
        400 | 415 => Err(Error::new("The logo must be a PNG, JPEG, WebP, GIF or SVG image.")),
        s => Err(Error::new(format!("The logo could not be saved (HTTP {s})."))),
    }
}

// ---------- collections ----------

/// The account's collections.
pub async fn collections() -> Result<Vec<CollectionSummary>> {
    get("/collections").await
}

/// One collection with its items.
pub async fn collection(id: i64) -> Result<Collection> {
    get(&format!("/collections/{id}")).await
}

/// Makes an empty collection.
pub async fn create_collection(name: &str) -> Result<Collection> {
    post("/collections", json!({ "name": name, "items": [] })).await
}

/// Renames a collection.
pub async fn rename_collection(id: i64, name: &str) -> Result<()> {
    api::patch(&format!("/collections/{id}"), json!({ "name": name })).await.map(drop)
}

/// Deletes a collection (the files stay).
pub async fn delete_collection(id: i64) -> Result<()> {
    api::del(&format!("/collections/{id}")).await.map(drop)
}

/// Takes one item out of a collection.
pub async fn remove_item(id: i64, item: i64) -> Result<()> {
    api::del(&format!("/collections/{id}/items/{item}")).await.map(drop)
}

/// A share link for a whole collection (`expires_at` RFC 3339).
pub async fn share_collection(id: i64, password: &str, expires_at: Option<String>, max_downloads: Option<u64>) -> Result<Share> {
    let mut body = json!({ "collection_id": id, "expires_at": expires_at, "max_downloads": max_downloads });
    if !password.is_empty() {
        body["password"] = json!(password);
    }
    post("/shares", body).await
}

// ---------- helpers ----------

/// Bytes as people read them (`1.5 GB`), binary units like the server's sizes.
pub fn format_size(bytes: i64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = bytes.max(0) as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} B", bytes.max(0))
    } else if v >= 100.0 || (v - v.round()).abs() < 0.05 {
        format!("{:.0} {}", v, UNITS[i])
    } else {
        format!("{:.1} {}", v, UNITS[i])
    }
}

/// A size such as `10G`, `500 MB` or `1048576` in bytes: `Ok(None)` for empty (no limit), `Err` when it
/// can't be read.
pub fn parse_size(s: &str) -> std::result::Result<Option<i64>, &'static str> {
    const BAD: &str = "Use a size such as 500M, 10G or 1T, or leave it empty for no limit.";
    let t = s.trim().to_ascii_lowercase().replace(' ', "");
    if t.is_empty() {
        return Ok(None);
    }
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let n: f64 = num.parse().map_err(|_| BAD)?;
    let unit = unit.trim_end_matches('b').trim_end_matches('i');
    let pow = match unit {
        "" => 0,
        "k" => 1,
        "m" => 2,
        "g" => 3,
        "t" => 4,
        "p" => 5,
        _ => return Err(BAD),
    };
    Ok(Some((n * 1024f64.powi(pow)).round() as i64))
}

/// A quota limit as the box shows it (`10G`), "" for none.
pub fn size_box(limit: Option<i64>) -> String {
    let Some(b) = limit else { return String::new() };
    for (pow, u) in [(5, "P"), (4, "T"), (3, "G"), (2, "M"), (1, "K")] {
        let unit = 1024i64.pow(pow);
        if b >= unit && b % unit == 0 {
            return format!("{}{u}", b / unit);
        }
    }
    let g = b as f64 / 1024f64.powi(3);
    if g >= 1.0 {
        format!("{g:.1}G")
    } else {
        b.to_string()
    }
}

/// Turns `p` on or off and keeps the set sensible: anything implies view, and taking view away takes
/// everything; the answer is in [`PERMS`] order.
pub fn toggle_perm(perms: &[String], p: &str, on: bool) -> Vec<String> {
    let mut set: Vec<&str> = perms.iter().map(String::as_str).collect();
    if on {
        set.push(p);
        set.push("view");
    } else if p == "view" {
        set.clear();
    } else {
        set.retain(|x| *x != p);
    }
    PERMS.iter().filter(|x| set.contains(x)).map(|x| x.to_string()).collect()
}

/// A rule's permissions in words: "No access", "Full access" or the list.
pub fn perms_words(perms: &[String]) -> String {
    match perms.len() {
        0 => "No access".into(),
        n if n == PERMS.len() => "Full access".into(),
        _ => perms.join(", "),
    }
}

/// What a role allows, in one line.
pub fn role_hint(r: Role) -> &'static str {
    match r {
        Role::Admin => "Every permission on every source; manages users and settings.",
        Role::User => "Can do what their permissions allow.",
        Role::ReadOnly => "Can view and download what their permissions allow, nothing more.",
    }
}

/// The roles in the order the picker shows them, with their ids.
pub const ROLES: [(Role, &str); 3] = [(Role::User, "user"), (Role::ReadOnly, "read_only"), (Role::Admin, "admin")];

/// The role of a picker id.
pub fn role_of(id: &str) -> Role {
    ROLES.iter().find(|(_, i)| *i == id).map(|(r, _)| *r).unwrap_or_default()
}

/// The picker id of a role.
#[cfg(test)]
pub fn role_id(r: Role) -> &'static str {
    ROLES.iter().find(|(x, _)| *x == r).map(|(_, i)| *i).unwrap_or("user")
}

/// The letters a made-up password uses (no look-alikes such as `0`/`O`, `1`/`l`).
pub const PASSWORD_LETTERS: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// A password from random numbers (`n` of them; each picks one of [`PASSWORD_LETTERS`]).
pub fn password_from(random: &[u32]) -> String {
    random.iter().map(|r| PASSWORD_LETTERS[*r as usize % PASSWORD_LETTERS.len()] as char).collect()
}

/// A random, readable 16-letter password, from the browser's `crypto.getRandomValues`.
pub fn generate_password() -> String {
    use js_sys::{Function, Reflect, Uint32Array};
    let buf = Uint32Array::new_with_length(16);
    let filled = kit_leptos::rt::window()
        .and_then(|w| Reflect::get(&w, &"crypto".into()).ok())
        .and_then(|c| Reflect::get(&c, &"getRandomValues".into()).ok().and_then(|f| f.dyn_into::<Function>().ok()).map(|f| (c, f)))
        .is_some_and(|(c, f)| f.call1(&c, &buf).is_ok());
    if !filled {
        // no Web Crypto: fall back to Math.random (still never sent anywhere but the form)
        for i in 0..16 {
            buf.set_index(i, (js_sys::Math::random() * u32::MAX as f64) as u32);
        }
    }
    password_from(&buf.to_vec())
}

/// A Unix time as a local date ("never" for `None`).
pub fn date(secs: Option<i64>) -> String {
    match secs {
        Some(s) if s > 0 => js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(s as f64 * 1000.0))
            .to_locale_date_string("default", &wasm_bindgen::JsValue::UNDEFINED)
            .as_string()
            .unwrap_or_default(),
        _ => "never".into(),
    }
}

/// A Unix time as a local date and time.
pub fn date_time(secs: i64) -> String {
    js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(secs as f64 * 1000.0))
        .to_locale_string("default", &wasm_bindgen::JsValue::UNDEFINED)
        .as_string()
        .unwrap_or_default()
}

/// A `datetime-local` value (`2026-11-05T14:30`, local time) as RFC 3339, `None` when empty or unreadable.
pub fn local_to_rfc3339(v: &str) -> Option<String> {
    if v.trim().is_empty() {
        return None;
    }
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(v));
    let ms = d.get_time();
    (!ms.is_nan()).then(|| d.to_iso_string().as_string().unwrap_or_default())
}

/// The secret of a two-factor setup in groups of four letters, easier to type.
pub fn grouped(secret: &str) -> String {
    secret.chars().collect::<Vec<_>>().chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_both_ways() {
        assert_eq!(parse_size(""), Ok(None));
        assert_eq!(parse_size("10G"), Ok(Some(10 * 1024 * 1024 * 1024)));
        assert_eq!(parse_size("500 MB"), Ok(Some(500 * 1024 * 1024)));
        assert_eq!(parse_size("1.5gib"), Ok(Some(1610612736)));
        assert_eq!(parse_size("2048"), Ok(Some(2048)));
        assert!(parse_size("lots").is_err());
        assert!(parse_size("5x").is_err());
        assert_eq!(size_box(Some(10 * 1024 * 1024 * 1024)), "10G");
        assert_eq!(size_box(Some(512 * 1024 * 1024)), "512M");
        assert_eq!(size_box(None), "");
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(10 * 1024 * 1024 * 1024), "10 GB");
    }

    #[test]
    fn permissions_stay_sensible() {
        let none: Vec<String> = vec![];
        assert_eq!(toggle_perm(&none, "upload", true), ["view", "upload"]);
        let some = toggle_perm(&none, "share", true);
        assert_eq!(toggle_perm(&some, "view", false), Vec::<String>::new());
        assert_eq!(toggle_perm(&some, "share", false), ["view"]);
        let all: Vec<String> = PERMS.iter().map(|s| s.to_string()).collect();
        assert_eq!(perms_words(&all), "Full access");
        assert_eq!(perms_words(&none), "No access");
        assert_eq!(perms_words(&some), "view, share");
    }

    #[test]
    fn rules_and_accounts_decode() {
        let r: Rule = serde_json::from_value(
            json!({"id": 1, "source": "home", "path": "/a", "subject": {"type": "group", "id": 4}, "perms": ["view"]}),
        )
        .unwrap();
        assert_eq!(r.subject, Subject::Group(4));
        assert_eq!(serde_json::to_value(Subject::User(3)).unwrap(), json!({"type": "user", "id": 3}));
        let a: Account = serde_json::from_value(json!({"id": 2, "username": "bo", "auth_source": "oidc", "role": "read_only", "is_admin": false, "read_only": true, "disabled": true, "created_at": 5})).unwrap();
        assert_eq!((a.role, a.disabled, a.auth_source.as_str()), (Role::ReadOnly, true, "oidc"));
        assert_eq!(role_of(role_id(Role::Admin)), Role::Admin);
    }

    #[test]
    fn branding_keeps_its_palettes_and_leaves_the_logo_out() {
        let b: Branding = serde_json::from_value(json!({"name": "Files", "default_theme": "dark", "light": {"--bg": "#fff"}, "dark": {}, "custom": {}, "logo_url": "/api/branding/logo?v=2"})).unwrap();
        let back = serde_json::to_value(&b).unwrap();
        assert_eq!(back["light"], json!({"--bg": "#fff"}));
        assert!(back.get("logo_url").is_none(), "the server refuses unknown fields");
    }

    #[test]
    fn made_up_passwords_avoid_look_alikes() {
        let p = password_from(&[0, 1, 2, 55, 56, 1000]);
        assert_eq!(p.len(), 6);
        assert!(!p.contains(['0', 'O', '1', 'l', 'I']));
        assert_eq!(grouped("ABCDEFGHIJ"), "ABCD EFGH IJ");
    }
}
