//! Themes and branding.
//!
//! *Branding* is server-wide and set by administrators: the product name shown
//! in the UI and the installed PWA, a logo, the default theme, and colour
//! overrides for the light, dark and custom themes. *Theme* is each user's
//! choice of `light`, `dark`, `system` or `custom` (with their own colours).
//!
//! Colours are CSS custom properties from a fixed list ([`THEME_VARS`]) and
//! values must be `#rgb` or `#rrggbb`, so nothing an administrator or user
//! types can inject CSS.
//!
//! | Method   | Path                          | Who     | What                          |
//! |----------|-------------------------------|---------|-------------------------------|
//! | `GET`    | `/api/branding`               | anyone  | Branding (login page needs it)|
//! | `GET`    | `/api/branding/logo`          | anyone  | The logo image                |
//! | `PUT`    | `/api/admin/branding`         | admin   | Replace branding settings     |
//! | `PUT`    | `/api/admin/branding/logo`    | admin   | Upload a logo (raw body)      |
//! | `DELETE` | `/api/admin/branding/logo`    | admin   | Remove the logo               |
//! | `GET`    | `/api/me/theme`               | user    | The caller's theme            |
//! | `PUT`    | `/api/me/theme`               | user    | Set the caller's theme        |
//! | `GET`    | `/manifest.webmanifest`       | anyone  | PWA manifest with branding    |

use std::collections::BTreeMap;

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, put},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    auth::{AdminUser, AuthUser},
    db,
    error::{AppError, AppResult},
    state::AppState,
};

/// CSS custom properties that themes may set (defined in `web/src/styles.css`).
pub const THEME_VARS: [&str; 9] = [
    "--bg",
    "--surface",
    "--surface-2",
    "--text",
    "--muted",
    "--border",
    "--accent",
    "--accent-contrast",
    "--danger",
];

/// Largest logo accepted, in bytes.
pub const MAX_LOGO_BYTES: usize = 512 * 1024;
/// Longest product name, in characters.
pub const MAX_NAME_CHARS: usize = 60;
/// Default product name.
pub const DEFAULT_NAME: &str = "Filebrary";

/// A theme choice.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ThemeName {
    /// Light colours.
    Light,
    /// Dark colours.
    Dark,
    /// Follow the operating system.
    #[default]
    System,
    /// The custom palette.
    Custom,
}

impl ThemeName {
    fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::System => "system",
            Self::Custom => "custom",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "light" => Self::Light,
            "dark" => Self::Dark,
            "custom" => Self::Custom,
            _ => Self::System,
        }
    }
}

/// CSS variable name → colour.
pub type Palette = BTreeMap<String, String>;

/// Branding settings as stored and as sent by administrators.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct BrandingSettings {
    /// Product name shown in the header, the page title and the PWA.
    pub name: String,
    /// Theme for users who have not picked one, and for public share pages.
    pub default_theme: ThemeName,
    /// Overrides applied to the light theme.
    pub light: Palette,
    /// Overrides applied to the dark theme.
    pub dark: Palette,
    /// The server-wide custom theme (users may override it with their own).
    pub custom: Palette,
}

/// Branding as served at `GET /api/branding`.
#[derive(Debug, Serialize)]
pub struct Branding {
    /// Settings (with the default name filled in).
    #[serde(flatten)]
    pub settings: BrandingSettings,
    /// URL of the logo, with a version parameter for caching, if one is set.
    pub logo_url: Option<String>,
}

/// A user's theme choice.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct UserTheme {
    /// The theme.
    pub theme: ThemeName,
    /// Personal custom colours (used with `custom`, on top of the server's).
    pub colours: Palette,
}

/// Routes merged into `/api`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/branding", get(get_branding))
        .route("/branding/logo", get(get_logo))
        .route("/admin/branding", put(put_branding))
        .route("/admin/branding/logo", put(put_logo).delete(delete_logo))
        .route("/me/theme", get(get_theme).put(put_theme))
}

/// Checks a colour: `#rgb` or `#rrggbb`.
pub fn valid_colour(c: &str) -> bool {
    let Some(hex) = c.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 6) && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Checks every entry of a palette against [`THEME_VARS`] and [`valid_colour`].
///
/// # Errors
///
/// `BadRequest` naming the first offending entry.
pub fn validate_palette(p: &Palette) -> AppResult<()> {
    for (k, v) in p {
        if !THEME_VARS.contains(&k.as_str()) {
            return Err(AppError::bad_request(format!(
                "unknown theme colour {k:?}; use one of {}",
                THEME_VARS.join(", ")
            )));
        }
        if !valid_colour(v) {
            return Err(AppError::bad_request(format!(
                "colour for {k} must look like #1a2b3c"
            )));
        }
    }
    Ok(())
}

fn validate_settings(s: &mut BrandingSettings) -> AppResult<()> {
    s.name = s.name.trim().to_owned();
    if s.name.chars().count() > MAX_NAME_CHARS {
        return Err(AppError::bad_request(format!(
            "the name is limited to {MAX_NAME_CHARS} characters"
        )));
    }
    if s.name.chars().any(char::is_control) {
        return Err(AppError::bad_request(
            "the name contains control characters",
        ));
    }
    validate_palette(&s.light)?;
    validate_palette(&s.dark)?;
    validate_palette(&s.custom)
}

/// Loads branding settings and the logo version (its update time), if any.
///
/// # Errors
///
/// Database failures. A corrupt stored value falls back to the defaults.
pub async fn load(state: &AppState) -> AppResult<(BrandingSettings, Option<i64>)> {
    let row: Option<(String, bool, i64)> =
        sqlx::query_as("SELECT data, logo IS NOT NULL, updated_at FROM branding WHERE id = 1")
            .fetch_optional(&state.db)
            .await?;
    let (mut settings, logo) = match row {
        Some((data, has_logo, updated)) => (
            serde_json::from_str(&data).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "ignoring unreadable branding settings");
                BrandingSettings::default()
            }),
            has_logo.then_some(updated),
        ),
        None => (BrandingSettings::default(), None),
    };
    if settings.name.is_empty() {
        settings.name = DEFAULT_NAME.to_owned();
    }
    Ok((settings, logo))
}

/// `GET /api/branding`.
async fn get_branding(State(state): State<AppState>) -> AppResult<Json<Branding>> {
    let (settings, logo) = load(&state).await?;
    Ok(Json(Branding {
        settings,
        logo_url: logo.map(|v| format!("/api/branding/logo?v={v}")),
    }))
}

/// `PUT /api/admin/branding`.
async fn put_branding(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(mut s): Json<BrandingSettings>,
) -> AppResult<Json<Branding>> {
    validate_settings(&mut s)?;
    let data = serde_json::to_string(&s).map_err(anyhow::Error::from)?;
    sqlx::query(
        "INSERT INTO branding (id, data, updated_at) VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET data = ?1, updated_at = ?2",
    )
    .bind(data)
    .bind(db::now())
    .execute(&state.db)
    .await?;
    tracing::info!(user = admin.user.id, "branding updated");
    get_branding(State(state)).await
}

/// Detects the image type of a logo from its bytes (never trusting the client).
fn sniff_logo(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("image/gif")
    } else {
        let text = std::str::from_utf8(data).ok()?;
        let t = text.trim_start_matches('\u{feff}').trim_start();
        (t.starts_with("<svg") || (t.starts_with("<?xml") && t.contains("<svg")))
            .then_some("image/svg+xml")
    }
}

/// `PUT /api/admin/branding/logo`: the raw image is the request body.
async fn put_logo(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    body: Bytes,
) -> AppResult<StatusCode> {
    if body.len() > MAX_LOGO_BYTES {
        return Err(AppError::PayloadTooLarge);
    }
    let mime = sniff_logo(&body).ok_or_else(|| {
        AppError::bad_request("the logo must be a PNG, JPEG, WebP, GIF or SVG image")
    })?;
    sqlx::query(
        "INSERT INTO branding (id, logo, logo_mime, updated_at) VALUES (1, ?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET logo = ?1, logo_mime = ?2, updated_at = ?3",
    )
    .bind(body.as_ref())
    .bind(mime)
    .bind(db::now())
    .execute(&state.db)
    .await?;
    tracing::info!(user = admin.user.id, mime, "logo updated");
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/admin/branding/logo`.
async fn delete_logo(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
) -> AppResult<StatusCode> {
    sqlx::query("UPDATE branding SET logo = NULL, logo_mime = NULL, updated_at = ? WHERE id = 1")
        .bind(db::now())
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/branding/logo`.
async fn get_logo(State(state): State<AppState>) -> AppResult<Response> {
    let row: Option<(Option<Vec<u8>>, Option<String>)> =
        sqlx::query_as("SELECT logo, logo_mime FROM branding WHERE id = 1")
            .fetch_optional(&state.db)
            .await?;
    let Some((Some(data), Some(mime))) = row else {
        return Err(AppError::NotFound);
    };
    let mut resp = data.into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&mime).map_err(anyhow::Error::from)?,
    );
    // The URL carries a version, so it can be cached; an SVG opened directly
    // must never run scripts in our origin.
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; sandbox"),
    );
    Ok(resp)
}

/// `GET /api/me/theme`: the caller's theme (the server default if unset).
async fn get_theme(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<UserTheme>> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT theme, colours FROM user_themes WHERE user_id = ?")
            .bind(auth.user.id)
            .fetch_optional(&state.db)
            .await?;
    Ok(Json(match row {
        Some((theme, colours)) => UserTheme {
            theme: ThemeName::parse(&theme),
            colours: serde_json::from_str(&colours).unwrap_or_default(),
        },
        None => UserTheme {
            theme: load(&state).await?.0.default_theme,
            colours: Palette::new(),
        },
    }))
}

/// `PUT /api/me/theme`.
async fn put_theme(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(t): Json<UserTheme>,
) -> AppResult<Json<UserTheme>> {
    validate_palette(&t.colours)?;
    let colours = serde_json::to_string(&t.colours).map_err(anyhow::Error::from)?;
    sqlx::query(
        "INSERT INTO user_themes (user_id, theme, colours, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(user_id) DO UPDATE SET theme = ?2, colours = ?3, updated_at = ?4",
    )
    .bind(auth.user.id)
    .bind(t.theme.as_str())
    .bind(colours)
    .bind(db::now())
    .execute(&state.db)
    .await?;
    Ok(Json(t))
}

/// `GET /manifest.webmanifest`: the web UI's manifest with the branding name,
/// colours and (if it is a raster image) the logo as an icon.
pub async fn manifest(State(state): State<AppState>) -> AppResult<Response> {
    let (s, logo) = load(&state).await?;
    let mut m: Value = crate::web::embedded("manifest.webmanifest")
        .and_then(|b| serde_json::from_slice(&b).ok())
        .filter(Value::is_object)
        .unwrap_or_else(
            || json!({ "start_url": "/", "scope": "/", "display": "standalone", "icons": [] }),
        );
    let short: String = s.name.chars().take(12).collect();
    m["name"] = json!(s.name);
    m["short_name"] = json!(short);
    let accent = s.light.get("--accent").cloned();
    let bg = s.light.get("--bg").cloned();
    if let Some(a) = accent {
        m["theme_color"] = json!(a);
    }
    if let Some(b) = bg {
        m["background_color"] = json!(b);
    }
    if let Some(v) = logo {
        let mime: Option<String> =
            sqlx::query_scalar("SELECT logo_mime FROM branding WHERE id = 1")
                .fetch_optional(&state.db)
                .await?
                .flatten();
        if let Some(mime) = mime.filter(|m| m != "image/gif") {
            let icon = json!({ "src": format!("/api/branding/logo?v={v}"), "type": mime,
                               "sizes": "any", "purpose": "any" });
            match m["icons"].as_array_mut() {
                Some(icons) => icons.insert(0, icon),
                None => m["icons"] = json!([icon]),
            }
        }
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/manifest+json"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok((headers, m.to_string()).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours() {
        for ok in ["#fff", "#A1b2C3"] {
            assert!(valid_colour(ok), "{ok}");
        }
        for bad in ["fff", "#ffff", "#ggg", "red", "#fff;}", "#fff\n", ""] {
            assert!(!valid_colour(bad), "{bad}");
        }
    }

    #[test]
    fn palettes() {
        let mut p = Palette::new();
        p.insert("--accent".into(), "#336699".into());
        validate_palette(&p).unwrap();
        p.insert("--evil".into(), "#000".into());
        assert!(validate_palette(&p).is_err());
        let mut q = Palette::new();
        q.insert("--bg".into(), "url(x)".into());
        assert!(validate_palette(&q).is_err());
    }

    #[test]
    fn settings_validation() {
        let mut s = BrandingSettings {
            name: "  Acme Files ".into(),
            ..Default::default()
        };
        validate_settings(&mut s).unwrap();
        assert_eq!(s.name, "Acme Files");
        s.name = "x".repeat(MAX_NAME_CHARS + 1);
        assert!(validate_settings(&mut s).is_err());
        assert!(serde_json::from_str::<BrandingSettings>(r#"{"nmae":"x"}"#).is_err());
    }

    #[test]
    fn logo_sniffing() {
        assert_eq!(sniff_logo(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(sniff_logo(b"\xff\xd8\xff\xe0"), Some("image/jpeg"));
        assert_eq!(sniff_logo(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_logo(b"  <svg xmlns='x'/>"), Some("image/svg+xml"));
        assert_eq!(
            sniff_logo(b"<?xml version='1.0'?><svg/>"),
            Some("image/svg+xml")
        );
        assert_eq!(sniff_logo(b"<html><script>"), None);
        assert_eq!(sniff_logo(b"MZ\x90\0"), None);
    }

    #[test]
    fn theme_names() {
        for t in [
            ThemeName::Light,
            ThemeName::Dark,
            ThemeName::System,
            ThemeName::Custom,
        ] {
            assert_eq!(ThemeName::parse(t.as_str()), t);
        }
        assert_eq!(ThemeName::parse("neon"), ThemeName::System);
    }
}
