//! Endpoints for single sign-on, two-factor login, API keys and quotas,
//! merged under `/api`.
//!
//! | Method & path | Who | Body / query | Answer |
//! |---|---|---|---|
//! | `GET /auth/methods` | anyone | | `{password, oidc: {label}?, ldap}` |
//! | `GET /auth/oidc/login` | anyone | `?return_to=/path` | 303 to the provider |
//! | `GET /auth/oidc/callback` | provider | `?code=&state=` | 303 to `return_to` with a session cookie, or to `/login?error=sso_failed` |
//! | `GET /me/totp` | session | | `{enabled, recovery_codes_left}` |
//! | `POST /me/totp/setup` | session | | `{secret, uri}` (2FA not on yet) |
//! | `POST /me/totp/confirm` | session | `{code}` | `{recovery_codes}` (2FA now on) |
//! | `POST /me/totp/recovery-codes` | session | `{code}` | `{recovery_codes}` (old ones void) |
//! | `POST /me/totp/disable` | session | `{code}` | 204 |
//! | `DELETE /users/{id}/totp` | admin | | 204 (reset a lost device) |
//! | `GET /me/api-keys` · `POST /me/api-keys` | session | `{name, scopes, expires_in_days?}` | keys / key with `token` (shown once) |
//! | `DELETE /me/api-keys/{id}` | session | | 204 |
//! | `GET /users/{id}/api-keys` | admin | | keys |
//! | `DELETE /api-keys/{id}` | admin | | 204 |
//! | `GET /me/quota` | user | | `{limit, used, free}` |
//! | `GET /users/{id}/quota` · `PUT /users/{id}/quota` | admin | `{limit: bytes \| "10G" \| null}` | quota |
//! | `POST /users/{id}/quota/rescan` | admin | | quota after measuring |
//!
//! "session" endpoints refuse API keys: a key cannot mint keys or change
//! the account's second factor. Codes in bodies are 6-digit TOTP codes or,
//! where a current code proves possession, also recovery codes.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::{delete, get, post},
};
use axum_extra::extract::CookieJar;
use filebrary_identity::{
    apikey,
    identities::Provider,
    quota::{self, Quota},
    totp,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{ExternalAccount, provision};
use crate::{
    auth::{AdminUser, AuthUser, session, users},
    db,
    error::{AppError, AppResult},
    state::AppState,
};

/// Routes for this module, merged under `/api`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/methods", get(methods))
        .route("/auth/oidc/login", get(oidc_login))
        .route("/auth/oidc/callback", get(oidc_callback))
        .route("/me/totp", get(totp_status))
        .route("/me/totp/setup", post(totp_setup))
        .route("/me/totp/confirm", post(totp_confirm))
        .route("/me/totp/recovery-codes", post(totp_recovery))
        .route("/me/totp/disable", post(totp_disable))
        .route("/users/{id}/totp", delete(admin_totp_reset))
        .route("/me/api-keys", get(list_keys).post(create_key))
        .route("/me/api-keys/{id}", delete(revoke_key))
        .route("/users/{id}/api-keys", get(admin_list_keys))
        .route("/api-keys/{id}", delete(admin_revoke_key))
        .route("/me/quota", get(my_quota))
        .route("/users/{id}/quota", get(get_quota).put(set_quota))
        .route("/users/{id}/quota/rescan", post(rescan_quota))
}

/// Refuses API-key requests on endpoints that manage credentials.
fn require_session(me: &AuthUser) -> AppResult<()> {
    if me.scopes.is_some() {
        return Err(AppError::forbidden(
            "this endpoint needs a browser session, not an API key",
        ));
    }
    Ok(())
}

async fn methods(State(state): State<AppState>) -> Json<Value> {
    let oidc = state.identity.oidc_config();
    Json(json!({
        "password": !oidc.is_some_and(|o| o.disable_password_login),
        "oidc": oidc.map(|o| json!({ "label": o.button_label })),
        "ldap": state.config.ldap.is_some(),
    }))
}

// ---------------------------------------------------------------- OIDC

#[derive(Deserialize)]
struct LoginQuery {
    return_to: Option<String>,
}

const SSO_FAILED: &str = "/login?error=sso_failed";

async fn oidc_login(State(state): State<AppState>, Query(q): Query<LoginQuery>) -> Response {
    if state.identity.oidc_config().is_none() {
        return AppError::NotFound.into_response();
    }
    let started = async {
        let p = state
            .identity
            .oidc_provider()
            .await?
            .ok_or(AppError::NotFound)?;
        p.begin(&state.db, q.return_to.as_deref())
            .await
            .map_err(AppError::from)
    };
    match started.await {
        Ok(url) => Redirect::to(&url).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "could not start single sign-on");
            Redirect::to(SSO_FAILED).into_response()
        }
    }
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn oidc_callback(
    State(state): State<AppState>,
    jar: CookieJar,
    client: ClientMeta,
    Query(q): Query<CallbackQuery>,
) -> Response {
    match finish_oidc(&state, &client, q).await {
        Ok((cookie, to)) => (jar.add(cookie), Redirect::to(&to)).into_response(),
        Err(e) => {
            tracing::info!(error = %e, "single sign-on failed");
            Redirect::to(SSO_FAILED).into_response()
        }
    }
}

async fn finish_oidc(
    state: &AppState,
    client: &ClientMeta,
    q: CallbackQuery,
) -> AppResult<(axum_extra::extract::cookie::Cookie<'static>, String)> {
    if let Some(err) = q.error {
        return Err(AppError::bad_request(format!("provider said: {err}")));
    }
    let (Some(code), Some(st)) = (q.code, q.state) else {
        return Err(AppError::bad_request("missing code or state"));
    };
    let p = state
        .identity
        .oidc_provider()
        .await?
        .ok_or(AppError::NotFound)?;
    let id = p.finish(&state.db, &code, &st).await?;
    let cfg = p.config();
    let ext = ExternalAccount {
        provider: Provider::Oidc,
        issuer: id.issuer.clone(),
        subject: id.subject.clone(),
        username: id.username.clone(),
        admin: cfg.admin_group.as_ref().map(|_| id.is_admin),
        groups: id.groups.clone(),
        auto_create: cfg.auto_create,
    };
    let rec = provision(state, &ext).await?;
    if rec.disabled {
        return Err(AppError::InvalidCredentials);
    }
    let (_, cookie) = session::issue(
        state,
        rec.id,
        client.ip.as_deref(),
        client.user_agent.as_deref(),
    )
    .await?;
    tracing::info!(
        user = rec.id,
        ip = client.ip.as_deref().unwrap_or("unknown"),
        "login via single sign-on"
    );
    Ok((cookie, id.return_to.unwrap_or_else(|| "/".to_owned())))
}

/// Client address and user agent for the session record.
struct ClientMeta {
    ip: Option<String>,
    user_agent: Option<String>,
}

impl axum::extract::FromRequestParts<AppState> for ClientMeta {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self {
            ip: crate::auth::client_ip(parts, &state.config),
            user_agent: parts
                .headers
                .get(axum::http::header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.chars().take(512).collect()),
        })
    }
}

// ---------------------------------------------------------------- TOTP

#[derive(Deserialize)]
struct CodeBody {
    code: String,
}

async fn totp_status(State(state): State<AppState>, me: AuthUser) -> AppResult<Json<totp::Status>> {
    Ok(Json(totp::status(&state.db, me.user.id).await?))
}

async fn totp_setup(
    State(state): State<AppState>,
    me: AuthUser,
) -> AppResult<Json<totp::Enrollment>> {
    require_session(&me)?;
    if me.user.auth_source == Provider::Oidc.as_str() {
        return Err(AppError::bad_request(
            "single sign-on accounts use the provider's two-factor settings",
        ));
    }
    Ok(Json(
        totp::begin_enrollment(
            &state.db,
            &state.secret,
            me.user.id,
            &state.config.totp_issuer,
            &me.user.username,
        )
        .await?,
    ))
}

async fn totp_confirm(
    State(state): State<AppState>,
    me: AuthUser,
    Json(b): Json<CodeBody>,
) -> AppResult<Json<Value>> {
    require_session(&me)?;
    let codes = totp::confirm_enrollment(&state.db, &state.secret, me.user.id, &b.code).await?;
    tracing::info!(user = me.user.id, "two-factor authentication switched on");
    Ok(Json(json!({ "recovery_codes": codes })))
}

async fn totp_recovery(
    State(state): State<AppState>,
    me: AuthUser,
    Json(b): Json<CodeBody>,
) -> AppResult<Json<Value>> {
    require_session(&me)?;
    totp::verify_login(&state.db, &state.secret, me.user.id, &b.code).await?;
    let codes = totp::regenerate_recovery_codes(&state.db, me.user.id).await?;
    Ok(Json(json!({ "recovery_codes": codes })))
}

async fn totp_disable(
    State(state): State<AppState>,
    me: AuthUser,
    Json(b): Json<CodeBody>,
) -> AppResult<StatusCode> {
    require_session(&me)?;
    totp::verify_login(&state.db, &state.secret, me.user.id, &b.code).await?;
    totp::disable(&state.db, me.user.id).await?;
    tracing::info!(user = me.user.id, "two-factor authentication switched off");
    Ok(StatusCode::NO_CONTENT)
}

async fn admin_totp_reset(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    users::get(&state.db, id).await?;
    totp::disable(&state.db, id).await?;
    tracing::info!(
        user = id,
        by = admin.user.id,
        "two-factor authentication reset by an administrator"
    );
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- API keys

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewKeyBody {
    name: String,
    scopes: Vec<String>,
    #[serde(default)]
    expires_in_days: Option<u32>,
}

async fn list_keys(
    State(state): State<AppState>,
    me: AuthUser,
) -> AppResult<Json<Vec<apikey::KeyInfo>>> {
    require_session(&me)?;
    Ok(Json(apikey::list(&state.db, me.user.id).await?))
}

async fn create_key(
    State(state): State<AppState>,
    me: AuthUser,
    Json(b): Json<NewKeyBody>,
) -> AppResult<(StatusCode, Json<apikey::NewKey>)> {
    require_session(&me)?;
    let scopes = apikey::parse_scopes(&b.scopes)?;
    let expires_at = match b.expires_in_days {
        Some(0) => return Err(AppError::bad_request("expires_in_days must be at least 1")),
        Some(d) => Some(db::now() + i64::from(d) * 86_400),
        None => None,
    };
    let key = apikey::create(
        &state.db,
        me.user.id,
        me.user.is_admin,
        &b.name,
        &scopes,
        expires_at,
    )
    .await?;
    tracing::info!(user = me.user.id, key = %key.info.id, scopes = %key.info.scopes, "API key created");
    Ok((StatusCode::CREATED, Json(key)))
}

async fn revoke_key(
    State(state): State<AppState>,
    me: AuthUser,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    require_session(&me)?;
    apikey::revoke(&state.db, Some(me.user.id), &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn admin_list_keys(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Vec<apikey::KeyInfo>>> {
    Ok(Json(apikey::list(&state.db, id).await?))
}

async fn admin_revoke_key(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    apikey::revoke(&state.db, None, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- quotas

fn quota_json(q: Quota) -> Json<Value> {
    Json(json!({ "limit": q.limit, "used": q.used, "free": q.free() }))
}

async fn my_quota(State(state): State<AppState>, me: AuthUser) -> AppResult<Json<Value>> {
    Ok(quota_json(
        quota::get(&state.db, me.user.id, state.identity.default_quota()).await?,
    ))
}

async fn get_quota(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    users::get(&state.db, id).await?;
    Ok(quota_json(
        quota::get(&state.db, id, state.identity.default_quota()).await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QuotaBody {
    /// Bytes, a size string such as "10G", or null for unlimited.
    limit: Option<Value>,
}

async fn set_quota(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<i64>,
    Json(b): Json<QuotaBody>,
) -> AppResult<Json<Value>> {
    users::get(&state.db, id).await?;
    let limit = match b.limit {
        None | Some(Value::Null) => None,
        Some(Value::Number(n)) => Some(
            n.as_i64()
                .filter(|v| *v >= 0)
                .ok_or_else(|| AppError::bad_request("limit must be a whole number of bytes"))?,
        ),
        Some(Value::String(s)) => Some(
            quota::parse_size(&s)
                .ok_or_else(|| AppError::bad_request(format!("{s:?} is not a size such as 10G")))?,
        ),
        Some(_) => return Err(AppError::bad_request("limit must be bytes, a size or null")),
    };
    quota::set_limit(&state.db, id, limit).await?;
    Ok(quota_json(
        quota::get(&state.db, id, state.identity.default_quota()).await?,
    ))
}

async fn rescan_quota(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    users::get(&state.db, id).await?;
    let default = state.identity.default_quota();
    if let Some(bytes) = super::measure_usage(&state, id).await? {
        quota::set_used(&state.db, id, bytes, default).await?;
    }
    Ok(quota_json(quota::get(&state.db, id, default).await?))
}
