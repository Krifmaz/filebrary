//! Management API for share links, nested at `/api/shares`.
//!
//! | Method | Path    | Purpose                                   |
//! |--------|---------|-------------------------------------------|
//! | GET    | `/`     | list the caller's shares (admins: all)    |
//! | POST   | `/`     | create a share                            |
//! | GET    | `/{id}` | one share                                 |
//! | PATCH  | `/{id}` | change password, expiry, limit or mode    |
//! | DELETE | `/{id}` | revoke a share                            |
//!
//! Every route needs a logged-in user (and the CSRF header on unsafe methods,
//! enforced by [`AuthUser`]). Shares of other users answer 404 unless the
//! caller is an administrator.

use std::collections::HashSet;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};

use super::{
    error::{ShareError, ShareResult},
    model::{
        CreateShareRequest, MAX_PATHS, ShareResponse, UpdateShareRequest, check_max_downloads,
        check_password, parse_expiry,
    },
    store::{NewShare, Share, ShareStore, ShareUpdate, now},
};
use crate::{
    auth::{AuthUser, Perms, User},
    error::AppError,
    state::AppState,
    storage::VPath,
};

/// Routes for `/api/shares`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", get(get_one).patch(update).delete(remove))
}

/// Shows a share to `viewer`: paths are mapped into the viewer's scope when
/// possible (administrators and out-of-scope paths see the source path).
fn view(share: &Share, viewer: &User, now: i64) -> ShareResponse {
    let paths = share
        .paths
        .iter()
        .map(|p| {
            viewer
                .visible_path(&share.source, p)
                .unwrap_or_else(|| p.clone())
                .to_string()
        })
        .collect();
    ShareResponse::from_share(share, now, paths)
}

async fn list(State(st): State<AppState>, auth: AuthUser) -> ShareResult<Json<Vec<ShareResponse>>> {
    let owner = (!auth.user.is_admin).then_some(auth.user.id);
    let now = now();
    let shares = ShareStore::new(st.db.clone()).list(owner).await?;
    Ok(Json(
        shares.iter().map(|s| view(s, &auth.user, now)).collect(),
    ))
}

async fn create(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreateShareRequest>,
) -> ShareResult<(StatusCode, Json<ShareResponse>)> {
    let user = &auth.user;
    let now = now();
    if let Some(cid) = req.collection_id {
        return create_for_collection(&st, user, cid, req, now).await;
    }
    if req.source.is_empty() {
        return Err(ShareError::bad_request("source is required"));
    }
    if req.paths.is_empty() {
        return Err(ShareError::bad_request("paths must not be empty"));
    }
    if req.paths.len() > MAX_PATHS {
        return Err(ShareError::bad_request("too many paths"));
    }
    if req.upload_only && req.paths.len() != 1 {
        return Err(ShareError::bad_request(
            "an upload-only link must target exactly one folder",
        ));
    }
    let source = st.source(&req.source).ok_or(AppError::NotFound)?;
    let needed = Perms::SHARE
        | if req.upload_only {
            Perms::UPLOAD
        } else {
            Perms::DOWNLOAD
        };

    let mut paths: Vec<VPath> = Vec::with_capacity(req.paths.len());
    let mut names = HashSet::new();
    for p in &req.paths {
        let visible = VPath::parse(p)?;
        let real = crate::auth::authorize(&st, user, &req.source, needed, &visible).await?;
        if crate::organize::trash::is_reserved(&real) {
            return Err(AppError::NotFound.into());
        }
        if paths.contains(&real) {
            continue;
        }
        let entry = source.storage.stat(&real).await?;
        if req.upload_only && !entry.is_dir() {
            return Err(ShareError::bad_request(
                "an upload-only link must target a folder",
            ));
        }
        if !req.upload_only && real.is_root() && req.paths.len() > 1 {
            return Err(ShareError::bad_request(
                "the source root cannot be combined with other items",
            ));
        }
        // Items of a multi-item share are addressed by name, so names must differ.
        if req.paths.len() > 1 && !names.insert(real.name().unwrap_or_default().to_owned()) {
            return Err(ShareError::bad_request(format!(
                "two shared items are both named {:?}",
                real.name().unwrap_or_default()
            )));
        }
        paths.push(real);
    }

    let password_hash = match req.password {
        Some(p) => {
            check_password(&p)?;
            Some(crate::auth::password::hash(p).await?)
        }
        None => None,
    };
    let expires_at = req
        .expires_at
        .as_deref()
        .map(|e| parse_expiry(e, now))
        .transpose()?;
    let max_downloads = req.max_downloads.map(check_max_downloads).transpose()?;

    let share = ShareStore::new(st.db.clone())
        .create(NewShare {
            owner_id: user.id,
            source: req.source,
            paths,
            password_hash,
            expires_at,
            max_downloads,
            upload_only: req.upload_only,
            collection_id: None,
        })
        .await?;
    tracing::info!(share = share.id, user = user.id, source = %share.source, "share created");
    Ok((StatusCode::CREATED, Json(view(&share, user, now))))
}

/// Password, expiry and download limit of a create request, validated.
async fn link_options(
    req: &CreateShareRequest,
    now: i64,
) -> ShareResult<(Option<String>, Option<i64>, Option<i64>)> {
    let password_hash = match &req.password {
        Some(p) => {
            check_password(p)?;
            Some(crate::auth::password::hash(p.clone()).await?)
        }
        None => None,
    };
    let expires_at = req
        .expires_at
        .as_deref()
        .map(|e| parse_expiry(e, now))
        .transpose()?;
    let max_downloads = req.max_downloads.map(check_max_downloads).transpose()?;
    Ok((password_hash, expires_at, max_downloads))
}

/// `POST /api/shares` with `collection_id`: a link serving a whole collection.
/// The caller must own it and hold `share` + `download` on every item.
async fn create_for_collection(
    st: &AppState,
    user: &User,
    cid: i64,
    req: CreateShareRequest,
    now: i64,
) -> ShareResult<(StatusCode, Json<ShareResponse>)> {
    if !req.source.is_empty() || !req.paths.is_empty() {
        return Err(ShareError::bad_request(
            "send either collection_id or source and paths, not both",
        ));
    }
    if req.upload_only {
        return Err(ShareError::bad_request(
            "a collection cannot be an upload-only link",
        ));
    }
    crate::collections::check_shareable(st, user, cid).await?;
    let (password_hash, expires_at, max_downloads) = link_options(&req, now).await?;
    let share = ShareStore::new(st.db.clone())
        .create(NewShare {
            owner_id: user.id,
            source: String::new(),
            paths: Vec::new(),
            password_hash,
            expires_at,
            max_downloads,
            upload_only: false,
            collection_id: Some(cid),
        })
        .await?;
    tracing::info!(
        share = share.id,
        user = user.id,
        collection = cid,
        "collection share created"
    );
    Ok((StatusCode::CREATED, Json(view(&share, user, now))))
}

/// Loads a share the caller may manage. Other users' shares look like 404.
async fn owned(store: &ShareStore, user: &User, id: i64) -> ShareResult<Share> {
    match store.get(id).await? {
        Some(s) if s.owner_id == user.id || user.is_admin => Ok(s),
        _ => Err(ShareError::not_found()),
    }
}

async fn get_one(
    State(st): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> ShareResult<Json<ShareResponse>> {
    let s = owned(&ShareStore::new(st.db.clone()), &auth.user, id).await?;
    Ok(Json(view(&s, &auth.user, now())))
}

async fn update(
    State(st): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<UpdateShareRequest>,
) -> ShareResult<Json<ShareResponse>> {
    let store = ShareStore::new(st.db.clone());
    let share = owned(&store, &auth.user, id).await?;
    let now = now();
    let mut upd = ShareUpdate::default();
    if let Some(p) = req.password {
        upd.password_hash = Some(match p {
            Some(p) => {
                check_password(&p)?;
                Some(crate::auth::password::hash(p).await?)
            }
            None => None,
        });
    }
    if let Some(e) = req.expires_at {
        upd.expires_at = Some(e.as_deref().map(|e| parse_expiry(e, now)).transpose()?);
    }
    if let Some(m) = req.max_downloads {
        upd.max_downloads = Some(m.map(check_max_downloads).transpose()?);
    }
    if let Some(u) = req.upload_only
        && u != share.upload_only
    {
        if u && share.collection_id.is_some() {
            return Err(ShareError::bad_request(
                "a collection cannot be an upload-only link",
            ));
        }
        if u {
            let [p] = share.paths.as_slice() else {
                return Err(ShareError::bad_request(
                    "an upload-only link must target exactly one folder",
                ));
            };
            let source = st.source(&share.source).ok_or(AppError::NotFound)?;
            if !source.storage.stat(p).await?.is_dir() {
                return Err(ShareError::bad_request(
                    "an upload-only link must target a folder",
                ));
            }
        }
        upd.upload_only = Some(u);
    }
    let s = store
        .update(id, upd)
        .await?
        .ok_or_else(ShareError::not_found)?;
    // The public side re-checks the owner's rights for the new mode on every use.
    Ok(Json(view(&s, &auth.user, now)))
}

async fn remove(
    State(st): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> ShareResult<StatusCode> {
    let store = ShareStore::new(st.db.clone());
    owned(&store, &auth.user, id).await?;
    store.delete(id).await?;
    tracing::info!(share = id, user = auth.user.id, "share deleted");
    Ok(StatusCode::NO_CONTENT)
}
