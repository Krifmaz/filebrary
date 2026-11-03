//! Events and notifications for Filebrary.
//!
//! Other parts of the server describe things that happened as an [`Event`]
//! and hand it to the [`EventBus`] with [`EventBus::publish`]. Publishing never
//! blocks and never fails: the bus is a bounded broadcast channel, and a slow
//! consumer only loses the oldest events (a warning is logged and the
//! `filebrary_events_dropped_total` counter goes up).
//!
//! A [`Dispatcher`] subscribes to the bus and delivers each event to every
//! configured [`Sink`] whose filter accepts it:
//!
//! * [`WebhookSink`] posts the event as JSON, optionally signed with
//!   HMAC-SHA256 (`X-Filebrary-Signature: sha256=<hex>`).
//! * [`NtfySink`] posts a short human-readable message to an
//!   [ntfy](https://ntfy.sh) topic.
//! * [`EmailSink`] sends a plain-text mail over SMTP (implicit TLS, STARTTLS,
//!   or plain for a local relay).
//!
//! Delivery is retried with exponential backoff; failures are logged and
//! counted, never propagated back to the code that published the event.
//!
//! All targets are configured by the administrator in the server's
//! configuration file ([`NotificationsConfig`]). Nothing here runs a shell or
//! takes a destination from a request.

#![warn(missing_docs)]

mod config;
mod dispatcher;
mod event;
mod sinks;

pub use config::{
    EmailConfig, EmailSecurity, EventFilter, NotificationsConfig, NtfyConfig, RetryConfig,
    WebhookConfig,
};
pub use dispatcher::{Dispatcher, DispatcherHandle};
pub use event::{Event, EventBus, EventKind, Notice, quota_crossed};
pub use sinks::{EmailSink, NtfySink, Sink, SinkError, WebhookSink, sign_payload};
