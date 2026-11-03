//! FTP settings, read from the `[ftp]` table of the server configuration.
//!
//! ```toml
//! [ftp]
//! enabled = true
//! listen = "0.0.0.0:2121"
//! passive_ports = [50000, 50100]
//! passive_host = "203.0.113.7"   # the address clients connect back to
//!
//! [ftp.tls]
//! cert = "/data/tls/cert.pem"
//! key = "/data/tls/key.pem"
//! mode = "required"               # or "optional"
//! ```

use std::{net::SocketAddr, path::PathBuf};

use serde::Deserialize;

/// Whether clients must use TLS.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FtpsMode {
    /// Clients must upgrade both the control and data connections with
    /// `AUTH TLS` before logging in. The default when TLS is configured,
    /// because plain FTP sends passwords in clear text.
    #[default]
    Required,
    /// Clients may use TLS or plain FTP.
    Optional,
}

/// Certificate settings for explicit FTPS (`AUTH TLS`).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FtpsConfig {
    /// PEM file with the certificate chain.
    pub cert: PathBuf,
    /// PEM file with the private key.
    pub key: PathBuf,
    /// Whether TLS is required.
    #[serde(default)]
    pub mode: FtpsMode,
}

/// All FTP settings.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct FtpConfig {
    /// Turn the FTP server on. Off by default.
    pub enabled: bool,
    /// Control connection address. Default `0.0.0.0:2121`.
    pub listen: SocketAddr,
    /// Inclusive range of ports for passive data connections. Default
    /// `[50000, 50100]`. Forward this range too when behind NAT or Docker.
    pub passive_ports: [u16; 2],
    /// Address announced in `PASV` replies (IPv4 address or DNS name).
    /// Unset: the address the client connected to, which is wrong behind
    /// NAT or Docker port mapping.
    pub passive_host: Option<String>,
    /// Explicit FTPS. Unset: plain FTP only (a warning is logged at start).
    pub tls: Option<FtpsConfig>,
    /// Failed logins from one address before it is blocked. Default 5.
    pub max_login_attempts: u32,
    /// How long a blocked address stays blocked, in seconds. Default 300.
    pub login_block_secs: u64,
    /// Seconds without a command before a session is closed. Default 600.
    pub idle_timeout_secs: u64,
    /// Greeting sent on connect.
    pub greeting: String,
}

impl Default for FtpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: SocketAddr::from(([0, 0, 0, 0], 2121)),
            passive_ports: [50000, 50100],
            passive_host: None,
            tls: None,
            max_login_attempts: 5,
            login_block_secs: 300,
            idle_timeout_secs: 600,
            greeting: "Filebrary FTP server ready".to_owned(),
        }
    }
}

impl FtpConfig {
    /// Checks the settings for mistakes.
    ///
    /// # Errors
    ///
    /// Returns a message describing the first problem found.
    pub fn validate(&self) -> Result<(), String> {
        let [lo, hi] = self.passive_ports;
        if lo == 0 || lo > hi {
            return Err("ftp.passive_ports must be [low, high] with 0 < low <= high".into());
        }
        if self.max_login_attempts == 0 {
            return Err("ftp.max_login_attempts must be at least 1".into());
        }
        if self.idle_timeout_secs == 0 {
            return Err("ftp.idle_timeout_secs must be at least 1".into());
        }
        if self.greeting.contains(['\r', '\n']) {
            return Err("ftp.greeting must be a single line".into());
        }
        if let Some(tls) = &self.tls
            && self.enabled
        {
            for (what, p) in [("cert", &tls.cert), ("key", &tls.key)] {
                if !p.is_file() {
                    return Err(format!(
                        "ftp.tls.{what} {} is not a readable file",
                        p.display()
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_by_default() {
        let c = FtpConfig::default();
        assert!(!c.enabled);
        c.validate().unwrap();
    }

    #[test]
    fn parses_and_validates() {
        let c: FtpConfig = toml::from_str(
            "enabled = true\nlisten = '127.0.0.1:21'\npassive_ports = [6000, 6010]\npassive_host = 'ftp.example.com'",
        )
        .unwrap();
        c.validate().unwrap();
        assert_eq!(c.passive_ports, [6000, 6010]);
        let bad: FtpConfig = toml::from_str("passive_ports = [10, 5]").unwrap();
        assert!(bad.validate().is_err());
        let bad: FtpConfig =
            toml::from_str("enabled = true\n[tls]\ncert='/nope.pem'\nkey='/nope.key'").unwrap();
        assert!(bad.validate().is_err());
        assert!(toml::from_str::<FtpConfig>("bogus = 1").is_err());
    }
}
