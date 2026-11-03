//! Notification targets.

use std::time::Duration;

use futures::future::BoxFuture;
use hmac::{Hmac, KeyInit, Mac};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::authentication::Credentials,
};
use reqwest::{
    Client, StatusCode,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde_json::json;
use sha2::Sha256;

use crate::{
    EventKind, Notice,
    config::{EmailConfig, EmailSecurity, EventFilter, NtfyConfig, WebhookConfig},
};

/// Why a delivery failed.
#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    /// A failure that may go away (network error, timeout, 5xx, 429).
    /// The dispatcher retries these.
    #[error("{0}")]
    Transient(String),
    /// A failure that will not go away by retrying (4xx, rejected address).
    #[error("{0}")]
    Permanent(String),
}

impl SinkError {
    /// Whether retrying may help.
    pub fn is_transient(&self) -> bool {
        matches!(self, SinkError::Transient(_))
    }
}

/// A place notifications are delivered to.
pub trait Sink: Send + Sync {
    /// The configured name of this target.
    fn name(&self) -> &str;

    /// The kind of target: `webhook`, `ntfy` or `email`.
    fn kind(&self) -> &'static str;

    /// The events this target wants (empty means all).
    fn filter(&self) -> &EventFilter;

    /// Whether this target wants events of `kind`. Test events are always
    /// accepted.
    fn accepts(&self, kind: EventKind) -> bool {
        kind == EventKind::Test || self.filter().is_empty() || self.filter().contains(&kind)
    }

    /// Delivers one notification (a single attempt).
    fn send<'a>(&'a self, notice: &'a Notice) -> BoxFuture<'a, Result<(), SinkError>>;
}

/// Maps an HTTP response status to a result.
fn check_status(status: StatusCode) -> Result<(), SinkError> {
    if status.is_success() {
        Ok(())
    } else if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
        Err(SinkError::Transient(format!("server answered {status}")))
    } else {
        Err(SinkError::Permanent(format!("server answered {status}")))
    }
}

fn request_error(e: reqwest::Error) -> SinkError {
    // Strip the URL: it may contain a token in the query string.
    let e = e.without_url();
    if e.is_builder() {
        SinkError::Permanent(e.to_string())
    } else {
        SinkError::Transient(e.to_string())
    }
}

/// Computes the webhook signature header value for `body` sent at
/// `timestamp` (Unix seconds): `sha256=<hex HMAC-SHA256 of "<timestamp>.<body>">`.
///
/// Receivers recompute this with the shared secret and compare in constant
/// time, and reject old timestamps to stop replays.
pub fn sign_payload(secret: &[u8], timestamp: i64, body: &[u8]) -> String {
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// Posts each event as JSON to an HTTP endpoint.
pub struct WebhookSink {
    cfg: WebhookConfig,
    headers: HeaderMap,
    client: Client,
}

impl WebhookSink {
    /// Creates the sink. `client` should carry the per-attempt timeout.
    ///
    /// # Errors
    ///
    /// Fails if a configured header is invalid.
    pub fn new(cfg: WebhookConfig, client: Client) -> Result<Self, String> {
        let mut headers = HeaderMap::new();
        for (k, v) in &cfg.headers {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|_| format!("invalid header name {k:?}"))?;
            let mut value =
                HeaderValue::from_str(v).map_err(|_| format!("invalid header value for {k:?}"))?;
            value.set_sensitive(true);
            headers.insert(name, value);
        }
        Ok(Self {
            cfg,
            headers,
            client,
        })
    }
}

impl Sink for WebhookSink {
    fn name(&self) -> &str {
        &self.cfg.name
    }

    fn kind(&self) -> &'static str {
        "webhook"
    }

    fn filter(&self) -> &EventFilter {
        &self.cfg.events
    }

    fn send<'a>(&'a self, notice: &'a Notice) -> BoxFuture<'a, Result<(), SinkError>> {
        Box::pin(async move {
            let body = serde_json::to_vec(notice)
                .map_err(|e| SinkError::Permanent(format!("serialising event: {e}")))?;
            let mut req = self
                .client
                .post(&self.cfg.url)
                .headers(self.headers.clone())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header("x-filebrary-event", notice.event.kind().as_str());
            if let Some(secret) = &self.cfg.secret {
                let ts = time::OffsetDateTime::now_utc().unix_timestamp();
                req = req.header("x-filebrary-timestamp", ts.to_string()).header(
                    "x-filebrary-signature",
                    sign_payload(secret.as_bytes(), ts, &body),
                );
            }
            let resp = req.body(body).send().await.map_err(request_error)?;
            check_status(resp.status())
        })
    }
}

/// Publishes a short message to an ntfy topic.
pub struct NtfySink {
    cfg: NtfyConfig,
    client: Client,
}

impl NtfySink {
    /// Creates the sink. `client` should carry the per-attempt timeout.
    pub fn new(cfg: NtfyConfig, client: Client) -> Self {
        Self { cfg, client }
    }
}

impl Sink for NtfySink {
    fn name(&self) -> &str {
        &self.cfg.name
    }

    fn kind(&self) -> &'static str {
        "ntfy"
    }

    fn filter(&self) -> &EventFilter {
        &self.cfg.events
    }

    fn send<'a>(&'a self, notice: &'a Notice) -> BoxFuture<'a, Result<(), SinkError>> {
        Box::pin(async move {
            // The JSON publish API (POST to the server root) keeps non-ASCII
            // titles intact, which the header-based API cannot.
            let mut body = json!({
                "topic": self.cfg.topic,
                "title": format!("{}: {}", notice.instance, notice.event.title()),
                "message": notice.event.body(),
                "priority": notice.event.priority(),
                "tags": [notice.event.tags()],
            });
            if let Some(click) = &self.cfg.click {
                body["click"] = json!(click);
            }
            let url = self.cfg.server.trim_end_matches('/').to_owned() + "/";
            let mut req = self.client.post(url).json(&body);
            if let Some(token) = &self.cfg.token {
                req = req.bearer_auth(token);
            } else if let (Some(u), Some(p)) = (&self.cfg.username, &self.cfg.password) {
                req = req.basic_auth(u, Some(p));
            }
            let resp = req.send().await.map_err(request_error)?;
            check_status(resp.status())
        })
    }
}

/// Sends a plain-text mail over SMTP.
pub struct EmailSink {
    cfg: EmailConfig,
    from: Mailbox,
    to: Vec<Mailbox>,
    transport: AsyncSmtpTransport<Tokio1Executor>,
}

impl EmailSink {
    /// Creates the sink. No connection is made until the first send.
    ///
    /// # Errors
    ///
    /// Fails on an invalid address or host name.
    pub fn new(cfg: EmailConfig, timeout: Duration) -> Result<Self, String> {
        let from: Mailbox = cfg
            .from
            .parse()
            .map_err(|e| format!("invalid from address: {e}"))?;
        let to = cfg
            .to
            .iter()
            .map(|t| t.parse().map_err(|e| format!("invalid address {t:?}: {e}")))
            .collect::<Result<Vec<Mailbox>, _>>()?;
        let builder = match cfg.security {
            EmailSecurity::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.smtp_host),
            EmailSecurity::Starttls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.smtp_host)
            }
            EmailSecurity::None => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
                &cfg.smtp_host,
            )),
        }
        .map_err(|e| format!("invalid SMTP host: {e}"))?;
        let mut builder = builder.port(cfg.port()).timeout(Some(timeout));
        if let (Some(u), Some(p)) = (&cfg.username, &cfg.password) {
            builder = builder.credentials(Credentials::new(u.clone(), p.clone()));
        }
        Ok(Self {
            transport: builder.build(),
            cfg,
            from,
            to,
        })
    }
}

impl Sink for EmailSink {
    fn name(&self) -> &str {
        &self.cfg.name
    }

    fn kind(&self) -> &'static str {
        "email"
    }

    fn filter(&self) -> &EventFilter {
        &self.cfg.events
    }

    fn send<'a>(&'a self, notice: &'a Notice) -> BoxFuture<'a, Result<(), SinkError>> {
        Box::pin(async move {
            let mut msg = Message::builder().from(self.from.clone()).subject(format!(
                "[{}] {}",
                notice.instance,
                notice.event.title()
            ));
            for to in &self.to {
                msg = msg.to(to.clone());
            }
            let at = notice
                .at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default();
            let text = format!(
                "{}\n\nEvent: {}\nTime: {at}\nServer: {}\n",
                notice.event.body(),
                notice.event.kind(),
                notice.instance
            );
            let msg = msg
                .header(ContentType::TEXT_PLAIN)
                .body(text)
                .map_err(|e| SinkError::Permanent(format!("building mail: {e}")))?;
            match self.transport.send(msg).await {
                Ok(_) => Ok(()),
                Err(e) if e.is_permanent() => Err(SinkError::Permanent(e.to_string())),
                Err(e) => Err(SinkError::Transient(e.to_string())),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_stable() {
        // Known answer: HMAC-SHA256(key="secret", msg="1700000000.{}").
        let sig = sign_payload(b"secret", 1_700_000_000, b"{}");
        assert!(sig.starts_with("sha256="));
        assert_eq!(sig.len(), "sha256=".len() + 64);
        assert_eq!(sig, sign_payload(b"secret", 1_700_000_000, b"{}"));
        assert_ne!(sig, sign_payload(b"other", 1_700_000_000, b"{}"));
        assert_ne!(sig, sign_payload(b"secret", 1_700_000_001, b"{}"));
    }

    #[test]
    fn status_mapping() {
        assert!(check_status(StatusCode::NO_CONTENT).is_ok());
        assert!(
            check_status(StatusCode::BAD_GATEWAY)
                .unwrap_err()
                .is_transient()
        );
        assert!(
            check_status(StatusCode::TOO_MANY_REQUESTS)
                .unwrap_err()
                .is_transient()
        );
        assert!(
            !check_status(StatusCode::NOT_FOUND)
                .unwrap_err()
                .is_transient()
        );
    }
}
