//! JSON shapes of the share API, shared by the management and public routes.

use serde::{Deserialize, Deserializer, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::{error::ShareError, store::Share};

/// Longest password accepted, to keep argon2 work bounded.
pub const MAX_PASSWORD_LEN: usize = 1024;
/// Most paths a single share may hold.
pub const MAX_PATHS: usize = 100;

/// Body of `POST /api/shares`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateShareRequest {
    /// Source id the paths belong to.
    #[serde(default)]
    pub source: String,
    /// One or more paths to share (as the caller sees them).
    #[serde(default)]
    pub paths: Vec<String>,
    /// Optional password visitors must enter.
    #[serde(default)]
    pub password: Option<String>,
    /// Optional RFC 3339 expiry time.
    #[serde(default)]
    pub expires_at: Option<String>,
    /// Optional download limit (at least 1).
    #[serde(default)]
    pub max_downloads: Option<i64>,
    /// Make this an upload-only drop link (requires a single folder).
    #[serde(default)]
    pub upload_only: bool,
    /// Share a whole collection instead of `source`/`paths` (which must then
    /// be left out).
    #[serde(default)]
    pub collection_id: Option<i64>,
}

/// Body of `PATCH /api/shares/{id}`. Absent fields stay as they are; `null`
/// clears password, expiry or download limit.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateShareRequest {
    /// New password, or `null` to remove it.
    #[serde(default, deserialize_with = "double_option")]
    pub password: Option<Option<String>>,
    /// New RFC 3339 expiry, or `null` to remove it.
    #[serde(default, deserialize_with = "double_option")]
    pub expires_at: Option<Option<String>>,
    /// New download limit, or `null` for unlimited.
    #[serde(default, deserialize_with = "double_option")]
    pub max_downloads: Option<Option<i64>>,
    /// Switch upload-only on or off.
    #[serde(default)]
    pub upload_only: Option<bool>,
}

/// Distinguishes a missing field (`None`) from an explicit `null` (`Some(None)`).
fn double_option<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(de).map(Some)
}

/// A share as returned by the management API.
#[derive(Debug, Serialize, PartialEq)]
pub struct ShareResponse {
    /// Database id.
    pub id: i64,
    /// Public token.
    pub token: String,
    /// Relative URL of the share page (`/s/{token}`).
    pub url: String,
    /// Owning user id.
    pub owner_id: i64,
    /// Source id.
    pub source: String,
    /// Shared paths.
    pub paths: Vec<String>,
    /// RFC 3339 expiry, if any.
    pub expires_at: Option<String>,
    /// Download limit, if any.
    pub max_downloads: Option<i64>,
    /// Downloads so far.
    pub downloads: i64,
    /// Upload-only drop link.
    pub upload_only: bool,
    /// Whether a password is set (the hash is never returned).
    pub has_password: bool,
    /// Whether the share is past expiry or out of downloads.
    pub expired: bool,
    /// RFC 3339 creation time.
    pub created_at: String,
    /// The shared collection, for collection links.
    pub collection_id: Option<i64>,
}

impl ShareResponse {
    /// Builds the API view of a share as of `now` (unix seconds). `paths` are
    /// the shared paths as the viewer should see them (scope-relative).
    pub fn from_share(s: &Share, now: i64, paths: Vec<String>) -> Self {
        Self {
            id: s.id,
            token: s.token.clone(),
            url: format!("/s/{}", s.token),
            owner_id: s.owner_id,
            source: s.source.clone(),
            paths,
            expires_at: s.expires_at.map(format_ts),
            max_downloads: s.max_downloads,
            downloads: s.downloads,
            upload_only: s.upload_only,
            has_password: s.password_hash.is_some(),
            expired: s.is_expired(now) || s.is_exhausted(),
            created_at: format_ts(s.created_at),
            collection_id: s.collection_id,
        }
    }
}

/// Formats unix seconds as RFC 3339 UTC.
pub fn format_ts(ts: i64) -> String {
    OffsetDateTime::from_unix_timestamp(ts)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_default()
}

/// Parses an RFC 3339 time into unix seconds; it must lie in the future.
pub fn parse_expiry(s: &str, now: i64) -> Result<i64, ShareError> {
    let t = OffsetDateTime::parse(s, &Rfc3339)
        .map_err(|_| ShareError::bad_request("expires_at must be an RFC 3339 time"))?
        .unix_timestamp();
    if t <= now {
        return Err(ShareError::bad_request("expires_at must be in the future"));
    }
    Ok(t)
}

/// Validates a download limit.
pub fn check_max_downloads(m: i64) -> Result<i64, ShareError> {
    if m < 1 {
        return Err(ShareError::bad_request("max_downloads must be at least 1"));
    }
    Ok(m)
}

/// Validates a new share password.
pub fn check_password(p: &str) -> Result<&str, ShareError> {
    if p.is_empty() {
        return Err(ShareError::bad_request(
            "password must not be empty; send null to remove it",
        ));
    }
    if p.len() > MAX_PASSWORD_LEN {
        return Err(ShareError::bad_request("password is too long"));
    }
    Ok(p)
}

/// One entry of a shared folder listing.
#[derive(Debug, Serialize, PartialEq)]
pub struct PublicEntry {
    /// File or folder name.
    pub name: String,
    /// Path relative to the share root, usable as `?path=`.
    pub path: String,
    /// Whether this is a folder.
    pub is_dir: bool,
    /// Size in bytes (0 for folders).
    pub size: u64,
    /// RFC 3339 modification time, if known.
    pub modified: Option<String>,
}

/// Body of `GET /api/public/shares/{token}`.
#[derive(Debug, Serialize, PartialEq)]
pub struct PublicShareResponse {
    /// Display name: the shared item's name, or the browsed subfolder's.
    pub name: String,
    /// Whether the thing at `path` is a folder.
    pub is_dir: bool,
    /// Size in bytes when it is a file.
    pub size: Option<u64>,
    /// Drop link: visitors may only upload.
    pub upload_only: bool,
    /// The share has a password and this visitor has not unlocked it.
    pub needs_password: bool,
    /// RFC 3339 expiry, if any.
    pub expires_at: Option<String>,
    /// Folder contents (absent for files, locked shares and drop links).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<PublicEntry>>,
}

/// Body of `POST /api/public/shares/{token}/unlock`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnlockRequest {
    /// Password to try.
    pub password: String,
}

/// Response of a successful unlock.
#[derive(Debug, Serialize)]
pub struct UnlockResponse {
    /// Grant to pass as `?access=` by clients that do not keep cookies.
    pub access: String,
}
