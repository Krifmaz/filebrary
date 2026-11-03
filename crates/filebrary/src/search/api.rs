//! HTTP endpoints for search, merged under `/api`.
//!
//! * `GET /api/search?q=&source=&path=&type=&ext=&min_size=&max_size=&after=&before=&tag=&sort=&order=&limit=&offset=`
//!   returns [`SearchResults`]. See [`SearchParams`] for every filter.
//! * `GET /api/search/status` lists the indexing status of the sources the
//!   caller can view.
//! * `POST /api/search/reindex` `{"source": "name"}` (administrators; omit
//!   `source` for all) starts a full rescan in the background and answers 202.

use axum::{
    Json, Router,
    extract::{Query, State, rejection::QueryRejection},
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;

use super::SourceStatus;
use super::query::{self, Filters, SearchParams, SearchResults};
use crate::{
    auth::{AdminUser, AuthUser, Perms},
    error::{AppError, AppResult},
    state::AppState,
};

/// Routes for this module, merged under `/api`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/search", get(search))
        .route("/search/status", get(status))
        .route("/search/reindex", post(reindex))
}

/// `GET /api/search`.
async fn search(
    State(state): State<AppState>,
    auth: AuthUser,
    params: Result<Query<SearchParams>, QueryRejection>,
) -> AppResult<Json<SearchResults>> {
    let Query(params) = params.map_err(|e| AppError::bad_request(e.body_text()))?;
    if !state.search.settings().enabled {
        return Err(AppError::bad_request("search is disabled on this server"));
    }
    let mut filters = Filters::from_params(&params)?;
    if let Some(content) = params.content.as_deref().filter(|c| !c.trim().is_empty()) {
        filters.content = Some(crate::content::candidates(&state, &auth.user, content).await?);
        filters.by_relevance = params.sort.is_none();
    }
    let (hits, has_more) =
        query::run(&state.db, &auth.user, &filters, |s| state.search.indexes(s)).await?;
    let indexing = state.search.status().iter().any(|s| {
        s.scanning
            && auth
                .user
                .grant(&s.source)
                .is_some_and(|g| g.perms.contains(Perms::VIEW))
    });
    Ok(Json(SearchResults {
        hits,
        has_more,
        indexing,
    }))
}

/// `GET /api/search/status`.
async fn status(State(state): State<AppState>, auth: AuthUser) -> Json<Vec<SourceStatus>> {
    Json(
        state
            .search
            .status()
            .into_iter()
            .filter(|s| {
                auth.user
                    .grant(&s.source)
                    .is_some_and(|g| g.perms.contains(Perms::VIEW))
            })
            .map(|mut s| {
                if !auth.user.is_admin {
                    s.last_error = None; // may contain host paths
                }
                s
            })
            .collect(),
    )
}

/// Body of `POST /api/search/reindex`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReindexBody {
    source: Option<String>,
}

/// `POST /api/search/reindex`.
async fn reindex(
    State(state): State<AppState>,
    _admin: AdminUser,
    body: Option<Json<ReindexBody>>,
) -> AppResult<StatusCode> {
    if !state.search.settings().enabled {
        return Err(AppError::bad_request("search is disabled on this server"));
    }
    match body.and_then(|Json(b)| b.source) {
        Some(name) => {
            if !state.search.request_rescan(&name) {
                return Err(AppError::NotFound);
            }
        }
        None => {
            for s in state.search.status() {
                state.search.request_rescan(&s.source);
            }
        }
    }
    Ok(StatusCode::ACCEPTED)
}
