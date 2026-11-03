//! Delivers events from the bus to the configured sinks.

use std::{sync::Arc, time::Duration};

use tokio::{
    sync::{Semaphore, broadcast::error::RecvError},
    task::JoinHandle,
};

use crate::{
    Event, EventBus, Notice,
    config::{NotificationsConfig, RetryConfig},
    sinks::{EmailSink, NtfySink, Sink, SinkError, WebhookSink},
};

/// Deliveries running at the same time, across all sinks. Keeps a burst of
/// uploads from opening hundreds of connections.
const MAX_IN_FLIGHT: usize = 16;

/// Routes events to sinks. Build it with [`Dispatcher::from_config`] (or
/// [`Dispatcher::new`] with your own sinks) and start it with
/// [`Dispatcher::spawn`].
pub struct Dispatcher {
    sinks: Vec<Arc<dyn Sink>>,
    retry: RetryConfig,
    instance: String,
}

/// A running dispatcher. Dropping the handle does not stop it; it stops
/// when every [`EventBus`] clone is dropped.
pub struct DispatcherHandle {
    inner: Arc<Dispatcher>,
    task: JoinHandle<()>,
}

impl Dispatcher {
    /// Builds the sinks described by `cfg`.
    ///
    /// # Errors
    ///
    /// Fails if the configuration is invalid or the HTTP client cannot be
    /// built.
    pub fn from_config(cfg: &NotificationsConfig) -> Result<Self, String> {
        cfg.validate()?;
        // reqwest and lettre are built without a bundled crypto provider;
        // use ring (pure Rust and assembly, no system libraries). Ignored if
        // the process already installed one.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let timeout = Duration::from_secs(cfg.retry.timeout_secs);
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .user_agent(concat!("filebrary/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::limited(3))
            .build()
            .map_err(|e| format!("building HTTP client: {e}"))?;
        let mut sinks: Vec<Arc<dyn Sink>> = Vec::new();
        for w in &cfg.webhooks {
            sinks.push(Arc::new(
                WebhookSink::new(w.clone(), client.clone())
                    .map_err(|e| format!("webhook {:?}: {e}", w.name))?,
            ));
        }
        for n in &cfg.ntfy {
            sinks.push(Arc::new(NtfySink::new(n.clone(), client.clone())));
        }
        for e in &cfg.email {
            sinks.push(Arc::new(
                EmailSink::new(e.clone(), timeout)
                    .map_err(|err| format!("email {:?}: {err}", e.name))?,
            ));
        }
        Ok(Self::new(sinks, cfg.retry.clone(), cfg.instance.clone()))
    }

    /// Builds a dispatcher over the given sinks.
    pub fn new(sinks: Vec<Arc<dyn Sink>>, retry: RetryConfig, instance: String) -> Self {
        Self {
            sinks,
            retry,
            instance,
        }
    }

    /// The configured sinks, for listing in the admin API.
    pub fn sinks(&self) -> &[Arc<dyn Sink>] {
        &self.sinks
    }

    /// Subscribes to `bus` and delivers events in the background until every
    /// clone of the bus is dropped.
    pub fn spawn(self, bus: &EventBus) -> DispatcherHandle {
        let inner = Arc::new(self);
        let mut rx = bus.subscribe();
        let me = inner.clone();
        let task = tokio::spawn(async move {
            let permits = Arc::new(Semaphore::new(MAX_IN_FLIGHT));
            loop {
                let notice = match rx.recv().await {
                    Ok(n) => Arc::new(n),
                    Err(RecvError::Lagged(n)) => {
                        tracing::warn!(
                            dropped = n,
                            "notification queue overflowed; events dropped"
                        );
                        metrics::counter!("filebrary_events_dropped_total").increment(n);
                        continue;
                    }
                    Err(RecvError::Closed) => break,
                };
                for sink in me.sinks.iter().filter(|s| s.accepts(notice.event.kind())) {
                    let Ok(permit) = permits.clone().acquire_owned().await else {
                        return;
                    };
                    let sink = sink.clone();
                    let notice = notice.clone();
                    let retry = me.retry.clone();
                    tokio::spawn(async move {
                        let _ = deliver(sink.as_ref(), &notice, &retry).await;
                        drop(permit);
                    });
                }
            }
        });
        DispatcherHandle { inner, task }
    }
}

impl DispatcherHandle {
    /// The configured sinks.
    pub fn sinks(&self) -> &[Arc<dyn Sink>] {
        self.inner.sinks()
    }

    /// Sends a [`Event::Test`] to the sink named `name` right away (one
    /// attempt, no retries) and reports the outcome, for the admin "send
    /// test" button.
    ///
    /// # Errors
    ///
    /// Returns the delivery error, or a message if no sink has that name.
    pub async fn send_test(&self, name: &str, message: String) -> Result<(), String> {
        let sink = self
            .inner
            .sinks
            .iter()
            .find(|s| s.name() == name)
            .ok_or_else(|| format!("no notification target named {name:?}"))?;
        let notice = Notice {
            at: time::OffsetDateTime::now_utc(),
            instance: self.inner.instance.clone(),
            event: Event::Test { message },
        };
        let once = RetryConfig {
            attempts: 1,
            ..self.inner.retry.clone()
        };
        deliver(sink.as_ref(), &notice, &once)
            .await
            .map_err(|e| e.to_string())
    }

    /// Whether the background task has stopped.
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

/// Delivers one notice to one sink with retries and records the outcome.
async fn deliver(sink: &dyn Sink, notice: &Notice, retry: &RetryConfig) -> Result<(), SinkError> {
    let kind = notice.event.kind().as_str();
    let mut backoff = Duration::from_millis(retry.initial_backoff_ms);
    let attempts = retry.attempts.max(1);
    let timeout = Duration::from_secs(retry.timeout_secs.max(1));
    let mut attempt = 1;
    loop {
        // The sinks carry their own timeouts; this is a backstop.
        let result =
            match tokio::time::timeout(timeout + Duration::from_secs(1), sink.send(notice)).await {
                Ok(r) => r,
                Err(_) => Err(SinkError::Transient("timed out".into())),
            };
        match result {
            Ok(()) => {
                tracing::debug!(
                    sink = sink.name(),
                    event = kind,
                    attempt,
                    "notification sent"
                );
                metrics::counter!(
                    "filebrary_notifications_sent_total",
                    "sink" => sink.name().to_owned(),
                    "target" => sink.kind(),
                    "event" => kind,
                )
                .increment(1);
                return Ok(());
            }
            Err(e) if e.is_transient() && attempt < attempts => {
                tracing::debug!(sink = sink.name(), event = kind, attempt, error = %e, "notification failed, retrying");
                tokio::time::sleep(backoff).await;
                backoff = backoff.saturating_mul(2);
                attempt += 1;
            }
            Err(e) => {
                tracing::warn!(sink = sink.name(), target = sink.kind(), event = kind, attempt, error = %e, "notification failed");
                metrics::counter!(
                    "filebrary_notifications_failed_total",
                    "sink" => sink.name().to_owned(),
                    "target" => sink.kind(),
                    "event" => kind,
                )
                .increment(1);
                return Err(e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicU32, Ordering},
    };

    use futures::future::BoxFuture;

    use super::*;
    use crate::{EventKind, config::EventFilter};

    /// A sink that records what it receives and fails the first `fail` times.
    struct Recorder {
        name: String,
        filter: EventFilter,
        fail: AtomicU32,
        permanent: bool,
        got: Mutex<Vec<EventKind>>,
        calls: AtomicU32,
    }

    impl Recorder {
        fn new(name: &str, filter: EventFilter, fail: u32, permanent: bool) -> Arc<Self> {
            Arc::new(Self {
                name: name.into(),
                filter,
                fail: AtomicU32::new(fail),
                permanent,
                got: Mutex::new(Vec::new()),
                calls: AtomicU32::new(0),
            })
        }
    }

    impl Sink for Recorder {
        fn name(&self) -> &str {
            &self.name
        }
        fn kind(&self) -> &'static str {
            "test"
        }
        fn filter(&self) -> &EventFilter {
            &self.filter
        }
        fn send<'a>(&'a self, n: &'a Notice) -> BoxFuture<'a, Result<(), SinkError>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if self
                    .fail
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |f| f.checked_sub(1))
                    .is_ok()
                {
                    return Err(if self.permanent {
                        SinkError::Permanent("no".into())
                    } else {
                        SinkError::Transient("later".into())
                    });
                }
                self.got.lock().unwrap().push(n.event.kind());
                Ok(())
            })
        }
    }

    fn fast_retry() -> RetryConfig {
        RetryConfig {
            attempts: 3,
            initial_backoff_ms: 1,
            timeout_secs: 5,
        }
    }

    fn upload() -> Event {
        Event::Upload {
            user: "a".into(),
            source: "s".into(),
            path: "/f".into(),
            size: 1,
        }
    }

    fn failed_job() -> Event {
        Event::JobFailed {
            job_id: "1".into(),
            kind: "convert".into(),
            user: None,
            error: "x".into(),
        }
    }

    async fn wait_for(cond: impl Fn() -> bool) {
        for _ in 0..200 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("condition not met in time");
    }

    #[tokio::test]
    async fn routes_by_filter() {
        let all = Recorder::new("all", vec![], 0, false);
        let jobs = Recorder::new("jobs", vec![EventKind::JobFailed], 0, false);
        let bus = EventBus::new("t");
        let _h =
            Dispatcher::new(vec![all.clone(), jobs.clone()], fast_retry(), "t".into()).spawn(&bus);
        bus.publish(upload());
        bus.publish(failed_job());
        wait_for(|| all.got.lock().unwrap().len() == 2 && jobs.got.lock().unwrap().len() == 1)
            .await;
        assert_eq!(*jobs.got.lock().unwrap(), [EventKind::JobFailed]);
    }

    #[tokio::test]
    async fn retries_transient_failures() {
        let flaky = Recorder::new("flaky", vec![], 2, false);
        let bus = EventBus::new("t");
        let _h = Dispatcher::new(vec![flaky.clone()], fast_retry(), "t".into()).spawn(&bus);
        bus.publish(upload());
        wait_for(|| flaky.got.lock().unwrap().len() == 1).await;
        assert_eq!(flaky.calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn does_not_retry_permanent_failures() {
        let s = Recorder::new("p", vec![], 5, true);
        let err = deliver(
            s.as_ref(),
            &Notice {
                at: time::OffsetDateTime::now_utc(),
                instance: "t".into(),
                event: upload(),
            },
            &fast_retry(),
        )
        .await;
        assert!(err.is_err());
        assert_eq!(s.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn gives_up_after_attempts() {
        let s = Recorder::new("down", vec![], 100, false);
        let n = Notice {
            at: time::OffsetDateTime::now_utc(),
            instance: "t".into(),
            event: upload(),
        };
        assert!(deliver(s.as_ref(), &n, &fast_retry()).await.is_err());
        assert_eq!(s.calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_event_bypasses_filter() {
        let jobs = Recorder::new("jobs", vec![EventKind::JobFailed], 0, false);
        let bus = EventBus::new("t");
        let h = Dispatcher::new(vec![jobs.clone()], fast_retry(), "t".into()).spawn(&bus);
        h.send_test("jobs", "hello".into()).await.unwrap();
        assert_eq!(*jobs.got.lock().unwrap(), [EventKind::Test]);
        assert!(h.send_test("missing", "x".into()).await.is_err());
    }

    #[tokio::test]
    async fn stops_when_bus_dropped() {
        let bus = EventBus::new("t");
        let h = Dispatcher::new(vec![], fast_retry(), "t".into()).spawn(&bus);
        drop(bus);
        wait_for(|| h.is_finished()).await;
    }
}
