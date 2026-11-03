//! Server configuration.
//!
//! Configuration comes from an optional TOML file, then environment variables
//! (which win), then built-in defaults. Every setting is documented in
//! `docs/src/configuration.md`.
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

/// Which backend a source uses. Only `local` exists in v0.1.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    /// A directory on the host.
    #[default]
    Local,
}

/// One configured source.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    /// Identifier used in URLs and permissions: 1–32 of `[a-z0-9_-]`.
    pub name: String,
    /// Backend type.
    #[serde(default)]
    pub kind: SourceKind,
    /// Root directory (for `local`).
    pub path: PathBuf,
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
    /// The optional FTP/FTPS server (`[ftp]`), off by default.
    pub ftp: filebrary_ftp::FtpConfig,
    /// Event notifications by webhook, ntfy and email (`[notifications]`).
    pub notifications: filebrary_events::NotificationsConfig,
    /// The Prometheus endpoint (`[metrics]`).
    pub metrics: MetricsConfig,
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
            ftp: filebrary_ftp::FtpConfig::default(),
            notifications: filebrary_events::NotificationsConfig::default(),
            metrics: MetricsConfig::default(),
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
            cfg.sources.push(SourceConfig {
                name: "files".into(),
                kind: SourceKind::Local,
                path: cfg.data_dir.join("files"),
            });
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
        // An empty value counts as unset, so a mounted config file's [[sources]] can win.
        if let Some(v) = get("FILEBRARY_SOURCES").filter(|v| !v.trim().is_empty()) {
            self.sources = parse_sources(&v)?;
        }
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
        if self.login_max_attempts == 0 {
            bail!("login_max_attempts must be at least 1");
        }
        if let Some(u) = &self.public_url
            && !(u.starts_with("http://") || u.starts_with("https://"))
        {
            bail!("public_url must start with http:// or https://");
        }
        self.ftp.validate().map_err(anyhow::Error::msg)?;
        self.notifications.validate().map_err(anyhow::Error::msg)?;
        if self.metrics.token.as_deref().is_some_and(|t| t.len() < 16) {
            bail!("metrics.token must be at least 16 characters");
        }
        Ok(())
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
            Ok(SourceConfig {
                name: name.trim().to_owned(),
                kind: SourceKind::Local,
                path: PathBuf::from(path.trim()),
            })
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
}
