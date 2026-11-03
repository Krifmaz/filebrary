//! Notification settings, read from the `[notifications]` table of the
//! server configuration.
//!
//! ```toml
//! [notifications]
//! quota_warning_percent = 90
//!
//! [[notifications.webhooks]]
//! name = "automation"
//! url = "https://hooks.example.com/filebrary"
//! secret = "a long random string"
//! events = ["upload", "job_failed"]
//!
//! [[notifications.ntfy]]
//! name = "phone"
//! server = "https://ntfy.sh"
//! topic = "my-filebrary"
//! token = "tk_..."
//!
//! [[notifications.email]]
//! name = "admin-mail"
//! smtp_host = "smtp.example.com"
//! security = "starttls"
//! username = "filebrary@example.com"
//! password = "..."
//! from = "Filebrary <filebrary@example.com>"
//! to = ["admin@example.com"]
//! events = ["quota_near_full", "job_failed"]
//! ```

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::EventKind;

/// Which events a target receives. An empty list means every event.
pub type EventFilter = Vec<EventKind>;

/// All notification settings.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationsConfig {
    /// Name of this server in notifications, so a receiver listening to
    /// several servers can tell them apart. Defaults to `Filebrary`.
    pub instance: String,
    /// Percentage of a user's quota at which a `quota_near_full` event is
    /// sent (once, when usage crosses it). 1 to 100; default 90.
    pub quota_warning_percent: u8,
    /// Retry and timeout behaviour shared by all targets.
    pub retry: RetryConfig,
    /// HTTP webhooks that receive each event as JSON.
    pub webhooks: Vec<WebhookConfig>,
    /// ntfy topics.
    pub ntfy: Vec<NtfyConfig>,
    /// SMTP mail targets.
    pub email: Vec<EmailConfig>,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            instance: "Filebrary".to_owned(),
            quota_warning_percent: 90,
            retry: RetryConfig::default(),
            webhooks: Vec::new(),
            ntfy: Vec::new(),
            email: Vec::new(),
        }
    }
}

/// How hard to try delivering one notification.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct RetryConfig {
    /// Attempts in total, including the first. 1 to 10; default 4.
    pub attempts: u32,
    /// Wait before the first retry, in milliseconds; doubled each time.
    /// Default 1000.
    pub initial_backoff_ms: u64,
    /// Timeout for one attempt, in seconds. Default 10.
    pub timeout_secs: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            attempts: 4,
            initial_backoff_ms: 1000,
            timeout_secs: 10,
        }
    }
}

/// An HTTP endpoint that receives a `POST` with the event as JSON.
///
/// When `secret` is set, every request carries
/// `X-Filebrary-Timestamp: <unix seconds>` and
/// `X-Filebrary-Signature: sha256=<hex HMAC-SHA256 of "<timestamp>.<body>">`
/// so the receiver can check the request came from this server and is fresh.
#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    /// Unique name, used in logs, metrics and the test endpoint.
    pub name: String,
    /// `http://` or `https://` URL to post to.
    pub url: String,
    /// Shared secret for the signature header.
    #[serde(default)]
    pub secret: Option<String>,
    /// Extra request headers, for example an `Authorization` header.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Events to send; empty means all.
    #[serde(default)]
    pub events: EventFilter,
}

impl std::fmt::Debug for WebhookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookConfig")
            .field("name", &self.name)
            .field("url", &self.url)
            .field("secret", &self.secret.as_ref().map(|_| "***"))
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .field("events", &self.events)
            .finish()
    }
}

/// An [ntfy](https://ntfy.sh) topic.
#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NtfyConfig {
    /// Unique name, used in logs, metrics and the test endpoint.
    pub name: String,
    /// The ntfy server. Defaults to `https://ntfy.sh`.
    #[serde(default = "default_ntfy_server")]
    pub server: String,
    /// Topic to publish to (letters, digits, `-` and `_`).
    pub topic: String,
    /// Access token (`Authorization: Bearer`). Takes precedence over
    /// `username`/`password`.
    #[serde(default)]
    pub token: Option<String>,
    /// User name for basic authentication.
    #[serde(default)]
    pub username: Option<String>,
    /// Password for basic authentication.
    #[serde(default)]
    pub password: Option<String>,
    /// URL opened when the notification is tapped, for example the
    /// server's public URL.
    #[serde(default)]
    pub click: Option<String>,
    /// Events to send; empty means all.
    #[serde(default)]
    pub events: EventFilter,
}

impl std::fmt::Debug for NtfyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NtfyConfig")
            .field("name", &self.name)
            .field("server", &self.server)
            .field("topic", &self.topic)
            .field("token", &self.token.as_ref().map(|_| "***"))
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("events", &self.events)
            .finish()
    }
}

fn default_ntfy_server() -> String {
    "https://ntfy.sh".to_owned()
}

/// How the SMTP connection is secured.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EmailSecurity {
    /// TLS from the first byte (usually port 465).
    Tls,
    /// Plain connection upgraded with `STARTTLS`, which must succeed
    /// (usually port 587). The default.
    #[default]
    Starttls,
    /// No encryption. Only for a relay on the same host or network.
    None,
}

/// An SMTP mail target.
#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EmailConfig {
    /// Unique name, used in logs, metrics and the test endpoint.
    pub name: String,
    /// SMTP server host name.
    pub smtp_host: String,
    /// SMTP port. Defaults to 465 for `tls`, 587 for `starttls`, 25 for `none`.
    #[serde(default)]
    pub smtp_port: Option<u16>,
    /// Connection security.
    #[serde(default)]
    pub security: EmailSecurity,
    /// SMTP user name, if the server requires authentication.
    #[serde(default)]
    pub username: Option<String>,
    /// SMTP password.
    #[serde(default)]
    pub password: Option<String>,
    /// Sender address, e.g. `Filebrary <files@example.com>`.
    pub from: String,
    /// Recipient addresses.
    pub to: Vec<String>,
    /// Events to send; empty means all.
    #[serde(default)]
    pub events: EventFilter,
}

impl std::fmt::Debug for EmailConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailConfig")
            .field("name", &self.name)
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field("security", &self.security)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("from", &self.from)
            .field("to", &self.to)
            .field("events", &self.events)
            .finish()
    }
}

impl EmailConfig {
    /// The port to connect to, after applying the default for `security`.
    pub fn port(&self) -> u16 {
        self.smtp_port.unwrap_or(match self.security {
            EmailSecurity::Tls => 465,
            EmailSecurity::Starttls => 587,
            EmailSecurity::None => 25,
        })
    }
}

impl NotificationsConfig {
    /// Returns `true` when no target is configured.
    pub fn is_empty(&self) -> bool {
        self.webhooks.is_empty() && self.ntfy.is_empty() && self.email.is_empty()
    }

    /// Checks the settings for mistakes.
    ///
    /// # Errors
    ///
    /// Returns a message naming the first problem found: a duplicate or
    /// invalid target name, a URL that is not `http(s)`, an invalid topic or
    /// address, or a number out of range.
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=100).contains(&self.quota_warning_percent) {
            return Err("notifications.quota_warning_percent must be between 1 and 100".into());
        }
        if !(1..=10).contains(&self.retry.attempts) {
            return Err("notifications.retry.attempts must be between 1 and 10".into());
        }
        if self.retry.timeout_secs == 0 || self.retry.timeout_secs > 300 {
            return Err("notifications.retry.timeout_secs must be between 1 and 300".into());
        }
        let mut names = HashSet::new();
        let all_names = self
            .webhooks
            .iter()
            .map(|w| &w.name)
            .chain(self.ntfy.iter().map(|n| &n.name))
            .chain(self.email.iter().map(|e| &e.name));
        for name in all_names {
            if !valid_name(name) {
                return Err(format!(
                    "invalid notification target name {name:?}: use 1-64 characters from a-z, A-Z, 0-9, '-' and '_'"
                ));
            }
            if !names.insert(name) {
                return Err(format!("duplicate notification target name {name:?}"));
            }
        }
        for w in &self.webhooks {
            check_http_url(&w.url, &format!("webhook {:?} url", w.name))?;
            for (k, v) in &w.headers {
                if reqwest::header::HeaderName::from_bytes(k.as_bytes()).is_err()
                    || reqwest::header::HeaderValue::from_str(v).is_err()
                {
                    return Err(format!("webhook {:?} has an invalid header {k:?}", w.name));
                }
            }
        }
        for n in &self.ntfy {
            check_http_url(&n.server, &format!("ntfy {:?} server", n.name))?;
            if n.topic.is_empty()
                || n.topic.len() > 64
                || !n
                    .topic
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                return Err(format!(
                    "ntfy {:?} topic must be 1-64 characters from a-z, A-Z, 0-9, '-' and '_'",
                    n.name
                ));
            }
            if n.username.is_some() != n.password.is_some() {
                return Err(format!(
                    "ntfy {:?} needs both username and password, or neither",
                    n.name
                ));
            }
            if let Some(c) = &n.click {
                check_http_url(c, &format!("ntfy {:?} click", n.name))?;
            }
        }
        for e in &self.email {
            if e.smtp_host.is_empty() {
                return Err(format!("email {:?} needs smtp_host", e.name));
            }
            if e.username.is_some() != e.password.is_some() {
                return Err(format!(
                    "email {:?} needs both username and password, or neither",
                    e.name
                ));
            }
            e.from
                .parse::<lettre::message::Mailbox>()
                .map_err(|err| format!("email {:?} from address: {err}", e.name))?;
            if e.to.is_empty() {
                return Err(format!(
                    "email {:?} needs at least one address in to",
                    e.name
                ));
            }
            for to in &e.to {
                to.parse::<lettre::message::Mailbox>()
                    .map_err(|err| format!("email {:?} address {to:?}: {err}", e.name))?;
            }
        }
        Ok(())
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn check_http_url(raw: &str, what: &str) -> Result<(), String> {
    let u = url::Url::parse(raw).map_err(|e| format!("{what} is not a valid URL: {e}"))?;
    if u.scheme() != "http" && u.scheme() != "https" {
        return Err(format!("{what} must start with http:// or https://"));
    }
    if u.host_str().is_none() {
        return Err(format!("{what} has no host"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml_text: &str) -> NotificationsConfig {
        toml::from_str(toml_text).unwrap()
    }

    #[test]
    fn defaults_are_valid_and_empty() {
        let c = NotificationsConfig::default();
        assert!(c.is_empty());
        c.validate().unwrap();
    }

    #[test]
    fn full_example_parses() {
        let c = parse(
            r#"
            quota_warning_percent = 80
            [[webhooks]]
            name = "hook"
            url = "https://example.com/h"
            secret = "s"
            headers = { Authorization = "Bearer x" }
            events = ["upload", "job_failed"]
            [[ntfy]]
            name = "phone"
            topic = "files_1"
            token = "tk"
            [[email]]
            name = "mail"
            smtp_host = "smtp.example.com"
            security = "tls"
            username = "u"
            password = "p"
            from = "Filebrary <f@example.com>"
            to = ["a@example.com"]
            "#,
        );
        c.validate().unwrap();
        assert_eq!(
            c.webhooks[0].events,
            [EventKind::Upload, EventKind::JobFailed]
        );
        assert_eq!(c.ntfy[0].server, "https://ntfy.sh");
        assert_eq!(c.email[0].port(), 465);
        // Secrets never show up in debug output (and so in logs).
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("\"p\"") && !dbg.contains("\"tk\"") && !dbg.contains("Bearer"));
    }

    #[test]
    fn rejects_mistakes() {
        let bad = [
            "quota_warning_percent = 0",
            "[[webhooks]]\nname='a'\nurl='ftp://x'",
            "[[webhooks]]\nname='a'\nurl='http://x'\n[[ntfy]]\nname='a'\ntopic='t'",
            "[[ntfy]]\nname='a'\ntopic='bad topic'",
            "[[ntfy]]\nname='a'\ntopic='t'\nusername='u'",
            "[[email]]\nname='m'\nsmtp_host='h'\nfrom='nope'\nto=['a@b.c']",
            "[[email]]\nname='m'\nsmtp_host='h'\nfrom='a@b.c'\nto=[]",
            "[[webhooks]]\nname='bad name'\nurl='http://x'",
        ];
        for b in bad {
            assert!(parse(b).validate().is_err(), "accepted: {b}");
        }
        assert!(toml::from_str::<NotificationsConfig>("unknown = 1").is_err());
        assert!(
            toml::from_str::<NotificationsConfig>(
                "[[webhooks]]\nname='a'\nurl='http://x'\nevents=['nope']"
            )
            .is_err()
        );
    }
}
