//! Event types and the in-process event bus.

use std::fmt;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tokio::sync::broadcast;

/// Something that happened in Filebrary that someone may want to hear about.
///
/// Events are serialised with an internal `type` tag, so a webhook receives
/// for example:
///
/// ```json
/// {"type":"upload","user":"alice","source":"media","path":"/a.txt","size":12}
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A file finished uploading (over HTTP, WebDAV or FTP).
    Upload {
        /// Name of the user who uploaded the file, or `share:<id>` for an
        /// anonymous upload through a drop link.
        user: String,
        /// Name of the storage source the file was written to.
        source: String,
        /// Path of the file inside the source, starting with `/`.
        path: String,
        /// Size of the file in bytes.
        size: u64,
    },
    /// Someone opened a share link.
    ShareOpened {
        /// Identifier of the share.
        share_id: String,
        /// Name of the user who created the share.
        owner: String,
        /// Name of the storage source the shared item lives in.
        source: String,
        /// Path of the shared item inside the source.
        path: String,
        /// Address of the visitor, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote_addr: Option<String>,
    },
    /// A user's storage use crossed the warning threshold of their quota.
    QuotaNearFull {
        /// Name of the user.
        user: String,
        /// Bytes in use after the write that crossed the threshold.
        used_bytes: u64,
        /// The user's quota in bytes.
        quota_bytes: u64,
        /// The threshold that was crossed, in percent of the quota.
        threshold_percent: u8,
    },
    /// A background job failed after its last retry.
    JobFailed {
        /// Identifier of the job.
        job_id: String,
        /// Kind of job, such as `convert`, `index` or `archive`.
        kind: String,
        /// User who started the job, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
        /// The error message of the last attempt.
        error: String,
    },
    /// A test notification sent by an administrator to check a target.
    /// Every sink accepts it regardless of its event filter.
    Test {
        /// Free text from the administrator.
        message: String,
    },
}

/// The kind of an [`Event`], without its data. Used in sink filters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// See [`Event::Upload`].
    Upload,
    /// See [`Event::ShareOpened`].
    ShareOpened,
    /// See [`Event::QuotaNearFull`].
    QuotaNearFull,
    /// See [`Event::JobFailed`].
    JobFailed,
    /// See [`Event::Test`].
    Test,
}

impl EventKind {
    /// Every kind, in declaration order.
    pub const ALL: [EventKind; 5] = [
        EventKind::Upload,
        EventKind::ShareOpened,
        EventKind::QuotaNearFull,
        EventKind::JobFailed,
        EventKind::Test,
    ];

    /// The `snake_case` name used in configuration and in the `type` field.
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Upload => "upload",
            EventKind::ShareOpened => "share_opened",
            EventKind::QuotaNearFull => "quota_near_full",
            EventKind::JobFailed => "job_failed",
            EventKind::Test => "test",
        }
    }
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Event {
    /// The kind of this event.
    pub fn kind(&self) -> EventKind {
        match self {
            Event::Upload { .. } => EventKind::Upload,
            Event::ShareOpened { .. } => EventKind::ShareOpened,
            Event::QuotaNearFull { .. } => EventKind::QuotaNearFull,
            Event::JobFailed { .. } => EventKind::JobFailed,
            Event::Test { .. } => EventKind::Test,
        }
    }

    /// A short one-line title, used as the ntfy title and the mail subject.
    pub fn title(&self) -> String {
        match self {
            Event::Upload { user, .. } => format!("New upload by {user}"),
            Event::ShareOpened { .. } => "Share link opened".to_owned(),
            Event::QuotaNearFull { user, .. } => format!("Storage almost full for {user}"),
            Event::JobFailed { kind, .. } => format!("Job failed: {kind}"),
            Event::Test { .. } => "Filebrary test notification".to_owned(),
        }
    }

    /// A human-readable description of the event.
    pub fn body(&self) -> String {
        match self {
            Event::Upload {
                user,
                source,
                path,
                size,
            } => format!("{user} uploaded {source}:{path} ({})", human_bytes(*size)),
            Event::ShareOpened {
                share_id,
                owner,
                source,
                path,
                remote_addr,
            } => {
                let from = remote_addr
                    .as_deref()
                    .map(|a| format!(" from {a}"))
                    .unwrap_or_default();
                format!("Share {share_id} of {source}:{path} (owner {owner}) was opened{from}")
            }
            Event::QuotaNearFull {
                user,
                used_bytes,
                quota_bytes,
                threshold_percent,
            } => format!(
                "{user} is using {} of {} (over {threshold_percent}%)",
                human_bytes(*used_bytes),
                human_bytes(*quota_bytes)
            ),
            Event::JobFailed {
                job_id,
                kind,
                user,
                error,
            } => {
                let by = user
                    .as_deref()
                    .map(|u| format!(" started by {u}"))
                    .unwrap_or_default();
                format!("Job {job_id} ({kind}){by} failed: {error}")
            }
            Event::Test { message } => message.clone(),
        }
    }

    /// ntfy priority, 1 (min) to 5 (max).
    pub fn priority(&self) -> u8 {
        match self {
            Event::Upload { .. } | Event::ShareOpened { .. } | Event::Test { .. } => 3,
            Event::QuotaNearFull { .. } | Event::JobFailed { .. } => 4,
        }
    }

    /// ntfy tags (emoji short codes) for the event.
    pub fn tags(&self) -> &'static str {
        match self {
            Event::Upload { .. } => "inbox_tray",
            Event::ShareOpened { .. } => "link",
            Event::QuotaNearFull { .. } => "warning",
            Event::JobFailed { .. } => "x",
            Event::Test { .. } => "white_check_mark",
        }
    }
}

/// Returns `true` when a write that moves a user's usage from `before` to
/// `after` bytes crosses `threshold_percent` of `quota` bytes.
///
/// Call this from the quota check and publish [`Event::QuotaNearFull`] only
/// when it returns `true`, so a user near the limit is told once rather than
/// on every upload. A quota of zero means "unlimited" and never triggers.
pub fn quota_crossed(before: u64, after: u64, quota: u64, threshold_percent: u8) -> bool {
    if quota == 0 {
        return false;
    }
    let limit = (quota as u128) * (threshold_percent.min(100) as u128) / 100;
    (before as u128) < limit && (after as u128) >= limit
}

/// An [`Event`] together with when it happened and which server sent it.
/// This is what sinks receive and what a webhook body contains.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    /// When the event was published.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// The configured instance name (for example the branding name), so a
    /// receiver that listens to several servers can tell them apart.
    pub instance: String,
    /// The event itself, flattened into the same JSON object.
    #[serde(flatten)]
    pub event: Event,
}

/// Default capacity of the bus: how many events a lagging dispatcher may fall
/// behind before the oldest are dropped.
const DEFAULT_CAPACITY: usize = 1024;

/// A cheap-to-clone handle for publishing events.
///
/// Put one in the application state; every route, the WebDAV and FTP
/// servers, and the job runner publish through it.
#[derive(Clone, Debug)]
pub struct EventBus {
    tx: broadcast::Sender<Notice>,
    instance: String,
}

impl EventBus {
    /// Creates a bus with the default capacity.
    pub fn new(instance: impl Into<String>) -> Self {
        Self::with_capacity(instance, DEFAULT_CAPACITY)
    }

    /// Creates a bus that buffers up to `capacity` events per subscriber.
    pub fn with_capacity(instance: impl Into<String>, capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(1));
        Self {
            tx,
            instance: instance.into(),
        }
    }

    /// Publishes an event. Never blocks and never fails; if nobody is
    /// subscribed the event is simply discarded.
    pub fn publish(&self, event: Event) {
        let kind = event.kind();
        metrics::counter!("filebrary_events_published_total", "kind" => kind.as_str()).increment(1);
        let notice = Notice {
            at: OffsetDateTime::now_utc(),
            instance: self.instance.clone(),
            event,
        };
        // An error only means there are no subscribers.
        let _ = self.tx.send(notice);
    }

    /// Subscribes to every event published from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<Notice> {
        self.tx.subscribe()
    }
}

/// Formats a byte count with binary units, for messages.
pub(crate) fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialises_with_type_tag() {
        let e = Event::Upload {
            user: "alice".into(),
            source: "media".into(),
            path: "/a.txt".into(),
            size: 12,
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "upload");
        assert_eq!(v["size"], 12);
        let back: Event = serde_json::from_value(v).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn notice_flattens_event() {
        let n = Notice {
            at: OffsetDateTime::now_utc(),
            instance: "home".into(),
            event: Event::JobFailed {
                job_id: "7".into(),
                kind: "convert".into(),
                user: None,
                error: "boom".into(),
            },
        };
        let v = serde_json::to_value(&n).unwrap();
        assert_eq!(v["type"], "job_failed");
        assert_eq!(v["instance"], "home");
        assert!(v.get("user").is_none());
        assert!(v["at"].is_string());
    }

    #[test]
    fn kind_names_match_serde() {
        for kind in EventKind::ALL {
            let s = serde_json::to_value(kind).unwrap();
            assert_eq!(s, kind.as_str());
        }
    }

    #[test]
    fn quota_threshold_fires_once() {
        let q = 1000;
        assert!(!quota_crossed(0, 899, q, 90));
        assert!(quota_crossed(899, 900, q, 90));
        assert!(quota_crossed(100, 2000, q, 90));
        assert!(!quota_crossed(900, 950, q, 90));
        assert!(!quota_crossed(0, u64::MAX, 0, 90));
    }

    #[test]
    fn human_bytes_formats() {
        assert_eq!(human_bytes(12), "12 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(5 * 1024 * 1024 * 1024), "5.0 GiB");
    }

    #[tokio::test]
    async fn bus_delivers_to_subscribers() {
        let bus = EventBus::new("test");
        let mut rx = bus.subscribe();
        bus.publish(Event::QuotaNearFull {
            user: "bob".into(),
            used_bytes: 9,
            quota_bytes: 10,
            threshold_percent: 90,
        });
        let n = rx.recv().await.unwrap();
        assert_eq!(n.event.kind(), EventKind::QuotaNearFull);
        assert_eq!(n.instance, "test");
    }

    #[test]
    fn publish_without_subscribers_is_fine() {
        EventBus::new("x").publish(Event::ShareOpened {
            share_id: "s".into(),
            owner: "o".into(),
            source: "src".into(),
            path: "/".into(),
            remote_addr: None,
        });
    }
}
