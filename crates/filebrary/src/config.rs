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
    /// Indexed search (see [`crate::search::SearchSettings`]).
    pub search: crate::search::SearchSettings,
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
            search: crate::search::SearchSettings::default(),
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
        if self.login_max_attempts == 0 {
            bail!("login_max_attempts must be at least 1");
        }
        self.search.validate()?;
        if let Some(u) = &self.public_url
            && !(u.starts_with("http://") || u.starts_with("https://"))
        {
            bail!("public_url must start with http:// or https://");
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
