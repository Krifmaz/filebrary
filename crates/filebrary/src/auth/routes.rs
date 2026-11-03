//! Authentication and administration endpoints, merged under `/api`.
//!
//! | Method & path | Who | Body / query | Answer |
//! |---|---|---|---|
//! | `POST /auth/login` | anyone | `{username, password, totp?}` | `{user, csrf_token}` + session cookie |
//! | `POST /auth/logout` | user | | 204, cookie cleared |
//! | `GET /me` | user | | `{user, csrf_token, groups, sources}` |
//! | `POST /me/password` | user | `{current, new}` | 204; other sessions end |
//! | `GET /me/permissions` | user | `?source=&path=` | `{perms}` at that path |
//! | `GET /users` · `POST /users` | admin | `{username, password, is_admin?}` | users / user |
//! | `GET/PUT/DELETE /users/{id}` | admin | `PUT`: [`users::UserUpdate`] | user / 204 |
//! | `GET/PUT /users/{id}/scopes` | admin | `PUT`: `{source, scope}` | `{source: scope}` |
//! | `GET /users/{id}/groups` | admin | | groups |
//! | `GET /groups` · `POST /groups` | admin | `{name}` | groups / group |
//! | `DELETE /groups/{id}` | admin | | 204 |
//! | `GET /groups/{id}/members` | admin | | users |
//! | `PUT/DELETE /groups/{id}/members/{user_id}` | admin | | 204 |
//! | `GET /permissions` · `PUT /permissions` | admin | `?source=` / `{source, path, subject, perms}` | rules / rule |
//! | `DELETE /permissions/{id}` | admin | | 204 |
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
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/me", get(me))
        .route("/me/password", post(change_password))
        .route("/me/permissions", get(my_permissions))
        .route("/users", get(list_users).post(create_user))
        .route(
            "/users/{id}",
            get(get_user).put(update_user).delete(delete_user),
        )
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
        "auth_source": u.auth_source,
    })
}

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
    #[serde(default)]
    totp: Option<String>,
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
    let (rec, hash) = match found {
        Some((rec, Some(hash))) if rec.auth_source == "local" => (Some(rec), hash),
        _ => (None, password::dummy_hash().await),
    };
    let ok = password::verify(body.password, hash).await;
    let rec = match rec {
        Some(r) if ok && !r.disabled => r,
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
    let (sess, cookie) = session::issue(
        &state,
        rec.id,
        client.ip.as_deref(),
        client.user_agent.as_deref(),
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

// ---- users (administrators) ----

async fn list_users(
    State(state): State<AppState>,
    _a: AdminUser,
) -> AppResult<Json<Vec<users::UserRecord>>> {
    Ok(Json(users::list(&state.db).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewUser {
    username: String,
    password: String,
    #[serde(default)]
    is_admin: bool,
}

async fn create_user(
    State(state): State<AppState>,
    _a: AdminUser,
    Json(b): Json<NewUser>,
) -> AppResult<(StatusCode, Json<users::UserRecord>)> {
    let u = users::create(&state.db, &b.username, &b.password, b.is_admin).await?;
    Ok((StatusCode::CREATED, Json(u)))
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
