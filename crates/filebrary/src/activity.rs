//! The activity log: who did what, to which file, and when.
//!
//! # Recording
//!
//! Any handler that changes something (or opens a share) records one event:
//!
//! ```ignore
//! use filebrary::activity::{Activity, action};
//! Activity::new(action::UPLOAD)
//!     .user(&auth.user)
//!     .at("files", &real_path)
//!     .ip(client_ip.0.as_deref())
//!     .record(&state)
//!     .await;
//! ```
//!
//! Recording never fails the request: a database error is logged and dropped.
//! Paths are *real* paths inside the source (the user's scope applied), the
//! same as everywhere else in the database.
//!
//! # Reading
//!
//! `GET /api/activity` (see [`routes`]) returns events newest first. Admins see
//! every event; other users see the events on paths they can view plus their
//! own actions, with paths shown relative to their scope.
//!
//! # Retention
//!
//! Events older than `activity_retention_days` (default 365, `0` keeps them
//! forever) are deleted by the hourly maintenance task, see [`purge_old`].

use std::net::{IpAddr, SocketAddr};

use axum::{
    Json, Router,
    extract::{ConnectInfo, FromRequestParts, Query, State},
    http::request::Parts,
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::{
    auth::{AuthUser, Perms, User},
    db,
    error::{AppError, AppResult},
    organize::meta,
    state::AppState,
    storage::VPath,
};

/// Well-known action names. Other work streams may add their own using the
/// same `noun.verb` or plain-verb style; the column is free text.
pub mod action {
    /// A file was uploaded (created or overwritten).
    pub const UPLOAD: &str = "upload";
    /// A file or a zip of a selection was downloaded.
    pub const DOWNLOAD: &str = "download";
    /// A folder was created.
    pub const MKDIR: &str = "mkdir";
    /// A file was edited in place (text editor).
    pub const EDIT: &str = "edit";
    /// Renamed or moved (`target` is the new path).
    pub const MOVE: &str = "move";
    /// Copied (`target` is the copy).
    pub const COPY: &str = "copy";
    /// Moved to the trash bin.
    pub const TRASH: &str = "trash";
    /// Restored from the trash bin (`target` is where it went).
    pub const RESTORE: &str = "restore";
    /// Deleted permanently (directly, or purged from the trash by a user).
    pub const DELETE: &str = "delete";
    /// Purged from the trash automatically after the retention period.
    pub const PURGE: &str = "trash.purge";
    /// Tags of a file changed (`details.added`, `details.removed`).
    pub const TAG: &str = "tag";
    /// A share link was created.
    pub const SHARE_CREATE: &str = "share.create";
    /// A share link was opened (anonymous).
    pub const SHARE_OPEN: &str = "share.open";
    /// A share link was deleted.
    pub const SHARE_DELETE: &str = "share.delete";
    /// A user logged in.
    pub const LOGIN: &str = "login";
}

/// The client address, from the connection or (when `trust_proxy_headers`
/// is on) the first `X-Forwarded-For` entry. Never rejects.
#[derive(Clone, Debug, Default)]
pub struct ClientIp(pub Option<String>);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if state.config.trust_proxy_headers
            && let Some(ip) = parts
                .headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
                .and_then(|v| v.trim().parse::<IpAddr>().ok())
        {
            return Ok(Self(Some(ip.to_string())));
        }
        Ok(Self(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0.ip().to_string()),
        ))
    }
}

/// An event being built; finish with [`Activity::record`].
#[derive(Clone, Debug, Default)]
#[must_use = "call .record(&state) to store the event"]
pub struct Activity {
    action: String,
    user_id: Option<i64>,
    username: Option<String>,
    source: Option<String>,
    path: Option<String>,
    target: Option<String>,
    details: Option<Value>,
    ip: Option<String>,
}

impl Activity {
    /// Starts an event for `action` (see [`action`]).
    pub fn new(action: &str) -> Self {
        Self {
            action: action.to_owned(),
            ..Self::default()
        }
    }

    /// Sets the acting user.
    pub fn user(mut self, user: &User) -> Self {
        self.user_id = Some(user.id);
        self.username = Some(user.username.clone());
        self
    }

    /// Sets the acting user by id and name (when no [`User`] is loaded).
    pub fn user_raw(mut self, id: i64, username: &str) -> Self {
        self.user_id = Some(id);
        self.username = Some(username.to_owned());
        self
    }

    /// Sets the source and real path the action applied to.
    pub fn at(mut self, source: &str, path: &VPath) -> Self {
        self.source = Some(source.to_owned());
        self.path = Some(path.to_string());
        self
    }

    /// Sets the destination (real path in the same source) of a move or copy.
    pub fn target(mut self, path: &VPath) -> Self {
        self.target = Some(path.to_string());
        self
    }

    /// Attaches a small JSON object with extra facts (sizes, share id, ...).
    pub fn details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    /// Sets the client address.
    pub fn ip(mut self, ip: Option<&str>) -> Self {
        self.ip = ip.map(str::to_owned);
        self
    }

    /// Stores the event. Failures are logged, never returned.
    pub async fn record(self, state: &AppState) {
        self.record_in(&state.db).await;
    }

    /// Stores the event in `pool`. Failures are logged, never returned.
    pub async fn record_in(self, pool: &SqlitePool) {
        let r = sqlx::query(
            "INSERT INTO activity (at, user_id, username, action, source, path, target, details, ip)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(db::now())
        .bind(self.user_id)
        .bind(&self.username)
        .bind(&self.action)
        .bind(&self.source)
        .bind(&self.path)
        .bind(&self.target)
        .bind(self.details.as_ref().map(Value::to_string))
        .bind(&self.ip)
        .execute(pool)
        .await;
        if let Err(e) = r {
            tracing::warn!(error = %e, action = %self.action, "could not record activity");
        }
    }
}

/// Deletes events older than the configured retention. Returns how many.
///
/// # Errors
///
/// Database failures.
pub async fn purge_old(state: &AppState) -> AppResult<u64> {
    let days = state.config.activity_retention_days;
    if days == 0 {
        return Ok(0);
    }
    let cutoff = db::now() - i64::try_from(days.saturating_mul(86_400)).unwrap_or(i64::MAX / 2);
    let r = sqlx::query("DELETE FROM activity WHERE at < ?")
        .bind(cutoff)
        .execute(&state.db)
        .await?;
    Ok(r.rows_affected())
}

/// One event as returned by the API.
#[derive(Debug, Serialize)]
pub struct Event {
    /// Event id (use as `before` to page backwards).
    pub id: i64,
    /// Unix time.
    pub at: i64,
    /// Acting user's id, if any.
    pub user_id: Option<i64>,
    /// Acting user's name at the time, if any.
    pub username: Option<String>,
    /// Action name.
    pub action: String,
    /// Source name.
    pub source: Option<String>,
    /// Path as the caller sees it (`null` if outside the caller's scope).
    pub path: Option<VPath>,
    /// Destination as the caller sees it.
    pub target: Option<VPath>,
    /// Extra facts.
    pub details: Option<Value>,
    /// Client address (administrators only).
    pub ip: Option<String>,
}

/// Query parameters of `GET /api/activity`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ActivityQuery {
    /// Only this source.
    pub source: Option<String>,
    /// Only this path (requires `source`), plus everything below it unless
    /// `recursive=false`.
    pub path: Option<String>,
    /// Include events below `path` (default true).
    pub recursive: Option<bool>,
    /// Only this user (by name).
    pub user: Option<String>,
    /// Only this action.
    pub action: Option<String>,
    /// Events at or after this Unix time.
    pub since: Option<i64>,
    /// Events before this Unix time.
    pub until: Option<i64>,
    /// Events with an id lower than this (paging cursor).
    pub before: Option<i64>,
    /// Page size, 1–500 (default 100).
    pub limit: Option<u32>,
}

/// One page of events.
#[derive(Debug, Serialize)]
pub struct EventPage {
    /// Events, newest first.
    pub items: Vec<Event>,
    /// Pass as `before` for the next page; `null` when there is none.
    pub next_before: Option<i64>,
}

/// Routes nested at `/api/activity`.
pub fn routes() -> Router<AppState> {
    Router::new().route("/", get(list))
}

type Row = (
    i64,
    i64,
    Option<i64>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// `GET /api/activity`: events visible to the caller, newest first.
async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<ActivityQuery>,
) -> AppResult<Json<EventPage>> {
    let user = &auth.user;
    let limit = i64::from(q.limit.unwrap_or(100).clamp(1, 500));
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
        "SELECT id, at, user_id, username, action, source, path, target, details, ip \
         FROM activity WHERE 1 = 1",
    );

    if let Some(source) = &q.source {
        qb.push(" AND source = ").push_bind(source.clone());
        if let Some(p) = &q.path {
            let vis = VPath::parse(p)?;
            let real = user.require(source, Perms::VIEW, &vis)?;
            let k = meta::key(&real);
            if q.recursive.unwrap_or(true) {
                qb.push(" AND (path = ")
                    .push_bind(k.clone())
                    .push(" OR substr(path, 1, length(")
                    .push_bind(k.clone())
                    .push(") + 1) = ")
                    .push_bind(k)
                    .push(" || '/')");
            } else {
                qb.push(" AND path = ").push_bind(real.to_string());
            }
        }
    } else if q.path.is_some() {
        return Err(AppError::bad_request("path needs source"));
    }
    if let Some(u) = &q.user {
        qb.push(" AND username = ").push_bind(u.clone());
    }
    if let Some(a) = &q.action {
        qb.push(" AND action = ").push_bind(a.clone());
    }
    if let Some(t) = q.since {
        qb.push(" AND at >= ").push_bind(t);
    }
    if let Some(t) = q.until {
        qb.push(" AND at < ").push_bind(t);
    }
    if let Some(b) = q.before {
        qb.push(" AND id < ").push_bind(b);
    }

    if !user.is_admin {
        // Own actions, or events on a path inside a viewable grant.
        qb.push(" AND (user_id = ").push_bind(user.id);
        for (source, grant) in &user.grants {
            if !grant.perms.contains(Perms::VIEW) {
                continue;
            }
            let k = meta::key(&grant.scope);
            qb.push(" OR (source = ")
                .push_bind(source.clone())
                .push(" AND (path = ")
                .push_bind(k.clone())
                .push(" OR substr(path, 1, length(")
                .push_bind(k.clone())
                .push(") + 1) = ")
                .push_bind(k)
                .push(" || '/'))");
        }
        qb.push(")");
    }
    qb.push(" ORDER BY id DESC LIMIT ").push_bind(limit + 1);

    let mut rows: Vec<Row> = qb.build_query_as().fetch_all(&state.db).await?;
    let more = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));

    let show = |source: &Option<String>, p: Option<String>| -> Option<VPath> {
        let real = VPath::parse(&p?).ok()?;
        user.visible_path(source.as_deref()?, &real)
    };
    let items: Vec<Event> = rows
        .into_iter()
        .map(
            |(id, at, user_id, username, action, source, path, target, details, ip)| Event {
                id,
                at,
                user_id,
                username,
                action,
                path: show(&source, path),
                target: show(&source, target),
                source,
                details: details.and_then(|d| serde_json::from_str(&d).ok()),
                ip: if user.is_admin { ip } else { None },
            },
        )
        .collect();
    let next_before = more.then(|| items.last().map(|e| e.id)).flatten();
    Ok(Json(EventPage { items, next_before }))
}
