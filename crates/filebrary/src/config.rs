//! Server configuration.
//!
//! Configuration comes from an optional TOML file, then environment variables
//! (which win), then built-in defaults. Every setting is documented in
//! `docs/src/configuration/reference.md` (checked by
//! `tests/docs_config.rs`).
//!
//! ```toml
//! listen = "0.0.0.0:8080"
//! data_dir = "/data"
//! public_url = "https://files.example.com"
//!
//! [[sources]]
//! name = "files"
//! path = "/srv"
//! ```

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use filebrary_storage::{s3::S3Config, sftp::SftpConfig, smb::SmbConfig};
use serde::Deserialize;

/// Log output format.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Human-readable lines.
    #[default]
    Text,
    /// One JSON object per line, for log collectors.
    Json,
}

/// Settings for the Prometheus `/metrics` endpoint (`[metrics]` table).
#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct MetricsConfig {
    /// Serve `/metrics`. On by default.
    pub enabled: bool,
    /// If set, scrapers must send `Authorization: Bearer <token>`.
    /// Recommended when the server is reachable from the internet.
    pub token: Option<String>,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            token: None,
        }
    }
}

impl std::fmt::Debug for MetricsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetricsConfig")
            .field("enabled", &self.enabled)
            .field("token", &self.token.as_ref().map(|_| "***"))
            .finish()
    }
}

/// Which backend a source uses.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    /// A directory on the host (`path`).
    #[default]
    Local,
    /// An S3-compatible bucket (the `[sources.s3]` table).
    S3,
    /// A directory on an SFTP server (the `[sources.sftp]` table).
    Sftp,
    /// A folder on an SMB share (the `[sources.smb]` table).
    Smb,
}

/// One configured source.
///
/// Remote backends take their settings from a sub-table named after the
/// kind, so each kind's fields stay separate and typos are caught:
///
/// ```toml
/// [[sources]]
/// name = "photos"
/// kind = "s3"
/// [sources.s3]
/// bucket = "family-photos"
/// ```
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    /// Identifier used in URLs and permissions: 1–32 of `[a-z0-9_-]`.
    pub name: String,
    /// Backend type.
    #[serde(default)]
    pub kind: SourceKind,
    /// Root directory (for `local`; leave unset for the other kinds).
    #[serde(default)]
    pub path: PathBuf,
    /// S3 settings (for `kind = "s3"`).
    #[serde(default)]
    pub s3: Option<S3Config>,
    /// SFTP settings (for `kind = "sftp"`).
    #[serde(default)]
    pub sftp: Option<SftpConfig>,
    /// SMB settings (for `kind = "smb"`).
    #[serde(default)]
    pub smb: Option<SmbConfig>,
}

impl SourceConfig {
    /// A local source rooted at `path`.
    pub fn local(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            kind: SourceKind::Local,
            path: path.into(),
            s3: None,
            sftp: None,
            smb: None,
        }
    }

    /// Checks that exactly the settings for `kind` are present.
    fn validate(&self) -> anyhow::Result<()> {
        let name = &self.name;
        let has_path = !self.path.as_os_str().is_empty();
        let tables = [
            ("s3", self.s3.is_some(), SourceKind::S3),
            ("sftp", self.sftp.is_some(), SourceKind::Sftp),
            ("smb", self.smb.is_some(), SourceKind::Smb),
        ];
        for (table, present, kind) in tables {
            if present && self.kind != kind {
                bail!("source {name:?}: [sources.{table}] is only allowed with kind = \"{table}\"");
            }
            if !present && self.kind == kind {
                bail!("source {name:?}: kind = \"{table}\" needs a [sources.{table}] table");
            }
        }
        match self.kind {
            SourceKind::Local if !has_path => bail!("source {name:?}: path is required"),
            SourceKind::Local => {}
            _ if has_path => {
                bail!(
                    "source {name:?}: path is only for local sources; set the root in the backend table"
                )
            }
            _ => {}
        }
        Ok(())
    }
}

/// The full server configuration.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// Address and port to listen on.
    pub listen: SocketAddr,
    /// Directory for the database, upload staging area and secret key.
    pub data_dir: PathBuf,
    /// The URL users reach the server at, e.g. `https://files.example.com`.
    /// Used for share links and to decide whether cookies are `Secure`.
    pub public_url: Option<String>,
    /// Force the `Secure` cookie flag on or off. Unset: on when `public_url`
    /// starts with `https://`.
    pub secure_cookies: Option<bool>,
    /// How long a login lasts without activity, in hours.
    pub session_ttl_hours: u64,
    /// Trust `X-Forwarded-For` for the client address (only behind a proxy you control).
    pub trust_proxy_headers: bool,
    /// Largest single upload chunk accepted, in MiB.
    pub max_chunk_mib: u64,
    /// Largest file accepted through a public upload (drop) link, in MiB.
    pub max_drop_upload_mib: u64,
    /// Hours after which an unfinished resumable upload is discarded.
    pub upload_expiry_hours: u64,
    /// Failed logins allowed per client address within `login_window_secs`.
    pub login_max_attempts: u32,
    /// Window for login rate limiting, in seconds.
    pub login_window_secs: u64,
    /// Name of the administrator created on first start (when there are no users).
    pub admin_username: String,
    /// Password for that first administrator. Unset: a random one is
    /// generated and logged once. Prefer `FILEBRARY_ADMIN_PASSWORD` over
    /// writing it into the file.
    pub admin_password: Option<String>,
    /// Log format.
    pub log_format: LogFormat,
    /// Days a deleted item stays in the trash before it is purged.
    /// `0` turns the trash off: deletes are permanent.
    pub trash_retention_days: u64,
    /// Days activity-log events are kept. `0` keeps them forever.
    pub activity_retention_days: u64,
    /// Storage sources.
    pub sources: Vec<SourceConfig>,
    /// Storage quota for users without their own setting, e.g. `"10G"`
    /// (binary units). Unset: unlimited.
    pub default_quota: Option<String>,
    /// Hours between measurements of quota usage on disk (corrects drift
    /// from deletes and changes made outside Filebrary). 0 turns it off.
    pub quota_rescan_hours: u64,
    /// Name shown in authenticator apps for TOTP 2FA.
    pub totp_issuer: String,
    /// OpenID Connect single sign-on (`[oidc]`). Unset: off.
    pub oidc: Option<filebrary_identity::oidc::OidcConfig>,
    /// LDAP login (`[ldap]`). Unset: off.
    pub ldap: Option<filebrary_identity::ldap::LdapConfig>,
    /// The optional FTP/FTPS server (`[ftp]`), off by default.
    pub ftp: filebrary_ftp::FtpConfig,
    /// Event notifications by webhook, ntfy and email (`[notifications]`).
    pub notifications: filebrary_events::NotificationsConfig,
    /// The Prometheus endpoint (`[metrics]`).
    pub metrics: MetricsConfig,
    /// Indexed search (see [`crate::search::SearchSettings`]).
    pub search: crate::search::SearchSettings,
    /// Background jobs, conversions and custom jobs (`[jobs]`).
    pub jobs: crate::jobs::JobsConfig,
    /// Thumbnails and previews (`[previews]` table).
    pub previews: PreviewSettings,
}

/// Settings for thumbnails and previews.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PreviewSettings {
    /// Turn previews off entirely.
    pub enabled: bool,
    /// Size limit of the preview cache in `<data_dir>/cache/previews`, in MiB.
    /// The least recently used previews are deleted past it.
    pub cache_mib: u64,
    /// Longest side of thumbnails, in pixels.
    pub thumb_size: u32,
    /// Longest side of large previews (RAW, HEIC, TIFF), in pixels.
    pub large_size: u32,
    /// Previews generated at once. 0 means one per CPU, at most 8.
    pub max_parallel: usize,
    /// Largest file from a remote source copied locally to make a preview, in MiB.
    pub max_remote_mib: u64,
}

impl Default for PreviewSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            cache_mib: 2048,
            thumb_size: 256,
            large_size: 1920,
            max_parallel: 0,
            max_remote_mib: 1024,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], 8080)),
            data_dir: PathBuf::from("./data"),
            public_url: None,
            secure_cookies: None,
            session_ttl_hours: 24 * 7,
            trust_proxy_headers: false,
            max_chunk_mib: 64,
            max_drop_upload_mib: 1024,
            upload_expiry_hours: 24,
            login_max_attempts: 5,
            login_window_secs: 300,
            admin_username: "admin".to_owned(),
            admin_password: None,
            log_format: LogFormat::Text,
            trash_retention_days: 30,
            activity_retention_days: 365,
            sources: Vec::new(),
            default_quota: None,
            quota_rescan_hours: 6,
            totp_issuer: "Filebrary".to_owned(),
            oidc: None,
            ldap: None,
            ftp: filebrary_ftp::FtpConfig::default(),
            notifications: filebrary_events::NotificationsConfig::default(),
            metrics: MetricsConfig::default(),
            search: crate::search::SearchSettings::default(),
            jobs: crate::jobs::JobsConfig::default(),
            previews: PreviewSettings::default(),
        }
    }
}

impl Config {
    /// Loads the configuration from `file` (if given) and the environment.
    ///
    /// # Errors
    ///
    /// Fails on unreadable or invalid files, invalid environment values, or an
    /// invalid source list.
    pub fn load(file: Option<&Path>) -> anyhow::Result<Self> {
        let mut cfg = match file {
            Some(p) => {
                let text = std::fs::read_to_string(p)
                    .with_context(|| format!("reading config file {}", p.display()))?;
                toml::from_str::<Config>(&text)
                    .with_context(|| format!("parsing config file {}", p.display()))?
            }
            None => Config::default(),
        };
        cfg.apply_env(|k| std::env::var(k).ok())?;
        if cfg.sources.is_empty() {
            cfg.sources
                .push(SourceConfig::local("files", cfg.data_dir.join("files")));
        }
        cfg.validate()?;
        Ok(cfg)
    }

    /// Applies `FILEBRARY_*` overrides read through `get`.
    ///
    /// # Errors
    ///
    /// Fails if a variable has an invalid value.
    pub fn apply_env(&mut self, get: impl Fn(&str) -> Option<String>) -> anyhow::Result<()> {
        fn parse<T: std::str::FromStr>(name: &str, v: &str) -> anyhow::Result<T> {
            v.parse()
                .map_err(|_| anyhow::anyhow!("invalid value for {name}: {v:?}"))
        }
        fn parse_bool(name: &str, v: &str) -> anyhow::Result<bool> {
            match v.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => Ok(true),
                "0" | "false" | "no" | "off" => Ok(false),
                _ => bail!("invalid value for {name}: {v:?} (use true or false)"),
            }
        }
        if let Some(v) = get("FILEBRARY_LISTEN") {
            self.listen = parse("FILEBRARY_LISTEN", &v)?;
        }
        if let Some(v) = get("FILEBRARY_DATA_DIR") {
            self.data_dir = PathBuf::from(v);
        }
        if let Some(v) = get("FILEBRARY_PUBLIC_URL") {
            self.public_url = (!v.is_empty()).then_some(v);
        }
        if let Some(v) = get("FILEBRARY_SECURE_COOKIES") {
            self.secure_cookies = Some(parse_bool("FILEBRARY_SECURE_COOKIES", &v)?);
        }
        if let Some(v) = get("FILEBRARY_TRUST_PROXY_HEADERS") {
            self.trust_proxy_headers = parse_bool("FILEBRARY_TRUST_PROXY_HEADERS", &v)?;
        }
        if let Some(v) = get("FILEBRARY_SESSION_TTL_HOURS") {
            self.session_ttl_hours = parse("FILEBRARY_SESSION_TTL_HOURS", &v)?;
        }
        if let Some(v) = get("FILEBRARY_ADMIN_USERNAME") {
            self.admin_username = v;
        }
        if let Some(v) = get("FILEBRARY_ADMIN_PASSWORD") {
            self.admin_password = (!v.is_empty()).then_some(v);
        }
        if let Some(v) = get("FILEBRARY_LOG_FORMAT") {
            self.log_format = match v.as_str() {
                "text" => LogFormat::Text,
                "json" => LogFormat::Json,
                _ => bail!("invalid value for FILEBRARY_LOG_FORMAT: {v:?} (use text or json)"),
            };
        }
        if let Some(v) = get("FILEBRARY_SEARCH_ENABLED") {
            self.search.enabled = parse_bool("FILEBRARY_SEARCH_ENABLED", &v)?;
        }
        if let Some(v) = get("FILEBRARY_SEARCH_WATCH") {
            self.search.watch = parse_bool("FILEBRARY_SEARCH_WATCH", &v)?;
        }
        if let Some(v) = get("FILEBRARY_SEARCH_RESCAN_MINUTES") {
            self.search.rescan_minutes = parse("FILEBRARY_SEARCH_RESCAN_MINUTES", &v)?;
        }
        if let Some(v) = get("FILEBRARY_FTP_ENABLED") {
            self.ftp.enabled = parse_bool("FILEBRARY_FTP_ENABLED", &v)?;
        }
        if let Some(v) = get("FILEBRARY_FTP_LISTEN") {
            self.ftp.listen = parse("FILEBRARY_FTP_LISTEN", &v)?;
        }
        if let Some(v) = get("FILEBRARY_FTP_PASSIVE_PORTS") {
            let (lo, hi) = v.split_once('-').with_context(|| {
                format!("invalid value for FILEBRARY_FTP_PASSIVE_PORTS: {v:?} (use low-high)")
            })?;
            self.ftp.passive_ports = [
                parse("FILEBRARY_FTP_PASSIVE_PORTS", lo.trim())?,
                parse("FILEBRARY_FTP_PASSIVE_PORTS", hi.trim())?,
            ];
        }
        if let Some(v) = get("FILEBRARY_FTP_PASSIVE_HOST") {
            self.ftp.passive_host = (!v.is_empty()).then_some(v);
        }
        if let Some(v) = get("FILEBRARY_METRICS_ENABLED") {
            self.metrics.enabled = parse_bool("FILEBRARY_METRICS_ENABLED", &v)?;
        }
        if let Some(v) = get("FILEBRARY_METRICS_TOKEN") {
            self.metrics.token = (!v.is_empty()).then_some(v);
        }
        if let Some(v) = get("FILEBRARY_WEBHOOK_URL").filter(|v| !v.is_empty()) {
            self.notifications
                .webhooks
                .retain(|w| w.name != "env-webhook");
            self.notifications
                .webhooks
                .push(filebrary_events::WebhookConfig {
                    name: "env-webhook".into(),
                    url: v,
                    secret: get("FILEBRARY_WEBHOOK_SECRET").filter(|s| !s.is_empty()),
                    headers: Default::default(),
                    events: Vec::new(),
                });
        }
        if let Some(v) = get("FILEBRARY_NTFY_TOPIC").filter(|v| !v.is_empty()) {
            self.notifications.ntfy.retain(|n| n.name != "env-ntfy");
            self.notifications.ntfy.push(filebrary_events::NtfyConfig {
                name: "env-ntfy".into(),
                server: get("FILEBRARY_NTFY_SERVER")
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "https://ntfy.sh".into()),
                topic: v,
                token: get("FILEBRARY_NTFY_TOKEN").filter(|s| !s.is_empty()),
                username: None,
                password: None,
                click: self.public_url.clone(),
                events: Vec::new(),
            });
        }
        if let Some(v) = get("FILEBRARY_TRASH_RETENTION_DAYS") {
            self.trash_retention_days = parse("FILEBRARY_TRASH_RETENTION_DAYS", &v)?;
        }
        if let Some(v) = get("FILEBRARY_ACTIVITY_RETENTION_DAYS") {
            self.activity_retention_days = parse("FILEBRARY_ACTIVITY_RETENTION_DAYS", &v)?;
        }
        if let Some(v) = get("FILEBRARY_DEFAULT_QUOTA") {
            self.default_quota = (!v.trim().is_empty()).then_some(v);
        }
        if let Some(v) = get("FILEBRARY_QUOTA_RESCAN_HOURS") {
            self.quota_rescan_hours = parse("FILEBRARY_QUOTA_RESCAN_HOURS", &v)?;
        }
        if let Some(v) = get("FILEBRARY_OIDC_CLIENT_SECRET").filter(|v| !v.is_empty()) {
            let Some(o) = self.oidc.as_mut() else {
                bail!("FILEBRARY_OIDC_CLIENT_SECRET is set but there is no [oidc] section");
            };
            o.client_secret = Some(filebrary_identity::SecretString::new(v));
            o.client_secret_file = None;
        }
        if let Some(v) = get("FILEBRARY_LDAP_BIND_PASSWORD").filter(|v| !v.is_empty()) {
            let Some(l) = self.ldap.as_mut() else {
                bail!("FILEBRARY_LDAP_BIND_PASSWORD is set but there is no [ldap] section");
            };
            l.bind_password = Some(filebrary_identity::SecretString::new(v));
            l.bind_password_file = None;
        }
        if let Some(v) = get("FILEBRARY_PREVIEWS") {
            self.previews.enabled = parse_bool("FILEBRARY_PREVIEWS", &v)?;
        }
        if let Some(v) = get("FILEBRARY_PREVIEW_CACHE_MIB") {
            self.previews.cache_mib = parse("FILEBRARY_PREVIEW_CACHE_MIB", &v)?;
        }
        // An empty value counts as unset, so a mounted config file's [[sources]] can win.
        if let Some(v) = get("FILEBRARY_SOURCES").filter(|v| !v.trim().is_empty()) {
            self.sources = parse_sources(&v)?;
        }
        self.jobs.apply_env(&get)?;
        Ok(())
    }

    /// Checks the configuration for mistakes.
    ///
    /// # Errors
    ///
    /// Fails on duplicate or invalid source names and out-of-range numbers.
    pub fn validate(&self) -> anyhow::Result<()> {
        let mut seen = std::collections::HashSet::new();
        for s in &self.sources {
            if !valid_source_name(&s.name) {
                bail!(
                    "invalid source name {:?}: use 1-32 characters from a-z, 0-9, '-' and '_'",
                    s.name
                );
            }
            if !seen.insert(&s.name) {
                bail!("duplicate source name {:?}", s.name);
            }
            s.validate()?;
        }
        if self.session_ttl_hours == 0 {
            bail!("session_ttl_hours must be at least 1");
        }
        if self.max_chunk_mib == 0 || self.max_chunk_mib > 4096 {
            bail!("max_chunk_mib must be between 1 and 4096");
        }
        if self.trash_retention_days > 36_500 || self.activity_retention_days > 36_500 {
            bail!("retention days must be at most 36500");
        }
        if !(32..=1024).contains(&self.previews.thumb_size)
            || !(256..=8192).contains(&self.previews.large_size)
        {
            bail!("previews.thumb_size must be 32-1024 and previews.large_size 256-8192");
        }
        if self.login_max_attempts == 0 {
            bail!("login_max_attempts must be at least 1");
        }
        self.search.validate()?;
        if let Some(u) = &self.public_url
            && !(u.starts_with("http://") || u.starts_with("https://"))
        {
            bail!("public_url must start with http:// or https://");
        }
        if let Some(q) = &self.default_quota
            && filebrary_identity::quota::parse_size(q).is_none()
        {
            bail!("default_quota {q:?} is not a size such as 500M or 10G");
        }
        if let Some(o) = &self.oidc {
            o.validate().map_err(|e| anyhow::anyhow!("[oidc]: {e}"))?;
            if self.public_url.is_none() {
                bail!("[oidc] needs public_url to build the callback URL");
            }
        }
        if let Some(l) = &self.ldap {
            l.validate().map_err(|e| anyhow::anyhow!("[ldap]: {e}"))?;
        }
        self.ftp.validate().map_err(anyhow::Error::msg)?;
        self.notifications.validate().map_err(anyhow::Error::msg)?;
        if self.metrics.token.as_deref().is_some_and(|t| t.len() < 16) {
            bail!("metrics.token must be at least 16 characters");
        }
        self.jobs.validate()?;
        Ok(())
    }

    /// `default_quota` in bytes (`None`: unlimited).
    pub fn default_quota_bytes(&self) -> Option<i64> {
        self.default_quota
            .as_deref()
            .and_then(filebrary_identity::quota::parse_size)
    }

    /// Whether session cookies get the `Secure` flag.
    pub fn cookies_secure(&self) -> bool {
        self.secure_cookies.unwrap_or_else(|| {
            self.public_url
                .as_deref()
                .is_some_and(|u| u.starts_with("https://"))
        })
    }
}

/// Returns `true` if `name` is a valid source identifier.
pub fn valid_source_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Parses `name=path` pairs separated by `;` (or `,`), e.g. `files=/srv;media=/mnt/media`.
fn parse_sources(v: &str) -> anyhow::Result<Vec<SourceConfig>> {
    v.split([';', ','])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|pair| {
            let (name, path) = pair
                .split_once('=')
                .with_context(|| format!("FILEBRARY_SOURCES entry {pair:?} is not name=path"))?;
            Ok(SourceConfig::local(name.trim(), path.trim()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_toml() {
        let cfg: Config = toml::from_str(
            r#"
            listen = "0.0.0.0:9000"
            public_url = "https://x.example"
            [[sources]]
            name = "media"
            path = "/mnt/media"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.listen.port(), 9000);
        assert!(cfg.cookies_secure());
        assert_eq!(cfg.sources[0].name, "media");
        assert_eq!(cfg.session_ttl_hours, 24 * 7);
        cfg.validate().unwrap();
    }

    #[test]
    fn identity_sections() {
        let mut cfg: Config = toml::from_str(
            r#"
            public_url = "https://files.example.com"
            default_quota = "10G"
            [oidc]
            issuer_url = "https://auth.example.com"
            client_id = "filebrary"
            [ldap]
            url = "ldaps://ldap.example.com"
            base_dn = "dc=example,dc=com"
            "#,
        )
        .unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.default_quota_bytes(), Some(10 << 30));
        cfg.apply_env(|k| (k == "FILEBRARY_OIDC_CLIENT_SECRET").then(|| "s3cret".into()))
            .unwrap();
        let o = cfg.oidc.as_ref().unwrap();
        assert_eq!(o.client_secret.as_ref().unwrap().expose(), "s3cret");
        assert!(!format!("{cfg:?}").contains("s3cret"));
        assert!(
            Config::default()
                .apply_env(|k| (k == "FILEBRARY_LDAP_BIND_PASSWORD").then(|| "x".into()))
                .is_err()
        );
        cfg.public_url = None;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(toml::from_str::<Config>("listn = \"x\"").is_err());
    }

    #[test]
    fn env_overrides() {
        let mut cfg = Config::default();
        cfg.apply_env(|k| match k {
            "FILEBRARY_SOURCES" => Some("files=/srv; media = /mnt/m".into()),
            "FILEBRARY_SECURE_COOKIES" => Some("true".into()),
            "FILEBRARY_LISTEN" => Some("0.0.0.0:1234".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(cfg.sources.len(), 2);
        assert_eq!(cfg.sources[1].name, "media");
        assert_eq!(cfg.sources[1].path, PathBuf::from("/mnt/m"));
        assert!(cfg.cookies_secure());
        assert_eq!(cfg.listen.port(), 1234);
        assert!(
            Config::default()
                .apply_env(|k| (k == "FILEBRARY_SECURE_COOKIES").then(|| "maybe".into()))
                .is_err()
        );
    }

    #[test]
    fn ops_sections() {
        let cfg: Config = toml::from_str(
            r#"
            [ftp]
            enabled = true
            listen = "127.0.0.1:2121"
            [metrics]
            token = "0123456789abcdef"
            [notifications]
            quota_warning_percent = 80
            [[notifications.ntfy]]
            name = "phone"
            topic = "files"
            "#,
        )
        .unwrap();
        cfg.validate().unwrap();
        assert!(cfg.ftp.enabled);
        assert_eq!(cfg.notifications.ntfy[0].topic, "files");
        assert!(!format!("{cfg:?}").contains("0123456789abcdef"));
        assert!(!Config::default().ftp.enabled);
        assert!(Config::default().metrics.enabled);

        let mut cfg = Config::default();
        cfg.apply_env(|k| match k {
            "FILEBRARY_FTP_ENABLED" => Some("true".into()),
            "FILEBRARY_FTP_PASSIVE_PORTS" => Some("6000-6010".into()),
            "FILEBRARY_WEBHOOK_URL" => Some("https://hooks.example.com/x".into()),
            "FILEBRARY_NTFY_TOPIC" => Some("t".into()),
            _ => None,
        })
        .unwrap();
        assert!(cfg.ftp.enabled);
        assert_eq!(cfg.ftp.passive_ports, [6000, 6010]);
        assert_eq!(cfg.notifications.webhooks[0].name, "env-webhook");
        assert_eq!(cfg.notifications.ntfy[0].server, "https://ntfy.sh");
        cfg.validate().unwrap();
        assert!(
            Config::default()
                .apply_env(|k| (k == "FILEBRARY_FTP_PASSIVE_PORTS").then(|| "6000".into()))
                .is_err()
        );
    }

    #[test]
    fn validates_source_names() {
        assert!(valid_source_name("my-files_2"));
        for bad in ["", "Files", "a/b", "a b", "..", &"x".repeat(33)] {
            assert!(!valid_source_name(bad), "{bad}");
        }
        let cfg = Config {
            sources: parse_sources("a=/x;a=/y").unwrap(),
            ..Config::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn parses_remote_sources() {
        let cfg: Config = toml::from_str(
            r#"
            [[sources]]
            name = "files"
            path = "/srv"

            [[sources]]
            name = "photos"
            kind = "s3"
            [sources.s3]
            bucket = "family"
            prefix = "lib"
            endpoint = "http://minio:9000"

            [[sources]]
            name = "nas"
            kind = "sftp"
            [sources.sftp]
            host = "nas.lan"
            username = "fb"
            private_key = "/keys/id"
            host_key_fingerprint = "SHA256:abc"

            [[sources]]
            name = "office"
            kind = "smb"
            [sources.smb]
            host = "fs"
            share = "Projects"
            "#,
        )
        .unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.sources[1].kind, SourceKind::S3);
        assert_eq!(cfg.sources[1].s3.as_ref().unwrap().bucket, "family");
        assert_eq!(cfg.sources[2].kind, SourceKind::Sftp);
        assert_eq!(cfg.sources[3].smb.as_ref().unwrap().share, "Projects");
    }

    #[test]
    fn source_tables_must_match_kind() {
        let check = |t: &str| {
            toml::from_str::<Config>(t)
                .map_err(anyhow::Error::from)
                .and_then(|c| c.validate())
        };
        // Missing table for the kind.
        assert!(check("[[sources]]\nname = \"a\"\nkind = \"s3\"").is_err());
        // Table for another kind.
        assert!(
            check("[[sources]]\nname = \"a\"\npath = \"/x\"\n[sources.smb]\nhost = \"h\"\nshare = \"s\"")
                .is_err()
        );
        // `path` on a remote source.
        assert!(
            check("[[sources]]\nname = \"a\"\nkind = \"smb\"\npath = \"/x\"\n[sources.smb]\nhost = \"h\"\nshare = \"s\"")
                .is_err()
        );
        // Local without a path.
        assert!(check("[[sources]]\nname = \"a\"").is_err());
        // Unknown keys inside a backend table.
        assert!(
            check("[[sources]]\nname = \"a\"\nkind = \"s3\"\n[sources.s3]\nbucket = \"b\"\nbuckt = \"c\"")
                .is_err()
        );
    }
}
