//! OpenID Connect single sign-on (Authelia, Authentik, Keycloak, Zitadel,
//! Google, Microsoft Entra, ...).
//!
//! The authorization code flow with PKCE (S256), a random `state` and a random
//! `nonce`:
//!
//! 1. [`OidcProvider::begin`] builds the provider URL and stores the PKCE
//!    verifier and nonce server side under a hash of `state` (10 minute life).
//! 2. The provider redirects back with `code` and `state`;
//!    [`OidcProvider::finish`] takes the stored login (single use), exchanges
//!    the code, and verifies the ID token's signature, issuer, audience,
//!    expiry and nonce.
//! 3. The caller maps the returned [`OidcIdentity`] to a local user through
//!    [`crate::identities`], keyed by issuer and `sub`.
//!
//! Group claims (for `admin_group` and `allowed_groups`) are read from the
//! verified ID token and, if absent there, from the userinfo endpoint.

use std::collections::BTreeMap;

use base64::Engine as _;
use openidconnect::{
    AdditionalClaims, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet,
    EndpointNotSet, EndpointSet, IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, UserInfoClaims,
    core::{CoreAuthenticationFlow, CoreClient, CoreGenderClaim, CoreProviderMetadata},
};
use serde::Deserialize;
use serde_json::Value;
use sqlx::SqlitePool;

use crate::{Error, Result, now, random_bytes, sha256};

/// How long a started login stays valid, in seconds.
pub const PENDING_TTL_SECS: i64 = 600;

/// OpenID Connect settings (`[auth.oidc]` in the config file).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OidcConfig {
    /// Issuer URL, e.g. `https://auth.example.com` (discovery is at
    /// `/.well-known/openid-configuration` below it).
    pub issuer_url: String,
    /// Client id registered at the provider.
    pub client_id: String,
    /// Client secret. Prefer `client_secret_file`.
    #[serde(default)]
    pub client_secret: Option<crate::SecretString>,
    /// File holding the client secret (for Docker secrets).
    #[serde(default)]
    pub client_secret_file: Option<std::path::PathBuf>,
    /// Scopes to request besides `openid`.
    #[serde(default = "default_scopes")]
    pub scopes: Vec<String>,
    /// Claim used as the local username.
    #[serde(default = "default_username_claim")]
    pub username_claim: String,
    /// Claim listing the user's groups.
    #[serde(default = "default_groups_claim")]
    pub groups_claim: String,
    /// Members of this group are administrators. Checked at every login.
    #[serde(default)]
    pub admin_group: Option<String>,
    /// Only members of one of these groups may log in. Empty: everyone.
    #[serde(default)]
    pub allowed_groups: Vec<String>,
    /// Create a local user on first login. Off: an administrator must create
    /// the user first (matched by username on first login, then by `sub`).
    #[serde(default = "yes")]
    pub auto_create: bool,
    /// Label on the login button.
    #[serde(default = "default_button")]
    pub button_label: String,
    /// Hide the password form when SSO is configured.
    #[serde(default)]
    pub disable_password_login: bool,
}

fn default_scopes() -> Vec<String> {
    vec!["profile".into(), "email".into(), "groups".into()]
}
fn default_username_claim() -> String {
    "preferred_username".into()
}
fn default_groups_claim() -> String {
    "groups".into()
}
fn default_button() -> String {
    "Sign in with SSO".into()
}
fn yes() -> bool {
    true
}

impl OidcConfig {
    /// Checks the settings without contacting the provider.
    ///
    /// # Errors
    ///
    /// `InvalidInput` naming the bad setting.
    pub fn validate(&self) -> Result<()> {
        IssuerUrl::new(self.issuer_url.clone())
            .map_err(|e| Error::invalid(format!("oidc.issuer_url: {e}")))?;
        if !self.issuer_url.starts_with("https://") && !is_loopback_url(&self.issuer_url) {
            return Err(Error::invalid(
                "oidc.issuer_url must use https:// (http:// is only allowed for localhost)",
            ));
        }
        if self.client_id.is_empty() {
            return Err(Error::invalid("oidc.client_id is required"));
        }
        if self.client_secret.is_some() && self.client_secret_file.is_some() {
            return Err(Error::invalid(
                "set oidc.client_secret or oidc.client_secret_file, not both",
            ));
        }
        Ok(())
    }

    fn secret(&self) -> Result<Option<String>> {
        crate::secret::read(
            self.client_secret.as_ref(),
            self.client_secret_file.as_deref(),
        )
        .map_err(|e| Error::Oidc(format!("reading oidc.client_secret_file: {e}")))
    }
}

fn is_loopback_url(u: &str) -> bool {
    ["http://localhost", "http://127.0.0.1", "http://[::1]"]
        .iter()
        .any(|p| {
            u.strip_prefix(p)
                .is_some_and(|r| r.is_empty() || r.starts_with([':', '/']))
        })
}

/// The user the provider vouched for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OidcIdentity {
    /// Issuer URL (part of the stable link to the local user).
    pub issuer: String,
    /// The `sub` claim (the other part).
    pub subject: String,
    /// Suggested local username, from `username_claim`, then `email`, then `sub`.
    pub username: String,
    /// Verified e-mail, if the provider says it is verified.
    pub email: Option<String>,
    /// Display name, if any.
    pub display_name: Option<String>,
    /// Groups from `groups_claim`.
    pub groups: Vec<String>,
    /// Member of `admin_group`.
    pub is_admin: bool,
    /// Where to send the browser after login (as passed to `begin`).
    pub return_to: Option<String>,
}

type Client = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// A discovered provider, ready to start and finish logins.
pub struct OidcProvider {
    cfg: OidcConfig,
    client: Client,
    http: openidconnect::reqwest::Client,
}

impl std::fmt::Debug for OidcProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcProvider")
            .field("issuer", &self.cfg.issuer_url)
            .finish_non_exhaustive()
    }
}

impl OidcProvider {
    /// Fetches the provider's discovery document and keys.
    ///
    /// `redirect_url` is `<public_url>/api/auth/oidc/callback` and must be
    /// registered at the provider.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for bad settings, `Oidc` if discovery fails.
    pub async fn discover(cfg: OidcConfig, redirect_url: &str) -> Result<Self> {
        cfg.validate()?;
        let http = openidconnect::reqwest::ClientBuilder::new()
            // Following redirects here would allow SSRF through the provider.
            .redirect(openidconnect::reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| Error::Oidc(format!("HTTP client: {e}")))?;
        let issuer = IssuerUrl::new(cfg.issuer_url.clone())
            .map_err(|e| Error::invalid(format!("oidc.issuer_url: {e}")))?;
        let meta = CoreProviderMetadata::discover_async(issuer, &http)
            .await
            .map_err(|e| Error::Oidc(format!("discovery failed: {e}")))?;
        let redirect = RedirectUrl::new(redirect_url.to_owned())
            .map_err(|e| Error::invalid(format!("redirect URL: {e}")))?;
        let client = CoreClient::from_provider_metadata(
            meta,
            ClientId::new(cfg.client_id.clone()),
            cfg.secret()?.map(ClientSecret::new),
        )
        .set_redirect_uri(redirect);
        Ok(Self { cfg, client, http })
    }

    /// The settings this provider was built from.
    pub fn config(&self) -> &OidcConfig {
        &self.cfg
    }

    /// Starts a login. Returns the URL to redirect the browser to.
    ///
    /// `return_to` must be a local path (starting with a single `/`); anything
    /// else is dropped so the callback cannot become an open redirect.
    ///
    /// # Errors
    ///
    /// Database errors.
    pub async fn begin(&self, db: &SqlitePool, return_to: Option<&str>) -> Result<String> {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut req = self.client.authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        );
        for s in &self.cfg.scopes {
            if s != "openid" {
                req = req.add_scope(Scope::new(s.clone()));
            }
        }
        let (url, state, nonce) = req.set_pkce_challenge(challenge).url();
        let return_to = return_to.filter(|r| is_local_path(r));
        let t = now();
        sqlx::query("DELETE FROM oidc_pending WHERE created_at < ?")
            .bind(t - PENDING_TTL_SECS)
            .execute(db)
            .await?;
        sqlx::query(
            "INSERT INTO oidc_pending (state_hash, nonce, pkce_verifier, return_to, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(sha256(state.secret().as_bytes()))
        .bind(nonce.secret())
        .bind(verifier.secret())
        .bind(return_to)
        .bind(t)
        .execute(db)
        .await?;
        Ok(url.to_string())
    }

    /// Finishes a login from the callback's `code` and `state`.
    ///
    /// # Errors
    ///
    /// `InvalidCredentials` for an unknown, reused or expired `state`, or a
    /// user outside `allowed_groups`; `Oidc` when the exchange or the token
    /// check fails.
    pub async fn finish(&self, db: &SqlitePool, code: &str, state: &str) -> Result<OidcIdentity> {
        let row: Option<(String, String, Option<String>, i64)> = sqlx::query_as(
            "DELETE FROM oidc_pending WHERE state_hash = ?
             RETURNING nonce, pkce_verifier, return_to, created_at",
        )
        .bind(sha256(state.as_bytes()))
        .fetch_optional(db)
        .await?;
        let Some((nonce, verifier, return_to, created)) = row else {
            return Err(Error::InvalidCredentials);
        };
        if now() - created > PENDING_TTL_SECS {
            return Err(Error::InvalidCredentials);
        }
        let token = self
            .client
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .map_err(|e| Error::Oidc(format!("no token endpoint: {e}")))?
            .set_pkce_verifier(PkceCodeVerifier::new(verifier))
            .request_async(&self.http)
            .await
            .map_err(|e| Error::Oidc(format!("code exchange failed: {e}")))?;
        let id_token = token
            .id_token()
            .ok_or_else(|| Error::Oidc("the provider sent no ID token".into()))?;
        let verifier = self.client.id_token_verifier();
        let claims = id_token
            .claims(&verifier, &Nonce::new(nonce))
            .map_err(|e| Error::Oidc(format!("ID token rejected: {e}")))?;
        // The signature is verified, so the payload's other claims can be trusted.
        let mut raw = jwt_payload(&id_token.to_string()).unwrap_or_default();
        let needs_userinfo = !raw.contains_key(&self.cfg.groups_claim)
            || !raw.contains_key(&self.cfg.username_claim);
        if needs_userinfo
            && let Ok(req) = self.client.user_info(
                token.access_token().to_owned(),
                Some(claims.subject().clone()),
            )
        {
            match req
                .request_async::<ExtraClaims, _, CoreGenderClaim>(&self.http)
                .await
            {
                Ok(info) => merge_userinfo(&mut raw, &info),
                Err(e) => tracing::debug!(error = %e, "userinfo request failed"),
            }
        }
        let ident = build_identity(
            &self.cfg,
            claims.issuer().as_str(),
            claims.subject().as_str(),
            &raw,
            return_to,
        );
        if !self.cfg.allowed_groups.is_empty()
            && !ident
                .groups
                .iter()
                .any(|g| self.cfg.allowed_groups.contains(g))
        {
            tracing::info!(subject = %ident.subject, "SSO login refused: not in allowed_groups");
            return Err(Error::InvalidCredentials);
        }
        Ok(ident)
    }
}

/// Every claim the provider sent that the standard set does not cover
/// (`groups`, custom username claims, ...).
#[derive(Clone, Debug, Default, Deserialize, serde::Serialize)]
struct ExtraClaims {
    #[serde(flatten)]
    rest: BTreeMap<String, Value>,
}

impl AdditionalClaims for ExtraClaims {}

/// Adds userinfo claims that the ID token lacked (the ID token wins on conflicts).
fn merge_userinfo(
    raw: &mut BTreeMap<String, Value>,
    info: &UserInfoClaims<ExtraClaims, CoreGenderClaim>,
) {
    if let Ok(Value::Object(m)) = serde_json::to_value(info) {
        for (k, v) in m {
            raw.entry(k).or_insert(v);
        }
    }
    for (k, v) in &info.additional_claims().rest {
        raw.entry(k.clone()).or_insert_with(|| v.clone());
    }
}

/// Whether `p` is a path on this site (no scheme, no `//host`, no backslash).
pub fn is_local_path(p: &str) -> bool {
    p.starts_with('/')
        && !p.starts_with("//")
        && !p.contains('\\')
        && !p.chars().any(char::is_control)
        && p.len() <= 2048
}

fn jwt_payload(jwt: &str) -> Option<BTreeMap<String, Value>> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn claim_str(raw: &BTreeMap<String, Value>, name: &str) -> Option<String> {
    raw.get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn build_identity(
    cfg: &OidcConfig,
    issuer: &str,
    subject: &str,
    raw: &BTreeMap<String, Value>,
    return_to: Option<String>,
) -> OidcIdentity {
    let email_verified = match raw.get("email_verified") {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "true",
        _ => false,
    };
    let email = claim_str(raw, "email").filter(|_| email_verified);
    let username = claim_str(raw, &cfg.username_claim)
        .or_else(|| email.clone())
        .unwrap_or_else(|| subject.to_owned());
    let groups: Vec<String> = match raw.get(&cfg.groups_claim) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        Some(Value::String(s)) => s
            .split([',', ' '])
            .filter(|x| !x.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    };
    let is_admin = cfg.admin_group.as_ref().is_some_and(|g| groups.contains(g));
    OidcIdentity {
        issuer: issuer.to_owned(),
        subject: subject.to_owned(),
        username,
        email,
        display_name: claim_str(raw, "name"),
        groups,
        is_admin,
        return_to,
    }
}

/// A random 16-byte value in hex, handy for tests and placeholders.
#[doc(hidden)]
pub fn random_hex() -> String {
    hex::encode(random_bytes::<16>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> OidcConfig {
        serde_json::from_str(
            r#"{"issuer_url":"https://auth.example.com","client_id":"filebrary",
                "admin_group":"admins"}"#,
        )
        .unwrap()
    }

    #[test]
    fn validation() {
        assert!(cfg().validate().is_ok());
        let mut c = cfg();
        c.issuer_url = "http://auth.example.com".into();
        assert!(c.validate().is_err());
        c.issuer_url = "http://localhost:9000".into();
        assert!(c.validate().is_ok());
        c.issuer_url = "http://localhost.evil.com".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn local_paths_only() {
        assert!(is_local_path("/files/docs?x=1"));
        assert!(!is_local_path("//evil.com"));
        assert!(!is_local_path("/\\evil.com"));
        assert!(!is_local_path("https://evil.com"));
        assert!(!is_local_path("files"));
    }

    #[test]
    fn identity_from_claims() {
        let raw: BTreeMap<String, Value> = serde_json::from_value(serde_json::json!({
            "preferred_username": "bob",
            "email": "bob@example.com",
            "email_verified": true,
            "name": "Bob B",
            "groups": ["users", "admins"]
        }))
        .unwrap();
        let id = build_identity(&cfg(), "https://auth.example.com", "abc", &raw, None);
        assert_eq!(id.username, "bob");
        assert_eq!(id.email.as_deref(), Some("bob@example.com"));
        assert!(id.is_admin);

        let raw: BTreeMap<String, Value> = serde_json::from_value(serde_json::json!({
            "email": "eve@example.com", "groups": "users"
        }))
        .unwrap();
        let id = build_identity(&cfg(), "i", "sub-1", &raw, None);
        // Unverified e-mail is neither trusted nor used as the username.
        assert_eq!(id.email, None);
        assert_eq!(id.username, "sub-1");
        assert!(!id.is_admin);
    }

    #[test]
    fn jwt_payload_decodes() {
        let p = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"sub":"x"}"#);
        let m = jwt_payload(&format!("h.{p}.s")).unwrap();
        assert_eq!(m["sub"], "x");
        assert!(jwt_payload("garbage").is_none());
    }
}
