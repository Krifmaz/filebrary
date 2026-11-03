//! Chunked, resumable uploads.
//!
//! 1. `POST /api/sources/{id}/uploads {path, size, overwrite?}` creates an
//!    upload and returns `{id, offset: 0, size}`.
//! 2. `PATCH /api/uploads/{uid}` with header `Upload-Offset: <offset>` and the
//!    raw chunk as body appends data. The offset must equal what the server
//!    has, otherwise `409` with the current offset is returned so the client
//!    can resume. Returns `{id, offset, size, done, entry?}`.
//! 3. When `offset == size` the file is moved into place atomically (parent
//!    folders are created, so folder uploads just send relative paths).
//! 4. `GET /api/uploads/{uid}` returns progress (to resume after a disconnect);
//!    `DELETE /api/uploads/{uid}` cancels.
//!
//! Data is staged in `<data_dir>/uploads/<uid>`; uploads idle for longer than
//! `upload_expiry_hours` are removed by [`purge_stale`].

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::AsyncWriteExt;

use super::files::{EntryDto, scoped, source_for};
use crate::{
    auth::{AuthUser, Perms},
    db,
    error::{AppError, AppResult},
    state::AppState,
    storage::VPath,
};

/// Header carrying the chunk's starting offset.
pub const UPLOAD_OFFSET: &str = "upload-offset";

/// Routes for this module, merged under `/api`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sources/{id}/uploads", post(create))
        .route("/uploads/{uid}", get(status).patch(append).delete(cancel))
}

/// Body for creating an upload.
#[derive(Debug, Deserialize)]
pub struct CreateUpload {
    path: VPath,
    size: u64,
    #[serde(default)]
    overwrite: bool,
}

/// Upload progress.
#[derive(Debug, Serialize)]
pub struct UploadStatus {
    id: String,
    offset: u64,
    size: u64,
    done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    entry: Option<EntryDto>,
}

#[derive(sqlx::FromRow)]
struct UploadRow {
    user_id: i64,
    source: String,
    path: String,
    size: i64,
    received: i64,
    overwrite: bool,
}

/// Validates an upload id (we generate 32 hex chars) before it touches the disk.
fn valid_uid(uid: &str) -> AppResult<()> {
    if uid.len() == 32 && uid.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AppError::NotFound)
    }
}

async fn load(state: &AppState, uid: &str, user_id: i64) -> AppResult<UploadRow> {
    valid_uid(uid)?;
    let row: UploadRow = sqlx::query_as(
        "SELECT user_id, source, path, size, received, overwrite FROM uploads WHERE id = ?",
    )
    .bind(uid)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;
    if row.user_id != user_id {
        return Err(AppError::NotFound);
    }
    Ok(row)
}

async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<CreateUpload>,
) -> AppResult<Json<UploadStatus>> {
    source_for(&state, &auth.user, &id)?;
    if body.path.is_root() {
        return Err(AppError::bad_request("a file name is required"));
    }
    let size = i64::try_from(body.size).map_err(|_| AppError::bad_request("size too large"))?;
    let need = if body.overwrite {
        Perms::UPLOAD | Perms::EDIT
    } else {
        Perms::UPLOAD
    };
    let real = auth.user.require(&id, need, &body.path)?;
    let st = scoped(&state, &auth.user, &id, need, &body.path).await?;
    match st.stat(&body.path).await {
        Ok(e) if e.is_dir() => return Err(AppError::conflict("a folder exists at that path")),
        Ok(_) if !body.overwrite => return Err(AppError::conflict("the target already exists")),
        _ => {}
    }
    let uid = uuid::Uuid::new_v4().simple().to_string();
    tokio::fs::File::create(state.upload_dir.join(&uid)).await?;
    let now = db::now();
    sqlx::query(
        "INSERT INTO uploads (id, user_id, source, path, size, received, overwrite, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, 0, ?, ?, ?)",
    )
    .bind(&uid)
    .bind(auth.user.id)
    .bind(&id)
    .bind(real.to_string())
    .bind(size)
    .bind(body.overwrite)
    .bind(now)
    .bind(now)
    .execute(&state.db)
    .await?;
    let mut status = UploadStatus {
        id: uid.clone(),
        offset: 0,
        size: body.size,
        done: false,
        entry: None,
    };
    if body.size == 0 {
        status = finish(&state, &auth, &uid).await?;
    }
    Ok(Json(status))
}

async fn status(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(uid): Path<String>,
) -> AppResult<Json<UploadStatus>> {
    let row = load(&state, &uid, auth.user.id).await?;
    Ok(Json(UploadStatus {
        id: uid,
        offset: row.received as u64,
        size: row.size as u64,
        done: false,
        entry: None,
    }))
}

/// Releases the in-flight marker when dropped.
struct InFlight<'a> {
    state: &'a AppState,
    uid: String,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if let Ok(mut set) = self.state.uploads_in_flight.lock() {
            set.remove(&self.uid);
        }
    }
}

async fn append(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(uid): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> AppResult<Response> {
    let row = load(&state, &uid, auth.user.id).await?;
    let offset: u64 = headers
        .get(UPLOAD_OFFSET)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| AppError::bad_request("Upload-Offset header required"))?;
    let received = row.received as u64;
    let size = row.size as u64;
    if offset != received {
        return Ok(offset_conflict(&uid, received, size));
    }
    {
        let mut set = state
            .uploads_in_flight
            .lock()
            .map_err(|_| AppError::Internal(anyhow::anyhow!("lock poisoned")))?;
        if !set.insert(uid.clone()) {
            return Ok(offset_conflict(&uid, received, size));
        }
    }
    let _guard = InFlight {
        state: &state,
        uid: uid.clone(),
    };
    // Permission may have been revoked since the upload started.
    auth.user
        .require(&row.source, Perms::UPLOAD, &VPath::root())?;

    let max_chunk = state.config.max_chunk_mib * 1024 * 1024;
    let staged = state.upload_dir.join(&uid);
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&staged)
        .await?;
    // Drop anything a previously interrupted chunk left beyond the committed offset.
    file.set_len(received).await?;
    let mut f = tokio::io::BufWriter::with_capacity(256 * 1024, file);
    tokio::io::AsyncSeekExt::seek(&mut f, std::io::SeekFrom::Start(received)).await?;
    let mut written: u64 = 0;
    let mut stream = body.into_data_stream();
    let mut failed = None;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                failed = Some(AppError::bad_request(format!("upload interrupted: {e}")));
                break;
            }
        };
        written += chunk.len() as u64;
        if written > max_chunk {
            failed = Some(AppError::PayloadTooLarge);
            break;
        }
        if received + written > size {
            failed = Some(AppError::bad_request("more data than the declared size"));
            break;
        }
        f.write_all(&chunk).await?;
    }
    if let Some(e) = failed {
        // Keep the committed offset; the client resumes from there.
        return Err(e);
    }
    f.flush().await?;
    f.into_inner().sync_data().await?;
    let new_offset = received + written;
    sqlx::query("UPDATE uploads SET received = ?, updated_at = ? WHERE id = ?")
        .bind(new_offset as i64)
        .bind(db::now())
        .bind(&uid)
        .execute(&state.db)
        .await?;
    if new_offset == size {
        return Ok(Json(finish(&state, &auth, &uid).await?).into_response());
    }
    Ok(Json(UploadStatus {
        id: uid,
        offset: new_offset,
        size,
        done: false,
        entry: None,
    })
    .into_response())
}

fn offset_conflict(uid: &str, offset: u64, size: u64) -> Response {
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
        .into_response()
}

/// Moves a completed upload into its source.
async fn finish(state: &AppState, auth: &AuthUser, uid: &str) -> AppResult<UploadStatus> {
    let row = load(state, uid, auth.user.id).await?;
    let real = VPath::parse(&row.path)?;
    // Re-check against the user's current grant (real path must stay in scope).
    let visible = auth
        .user
        .visible_path(&row.source, &real)
        .ok_or_else(|| AppError::forbidden("the upload target is no longer in your scope"))?;
    let need = if row.overwrite {
        Perms::UPLOAD | Perms::EDIT
    } else {
        Perms::UPLOAD
    };
    let st = scoped(state, &auth.user, &row.source, need, &visible).await?;
    if let Some(parent) = visible.parent() {
        st.mkdir(&parent).await?;
    }
    let staged = state.upload_dir.join(uid);
    let file = tokio::fs::File::open(&staged).await?;
    let result = st
        .write(
            &visible,
            Box::pin(tokio::io::BufReader::new(file)),
            row.overwrite,
        )
        .await;
    // Whatever happened, the staged data is consumed or useless now.
    remove(state, uid).await?;
    result?;
    let entry = st.stat(&visible).await?;
    Ok(UploadStatus {
        id: uid.to_owned(),
        offset: row.size as u64,
        size: row.size as u64,
        done: true,
        entry: Some(EntryDto::from_entry(entry, visible)),
    })
}

async fn remove(state: &AppState, uid: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM uploads WHERE id = ?")
        .bind(uid)
        .execute(&state.db)
        .await?;
    match tokio::fs::remove_file(state.upload_dir.join(uid)).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

async fn cancel(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(uid): Path<String>,
) -> AppResult<StatusCode> {
    load(&state, &uid, auth.user.id).await?;
    remove(&state, &uid).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes uploads idle for longer than the configured expiry, and staged
/// files with no database row. Returns how many uploads were removed.
///
/// # Errors
///
/// Database or file-system failures.
pub async fn purge_stale(state: &AppState) -> AppResult<u64> {
    let cutoff = db::now() - (state.config.upload_expiry_hours as i64) * 3600;
    let stale: Vec<(String,)> = sqlx::query_as("SELECT id FROM uploads WHERE updated_at < ?")
        .bind(cutoff)
        .fetch_all(&state.db)
        .await?;
    let n = stale.len() as u64;
    for (uid,) in stale {
        remove(state, &uid).await?;
    }
    // Orphaned staging files (e.g. the row was lost in a crash).
    let mut rd = tokio::fs::read_dir(&state.upload_dir).await?;
    while let Some(ent) = rd.next_entry().await? {
        let Some(name) = ent.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if valid_uid(&name).is_err() {
            continue;
        }
        let exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM uploads WHERE id = ?")
            .bind(&name)
            .fetch_optional(&state.db)
            .await?;
        if exists.is_none() {
            let _ = tokio::fs::remove_file(ent.path()).await;
        }
    }
    Ok(n)
}
