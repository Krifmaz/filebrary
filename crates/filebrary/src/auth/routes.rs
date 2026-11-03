//! Authentication and administration endpoints, merged under `/api`.
//!
//! | Method & path | Who | Body / query | Answer |
//! |---|---|---|---|
//! | `GET /setup` | anyone | | `{needed}`: whether first-run setup is waiting |
//! | `POST /setup` | anyone, once | `{token, username, password, remember?}` | `{user, csrf_token}` + session cookie |
//! | `POST /auth/login` | anyone | `{username, password, totp?, remember?}` | `{user, csrf_token}` + session cookie |
//! | `POST /auth/logout` | user | | 204, cookie cleared |
//! | `GET /me` | user | | `{user, csrf_token, groups, sources}` |
//! | `POST /me/password` | user | `{current, new}` | 204; other sessions end |
//! | `GET /me/permissions` | user | `?source=&path=` | `{perms}` at that path |
//! | `GET/PUT /me/preferences` | user | `PUT`: `{view_mode}` | `{view_mode}` |
//! | `GET /users` · `POST /users` | admin | [`NewUser`] | users / user |
//! | `GET/PUT/DELETE /users/{id}` | admin | `PUT`: [`users::UserUpdate`] | user / 204 |
//! | `PUT /users/{id}/home` | admin | `{source, path?}` | `{source, path}` |
//! | `GET/PUT /users/{id}/scopes` | admin | `PUT`: `{source, scope}` | `{source: scope}` |
//! | `GET /users/{id}/groups` | admin | | groups |
//! | `GET /groups` · `POST /groups` | admin | `{name}` | groups / group |
//! | `DELETE /groups/{id}` | admin | | 204 |
//! | `GET /groups/{id}/members` | admin | | users |
//! | `PUT/DELETE /groups/{id}/members/{user_id}` | admin | | 204 |
//! | `GET /permissions` · `PUT /permissions` | admin | `?source=` / `{source, path, subject, perms}` | rules / rule |
//! | `DELETE /permissions/{id}` | admin | | 204 |
//!
//! `remember` (default `true`) picks a persistent session; `false` gives a
//! browser-session cookie with a short lifetime (see [`session`]).
//!
//! `role` on users is `admin`, `user` or `read_only` ([`users::Role`]).
//!
//! Login failures answer 401 `invalid_credentials` (whether the user exists
//! or not, with the same timing), 401 `totp_required` when a second factor
//! hook wants a code, and 429 `too_many_requests` with `Retry-After` once
//! the rate limit is hit. The login body must be JSON, which browsers cannot
//! send cross-site without a CORS preflight, so login CSRF is not possible.

use std::collections::BTreeMap;

use axum::{
    Json, Router,
    extract::{FromRef, FromRequestParts, Path, Query, State},
    http::{StatusCode, header, request::Parts},
    routing::{get, post, put},
};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    AdminUser, AuthUser, Perms, Subject, User, client_ip, groups, password, permissions, session,
    users,
};
use crate::{
    error::{AppError, AppResult},
    state::AppState,
    storage::VPath,
};

/// Routes for this module, merged under `/api`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/setup", get(setup_status).post(setup))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/me", get(me))
        .route("/me/password", post(change_password))
        .route("/me/permissions", get(my_permissions))
        .route("/me/preferences", get(get_prefs).put(put_prefs))
        .route("/users", get(list_users).post(create_user))
        .route(
            "/users/{id}",
            get(get_user).put(update_user).delete(delete_user),
        )
        .route("/users/{id}/home", put(set_home))
        .route("/users/{id}/scopes", get(get_scopes).put(set_scope))
        .route("/users/{id}/groups", get(user_groups))
        .route("/groups", get(list_groups).post(create_group))
        .route("/groups/{id}", axum::routing::delete(delete_group))
        .route("/groups/{id}/members", get(group_members))
        .route(
            "/groups/{id}/members/{user_id}",
            put(add_member).delete(remove_member),
        )
        .route("/permissions", get(list_rules).put(set_rule))
        .route("/permissions/{id}", axum::routing::delete(delete_rule))
}

/// Client address and user agent, for sessions and rate limiting.
struct ClientInfo {
    ip: Option<String>,
    user_agent: Option<String>,
}

impl<S> FromRequestParts<S> for ClientInfo
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(state);
        Ok(Self {
            ip: client_ip(parts, &state.config),
            user_agent: parts
                .headers
                .get(header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned),
        })
    }
}

fn user_json(u: &User) -> Value {
    json!({
        "id": u.id,
        "username": u.username,
        "is_admin": u.is_admin,
        "read_only": u.read_only,
        "role": u.role(),
        "auth_source": u.auth_source,
    })
}

const fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
    #[serde(default)]
    totp: Option<String>,
    /// "Remember me": a persistent session (the default, for API clients).
    #[serde(default = "yes")]
    remember: bool,
}

// ---- first-run setup ----

async fn setup_status(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let needed = state.auth.setup_pending().await && users::count(&state.db).await? == 0;
    Ok(Json(json!({ "needed": needed })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupBody {
    token: String,
    username: String,
    password: String,
    #[serde(default = "yes")]
    remember: bool,
}

/// `POST /setup`: creates the first administrator, once, given the setup
/// token from the log, and signs them in.
async fn setup(
    State(state): State<AppState>,
    client: ClientInfo,
    jar: CookieJar,
    Json(body): Json<SetupBody>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let ip = client.ip.as_deref().unwrap_or("unknown");
    let limiter = &state.auth.limiter;
    limiter.check("\0setup", ip)?;
    // Held until the administrator exists, so two requests cannot both win.
    let mut slot = state.auth.setup_token.lock().await;
    let Some(expected) = slot.as_deref() else {
        return Err(AppError::conflict("setup is already complete"));
    };
    if users::count(&state.db).await? > 0 {
        *slot = None;
        return Err(AppError::conflict("setup is already complete"));
    }
    if !crate::crypto::ct_eq(body.token.trim().as_bytes(), expected.as_bytes()) {
        limiter.record_failure("\0setup", ip);
        tracing::info!(ip, "wrong setup token");
        return Err(AppError::forbidden(
            "the setup token is wrong; copy it from the server log",
        ));
    }
    let rec = users::create(&state.db, &body.username, &body.password, true).await?;
    *slot = None;
    drop(slot);
    limiter.record_success("\0setup");
    tracing::info!(username = %rec.username, ip, "first administrator created by setup");
    let user = super::load_user(&state, rec.id)
        .await?
        .ok_or(AppError::InvalidCredentials)?;
    let (sess, cookie) = session::issue_with(
        &state,
        rec.id,
        client.ip.as_deref(),
        client.user_agent.as_deref(),
        body.remember,
    )
    .await?;
    Ok((
        jar.add(cookie),
        Json(json!({ "user": user_json(&user), "csrf_token": sess.csrf_token })),
    ))
}

async fn login(
    State(state): State<AppState>,
    client: ClientInfo,
    jar: CookieJar,
    Json(body): Json<LoginBody>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let ip = client.ip.as_deref().unwrap_or("unknown");
    let limiter = &state.auth.limiter;
    limiter.check(&body.username, ip)?;
    if body.username.len() > users::MAX_USERNAME_LEN || body.password.len() > password::MAX_LEN {
        limiter.record_failure(&body.username, ip);
        return Err(AppError::InvalidCredentials);
    }

    let found = users::get_with_hash(&state.db, &body.username).await?;
    let rec = match found {
        Some((rec, Some(hash))) if rec.auth_source == "local" => {
            password::verify(body.password, hash).await.then_some(rec)
        }
        other => {
            // Not a local password account: an external directory (LDAP) may
            // know the user. Without one, spend the same time as a real check.
            let existing = other.map(|(r, _)| r);
            let ext = state
                .auth
                .hooks()
                .authenticate_external(&state, &body.username, &body.password, existing.as_ref())
                .await?;
            if ext.is_none() {
                password::verify(body.password, password::dummy_hash().await).await;
            }
            ext
        }
    };
    let rec = match rec {
        Some(r) if !r.disabled => r,
        _ => {
            limiter.record_failure(&body.username, ip);
            tracing::info!(username = %body.username, ip, "failed login");
            return Err(AppError::InvalidCredentials);
        }
    };

    match state
        .auth
        .hooks()
        .after_password(&state, &rec, body.totp.as_deref())
        .await
    {
        Ok(()) => {}
        // Asking for a code is not a failure.
        Err(AppError::TotpRequired) => return Err(AppError::TotpRequired),
        Err(e) => {
            limiter.record_failure(&body.username, ip);
            return Err(e);
        }
    }
    limiter.record_success(&body.username);

    let user = super::load_user(&state, rec.id)
        .await?
        .ok_or(AppError::InvalidCredentials)?;
    let (sess, cookie) = session::issue_with(
        &state,
        rec.id,
        client.ip.as_deref(),
        client.user_agent.as_deref(),
        body.remember,
    )
    .await?;
    tracing::info!(user = rec.id, ip, "login");
    Ok((
        jar.add(cookie),
        Json(json!({ "user": user_json(&user), "csrf_token": sess.csrf_token })),
    ))
}

async fn logout(
    State(state): State<AppState>,
    me: AuthUser,
    jar: CookieJar,
) -> AppResult<(StatusCode, CookieJar)> {
    if !me.session_hash.is_empty() {
        session::delete(&state, &me.session_hash).await?;
    }
    Ok((
        StatusCode::NO_CONTENT,
        jar.add(session::clear_cookie(&state)),
    ))
}

async fn me(State(state): State<AppState>, me: AuthUser) -> AppResult<Json<Value>> {
    let groups = groups::for_user(&state.db, me.user.id).await?;
    Ok(Json(json!({
        "user": user_json(&me.user),
        "csrf_token": me.csrf_token,
        "groups": groups,
        "sources": me.user.grants,
    })))
}

#[derive(Deserialize)]
struct PasswordBody {
    current: String,
    new: String,
}

async fn change_password(
    State(state): State<AppState>,
    client: ClientInfo,
    me: AuthUser,
    Json(body): Json<PasswordBody>,
) -> AppResult<StatusCode> {
    if me.scopes.is_some() {
        return Err(AppError::forbidden(
            "passwords cannot be changed with an API key",
        ));
    }
    let ip = client.ip.as_deref().unwrap_or("unknown");
    state.auth.limiter.check(&me.user.username, ip)?;
    let Some((rec, Some(hash))) = users::get_with_hash(&state.db, &me.user.username).await? else {
        return Err(AppError::bad_request(
            "this account signs in through an external provider",
        ));
    };
    if !password::verify(body.current, hash).await {
        state.auth.limiter.record_failure(&me.user.username, ip);
        return Err(AppError::forbidden("the current password is wrong"));
    }
    users::set_password(&state.db, rec.id, &body.new).await?;
    session::delete_for_user(&state.db, rec.id, Some(&me.session_hash)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct PermQuery {
    source: String,
    #[serde(default)]
    path: String,
}

async fn my_permissions(me: AuthUser, Query(q): Query<PermQuery>) -> AppResult<Json<Value>> {
    let path = VPath::parse(&q.path)?;
    Ok(Json(
        json!({ "perms": me.user.effective(&q.source, &path) }),
    ))
}

// ---- preferences ----

/// How folders open in the web UI (the modes of its view switcher).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    /// A table with sortable columns.
    Details,
    /// A compact multi-column list.
    Compact,
    /// Small tiles.
    Small,
    /// Medium tiles with thumbnails.
    Medium,
    /// Large tiles with thumbnails.
    Large,
}

impl ViewMode {
    const ALL: [Self; 5] = [
        Self::Details,
        Self::Compact,
        Self::Small,
        Self::Medium,
        Self::Large,
    ];

    fn parse(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|m| m.as_str() == s)
            .unwrap_or(Self::Details)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Details => "details",
            Self::Compact => "compact",
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }
}

/// A user's preferences (`GET/PUT /api/me/preferences`). The theme has its
/// own endpoint, `/api/me/theme`.
#[derive(Clone, Debug, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    /// Default view mode for folders.
    pub view_mode: ViewMode,
}

async fn get_prefs(State(state): State<AppState>, me: AuthUser) -> AppResult<Json<Preferences>> {
    let mode: Option<String> =
        sqlx::query_scalar("SELECT view_mode FROM user_preferences WHERE user_id = ?")
            .bind(me.user.id)
            .fetch_optional(&state.db)
            .await?;
    Ok(Json(Preferences {
        view_mode: mode.as_deref().map_or(ViewMode::Details, ViewMode::parse),
    }))
}

async fn put_prefs(
    State(state): State<AppState>,
    me: AuthUser,
    Json(p): Json<Preferences>,
) -> AppResult<Json<Preferences>> {
    sqlx::query(
        "INSERT INTO user_preferences (user_id, view_mode, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(user_id) DO UPDATE SET view_mode = ?2, updated_at = ?3",
    )
    .bind(me.user.id)
    .bind(p.view_mode.as_str())
    .bind(crate::db::now())
    .execute(&state.db)
    .await?;
    Ok(Json(p))
}

// ---- users (administrators) ----

async fn list_users(
    State(state): State<AppState>,
    _a: AdminUser,
) -> AppResult<Json<Vec<users::UserRecord>>> {
    Ok(Json(users::list(&state.db).await?))
}

/// Body of `POST /api/users`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewUser {
    /// Login name.
    pub username: String,
    /// Initial password.
    pub password: String,
    /// Role; defaults to `user` (or `admin` with the older `is_admin: true`).
    #[serde(default)]
    pub role: Option<users::Role>,
    /// The older form of `role`.
    #[serde(default)]
    pub is_admin: bool,
    /// Give the user a home folder (see [`HomeBody`]).
    #[serde(default)]
    pub home: Option<HomeBody>,
}

/// A home folder: the user is confined to `path` on `source` (it becomes
/// their root there), the folder is created if missing, and a rule grants
/// the user every permission on it (a read-only role still caps that at
/// view and download).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeBody {
    /// Source to put the folder on.
    pub source: String,
    /// Folder inside the source; defaults to `/home/<username>`.
    #[serde(default)]
    pub path: Option<String>,
}

async fn create_user(
    State(state): State<AppState>,
    _a: AdminUser,
    Json(b): Json<NewUser>,
) -> AppResult<(StatusCode, Json<users::UserRecord>)> {
    let role = match (b.role, b.is_admin) {
        (Some(r), _) => r,
        (None, true) => users::Role::Admin,
        (None, false) => users::Role::User,
    };
    if let Some(h) = &b.home {
        known_source(&state, &h.source)?;
    }
    let u = users::create_with_role(&state.db, &b.username, &b.password, role).await?;
    if let Some(h) = &b.home
        && let Err(e) = assign_home(&state, &u, h).await
    {
        // Do not leave a half-made account behind.
        let _ = users::delete(&state.db, u.id).await;
        return Err(e);
    }
    Ok((StatusCode::CREATED, Json(u)))
}

/// Creates the home folder, sets the scope and grants the user their folder.
async fn assign_home(state: &AppState, u: &users::UserRecord, h: &HomeBody) -> AppResult<VPath> {
    let src = state
        .source(&h.source)
        .ok_or_else(|| AppError::bad_request(format!("unknown source {:?}", h.source)))?;
    let path = match &h.path {
        Some(p) => VPath::parse(p)?,
        None => VPath::parse(&format!("/home/{}", u.username))?,
    };
    if path.is_root() {
        return Err(AppError::bad_request(
            "a home folder cannot be the source root; set no scope instead",
        ));
    }
    src.storage.mkdir(&path).await?;
    users::set_scope(&state.db, u.id, &h.source, &path).await?;
    permissions::set(&state.db, &h.source, &path, Subject::User(u.id), Perms::ALL).await?;
    Ok(path)
}

async fn set_home(
    State(state): State<AppState>,
    _a: AdminUser,
    Path(id): Path<i64>,
    Json(h): Json<HomeBody>,
) -> AppResult<Json<Value>> {
    known_source(&state, &h.source)?;
    let u = users::get(&state.db, id).await?;
    let path = assign_home(&state, &u, &h).await?;
    Ok(Json(
        json!({ "source": h.source, "path": path.to_string() }),
    ))
}

async fn get_user(
    State(state): State<AppState>,
    _a: AdminUser,
    Path(id): Path<i64>,
) -> AppResult<Json<users::UserRecord>> {
    Ok(Json(users::get(&state.db, id).await?))
}

async fn update_user(
    State(state): State<AppState>,
    _a: AdminUser,
    Path(id): Path<i64>,
    Json(b): Json<users::UserUpdate>,
) -> AppResult<Json<users::UserRecord>> {
    Ok(Json(users::update(&state.db, id, &b).await?))
}

async fn delete_user(
    State(state): State<AppState>,
    _a: AdminUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    users::delete(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_scopes(
    State(state): State<AppState>,
    _a: AdminUser,
    Path(id): Path<i64>,
) -> AppResult<Json<BTreeMap<String, String>>> {
    users::get(&state.db, id).await?;
    let s = users::scopes(&state.db, id).await?;
    Ok(Json(
        s.into_iter().map(|(k, v)| (k, v.to_string())).collect(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopeBody {
    source: String,
    scope: String,
}

fn known_source(state: &AppState, source: &str) -> AppResult<()> {
    if state.source(source).is_none() {
        return Err(AppError::bad_request(format!("unknown source {source:?}")));
    }
    Ok(())
}

async fn set_scope(
    State(state): State<AppState>,
    _a: AdminUser,
    Path(id): Path<i64>,
    Json(b): Json<ScopeBody>,
) -> AppResult<Json<BTreeMap<String, String>>> {
    known_source(&state, &b.source)?;
    let scope = VPath::parse(&b.scope)?;
    users::set_scope(&state.db, id, &b.source, &scope).await?;
    let s = users::scopes(&state.db, id).await?;
    Ok(Json(
        s.into_iter().map(|(k, v)| (k, v.to_string())).collect(),
    ))
}

async fn user_groups(
    State(state): State<AppState>,
    _a: AdminUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Vec<groups::Group>>> {
    users::get(&state.db, id).await?;
    Ok(Json(groups::for_user(&state.db, id).await?))
}

// ---- groups (administrators) ----

async fn list_groups(
    State(state): State<AppState>,
    _a: AdminUser,
) -> AppResult<Json<Vec<groups::Group>>> {
    Ok(Json(groups::list(&state.db).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewGroup {
    name: String,
}

async fn create_group(
    State(state): State<AppState>,
    _a: AdminUser,
    Json(b): Json<NewGroup>,
) -> AppResult<(StatusCode, Json<groups::Group>)> {
    Ok((
        StatusCode::CREATED,
        Json(groups::create(&state.db, &b.name).await?),
    ))
}

async fn delete_group(
    State(state): State<AppState>,
    _a: AdminUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    groups::delete(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn group_members(
    State(state): State<AppState>,
    _a: AdminUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Vec<users::UserRecord>>> {
    Ok(Json(groups::members(&state.db, id).await?))
}

async fn add_member(
    State(state): State<AppState>,
    _a: AdminUser,
    Path((id, user_id)): Path<(i64, i64)>,
) -> AppResult<StatusCode> {
    groups::add_member(&state.db, id, user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_member(
    State(state): State<AppState>,
    _a: AdminUser,
    Path((id, user_id)): Path<(i64, i64)>,
) -> AppResult<StatusCode> {
    groups::remove_member(&state.db, id, user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- permission rules (administrators) ----

#[derive(Deserialize)]
struct RuleQuery {
    source: Option<String>,
}

async fn list_rules(
    State(state): State<AppState>,
    _a: AdminUser,
    Query(q): Query<RuleQuery>,
) -> AppResult<Json<Vec<permissions::Rule>>> {
    Ok(Json(
        permissions::list(&state.db, q.source.as_deref()).await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleBody {
    source: String,
    #[serde(default)]
    path: String,
    subject: Subject,
    perms: Perms,
}

async fn set_rule(
    State(state): State<AppState>,
    _a: AdminUser,
    Json(b): Json<RuleBody>,
) -> AppResult<Json<permissions::Rule>> {
    known_source(&state, &b.source)?;
    let path = VPath::parse(&b.path)?;
    Ok(Json(
        permissions::set(&state.db, &b.source, &path, b.subject, b.perms).await?,
    ))
}

async fn delete_rule(
    State(state): State<AppState>,
    _a: AdminUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    permissions::delete(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
