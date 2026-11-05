//! Chunked, resumable uploads into drop links, on the same wire protocol as
//! signed-in uploads ([`crate::api::uploads`]), nested at
//! `/api/public/shares/{token}/uploads`:
//!
//! 1. `POST /start {path, size}` checks the name, the size limit, the
//!    creator's right and quota, and answers `201 {id, offset: 0, size, done}`.
//! 2. `PATCH /{uid}` with `Upload-Offset: <offset>` and the raw chunk as body
//!    appends data. An offset that differs from what the server has stored
//!    answers `409` with the stored offset (header and body) so the client
//!    resumes from there. Answers `{id, offset, size, done}`; the last chunk
//!    answers `done: true` and `name`, the name the file was stored under
//!    (a taken name gets a numbered suffix, never an overwrite).
//! 3. `GET /{uid}` tells how far an upload got, to resume after a dropped
//!    connection or a reload; `DELETE /{uid}` cancels it.
//!
//! Every call goes through the share again: an expired, deleted or locked
//! link stops its uploads at once, and a password-protected link needs its
//! access grant on every chunk.
//!
//! Uploads are staged like signed-in ones: a row in the `uploads` table and
//! the data in `<data_dir>/uploads/<uid>`, so idle uploads expire with
//! [`crate::api::uploads::purge_stale`]. The row's `user_id` holds the
//! *negated share id*: user ids are positive, so the signed-in routes (which
//! match on the caller's id) never see these rows, and each row belongs to
//! exactly one link. `path` holds the visitor's file name; the free name is
//! picked when the upload completes.

use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use axum_extra::extract::CookieJar;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::AsyncWriteExt;

use super::{
    error::{ShareError, ShareResult},
    public::{self, LiveShare, PublicQuery},
};
use crate::{api::uploads::UPLOAD_OFFSET, db, error::AppError, state::AppState};

/// Most uploads one drop link may have open at once; more answer 429 until
/// some finish, are cancelled or expire.
pub const MAX_OPEN_PER_LINK: i64 = 32;

/// Body for starting an upload.
#[derive(Debug, Deserialize)]
pub struct StartDrop {
    /// The file's name (one name, no folders).
    path: String,
    /// Its size in bytes.
    size: u64,
}

/// How far an upload got.
#[derive(Debug, Serialize)]
pub struct DropStatus {
    id: String,
    offset: u64,
    size: u64,
    done: bool,
    /// Once done: the name the file was stored under.
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(sqlx::FromRow)]
struct Row {
    path: String,
    size: i64,
    received: i64,
}

/// The `uploads.user_id` that marks a row as belonging to share `id`.
fn owner_key(share_id: i64) -> i64 {
    -share_id
}

/// Upload ids are 32 hex characters; anything else is unknown.
fn valid_uid(uid: &str) -> bool {
    uid.len() == 32 && uid.bytes().all(|b| b.is_ascii_hexdigit())
}

async fn load(st: &AppState, live: &LiveShare, uid: &str) -> ShareResult<Row> {
    if !valid_uid(uid) {
        return Err(ShareError::not_found());
    }
    sqlx::query_as("SELECT path, size, received FROM uploads WHERE id = ? AND user_id = ?")
        .bind(uid)
        .bind(owner_key(live.share.id))
        .fetch_optional(&st.db)
        .await?
        .ok_or_else(ShareError::not_found)
}

async fn remove(st: &AppState, uid: &str) -> ShareResult<()> {
    sqlx::query("DELETE FROM uploads WHERE id = ?")
        .bind(uid)
        .execute(&st.db)
        .await?;
    match tokio::fs::remove_file(st.upload_dir.join(uid)).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AppError::from(e).into()),
    }
}

fn answer(status: StatusCode, body: DropStatus) -> Response {
    public::no_store((status, Json(body)).into_response())
}

fn offset_conflict(uid: &str, offset: u64, size: u64) -> Response {
    public::no_store(
        (
            StatusCode::CONFLICT,
            [(UPLOAD_OFFSET, offset.to_string())],
            Json(json!({
                "error": "offset_mismatch",
                "message": "the upload offset does not match; resume from `offset`",
                "id": uid,
                "offset": offset,
                "size": size,
            })),
        )
            .into_response(),
    )
}

/// `POST /{token}/uploads/start`.
pub async fn start(
    State(st): State<AppState>,
    Path(token): Path<String>,
    Query(q): Query<PublicQuery>,
    jar: CookieJar,
    headers: HeaderMap,
    Json(body): Json<StartDrop>,
) -> ShareResult<Response> {
    let live = public::open_drop(&st, &token, &jar, &headers, &q).await?;
    let name = public::drop_name(&body.path)?;
    let limit = public::drop_limit(&st);
    if limit > 0 && body.size > limit {
        return Err(AppError::PayloadTooLarge.into());
    }
    let size = i64::try_from(body.size).map_err(|_| ShareError::bad_request("size too large"))?;
    // Refuse early what would be refused at the end (rights, quota), so
    // nobody sends a whole file for nothing. Checked again on completion.
    let (item, _, _) = public::drop_target(&st, &live, &name, Some(body.size)).await?;
    let open: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM uploads WHERE user_id = ?")
        .bind(owner_key(live.share.id))
        .fetch_one(&st.db)
        .await?;
    if open >= MAX_OPEN_PER_LINK {
        return Err(AppError::TooManyRequests { retry_after: 60 }.into());
    }
    let uid = uuid::Uuid::new_v4().simple().to_string();
    tokio::fs::File::create(st.upload_dir.join(&uid))
        .await
        .map_err(AppError::from)?;
    let now = db::now();
    sqlx::query(
        "INSERT INTO uploads (id, user_id, source, path, size, received, overwrite, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, 0, 0, ?, ?)",
    )
    .bind(&uid)
    .bind(owner_key(live.share.id))
    .bind(&item.source)
    .bind(&name)
    .bind(size)
    .bind(now)
    .bind(now)
    .execute(&st.db)
    .await?;
    if body.size == 0 {
        let done = finish(&st, &live, &uid, &name, 0).await?;
        return Ok(answer(StatusCode::CREATED, done));
    }
    Ok(answer(
        StatusCode::CREATED,
        DropStatus {
            id: uid,
            offset: 0,
            size: body.size,
            done: false,
            name: None,
        },
    ))
}

/// `GET /{token}/uploads/{uid}`.
pub async fn status(
    State(st): State<AppState>,
    Path((token, uid)): Path<(String, String)>,
    Query(q): Query<PublicQuery>,
    jar: CookieJar,
    headers: HeaderMap,
) -> ShareResult<Response> {
    let live = public::open_drop(&st, &token, &jar, &headers, &q).await?;
    let row = load(&st, &live, &uid).await?;
    Ok(answer(
        StatusCode::OK,
        DropStatus {
            id: uid,
            offset: row.received as u64,
            size: row.size as u64,
            done: false,
            name: None,
        },
    ))
}

/// Releases the in-flight marker when dropped.
struct InFlight<'a> {
    st: &'a AppState,
    uid: String,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if let Ok(mut set) = self.st.uploads_in_flight.lock() {
            set.remove(&self.uid);
        }
    }
}

/// `PATCH /{token}/uploads/{uid}`.
pub async fn append(
    State(st): State<AppState>,
    Path((token, uid)): Path<(String, String)>,
    Query(q): Query<PublicQuery>,
    jar: CookieJar,
    headers: HeaderMap,
    body: Body,
) -> ShareResult<Response> {
    let live = public::open_drop(&st, &token, &jar, &headers, &q).await?;
    let row = load(&st, &live, &uid).await?;
    let offset: u64 = headers
        .get(UPLOAD_OFFSET)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| ShareError::bad_request("Upload-Offset header required"))?;
    let received = row.received as u64;
    let size = row.size as u64;
    if offset != received {
        return Ok(offset_conflict(&uid, received, size));
    }
    {
        let mut set = st
            .uploads_in_flight
            .lock()
            .map_err(|_| ShareError::internal("lock poisoned"))?;
        if !set.insert(uid.clone()) {
            return Ok(offset_conflict(&uid, received, size));
        }
    }
    let _guard = InFlight {
        st: &st,
        uid: uid.clone(),
    };

    let max_chunk = st.config.max_chunk_mib * 1024 * 1024;
    let staged = st.upload_dir.join(&uid);
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&staged)
        .await
        .map_err(AppError::from)?;
    // Drop anything an interrupted chunk left beyond the committed offset.
    file.set_len(received).await.map_err(AppError::from)?;
    let mut f = tokio::io::BufWriter::with_capacity(256 * 1024, file);
    tokio::io::AsyncSeekExt::seek(&mut f, std::io::SeekFrom::Start(received))
        .await
        .map_err(AppError::from)?;
    let mut written: u64 = 0;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        // On any failure the committed offset stays; the client resumes there.
        let chunk =
            chunk.map_err(|e| ShareError::bad_request(format!("upload interrupted: {e}")))?;
        written += chunk.len() as u64;
        if written > max_chunk {
            return Err(AppError::PayloadTooLarge.into());
        }
        if received + written > size {
            return Err(ShareError::bad_request("more data than the declared size"));
        }
        f.write_all(&chunk).await.map_err(AppError::from)?;
    }
    f.flush().await.map_err(AppError::from)?;
    f.into_inner().sync_data().await.map_err(AppError::from)?;
    let new_offset = received + written;
    sqlx::query("UPDATE uploads SET received = ?, updated_at = ? WHERE id = ?")
        .bind(new_offset as i64)
        .bind(db::now())
        .bind(&uid)
        .execute(&st.db)
        .await?;
    if new_offset == size {
        let done = finish(&st, &live, &uid, &row.path, size).await?;
        return Ok(answer(StatusCode::OK, done));
    }
    Ok(answer(
        StatusCode::OK,
        DropStatus {
            id: uid,
            offset: new_offset,
            size,
            done: false,
            name: None,
        },
    ))
}

/// Moves a completed upload into the drop folder. The staged data is removed
/// whatever happens: a refusal at this point (rights or quota changed) is
/// final.
async fn finish(
    st: &AppState,
    live: &LiveShare,
    uid: &str,
    name: &str,
    size: u64,
) -> ShareResult<DropStatus> {
    let file = tokio::fs::File::open(st.upload_dir.join(uid))
        .await
        .map_err(AppError::from)?;
    let result = public::store_drop(
        st,
        live,
        name,
        Some(size),
        Box::pin(tokio::io::BufReader::new(file)),
    )
    .await;
    remove(st, uid).await?;
    let (stored, _) = result?;
    Ok(DropStatus {
        id: uid.to_owned(),
        offset: size,
        size,
        done: true,
        name: Some(stored),
    })
}

/// `DELETE /{token}/uploads/{uid}`.
pub async fn cancel(
    State(st): State<AppState>,
    Path((token, uid)): Path<(String, String)>,
    Query(q): Query<PublicQuery>,
    jar: CookieJar,
    headers: HeaderMap,
) -> ShareResult<Response> {
    let live = public::open_drop(&st, &token, &jar, &headers, &q).await?;
    load(&st, &live, &uid).await?;
    remove(&st, &uid).await?;
    Ok(public::no_store(StatusCode::NO_CONTENT.into_response()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_of_links_never_look_like_users() {
        assert!(owner_key(1) < 0);
        assert_ne!(owner_key(1), owner_key(2));
    }

    #[test]
    fn upload_ids_are_checked() {
        assert!(valid_uid(&"a".repeat(32)));
        assert!(!valid_uid("../../etc/passwd"));
        assert!(!valid_uid(&"g".repeat(32)));
    }
}
