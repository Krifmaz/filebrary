//! Prometheus metrics.
//!
//! Every crate records through the [`metrics`] facade
//! (`metrics::counter!("filebrary_...").increment(1)`), and this module
//! installs one process-wide Prometheus recorder and serves it at
//! `/metrics`. Metric names start with `filebrary_`.
//!
//! Recorded here:
//!
//! * `filebrary_http_requests_total{method,route,status}`: requests, by the
//!   *route pattern* (for example `/api/sources/{id}/files`), never the raw
//!   path, so share tokens and file names do not become labels.
//! * `filebrary_http_request_duration_seconds{method,route}`: latency histogram.
//! * `filebrary_http_requests_in_flight`.
//! * `filebrary_build_info{version}`, `filebrary_uptime_seconds`.
//! * `filebrary_process_resident_memory_bytes`,
//!   `filebrary_process_peak_resident_memory_bytes`,
//!   `filebrary_process_open_fds` (Linux only).
//! * `filebrary_db_connections{state}`, `filebrary_sources`,
//!   `filebrary_notification_targets`.
//!
//! Recorded elsewhere: `filebrary_events_published_total{kind}`,
//! `filebrary_events_dropped_total`, `filebrary_notifications_sent_total`,
//! `filebrary_notifications_failed_total` (events crate), and
//! `filebrary_ftp_logins_total{result}`, `filebrary_ftp_transfers_total{direction}`
//! (FTP crate).

use std::{sync::OnceLock, time::Instant};

use axum::{
    extract::{MatchedPath, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};

use crate::{crypto, state::AppState};

/// Latency buckets in seconds: from 1 ms (cached metadata) to 60 s (large
/// zip downloads).
const BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0,
];

static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// A handle to the process-wide Prometheus recorder.
#[derive(Clone)]
pub struct Metrics {
    handle: PrometheusHandle,
}

impl Metrics {
    /// Installs the recorder on first call and returns a handle to it.
    /// Later calls (several app states in tests) share the same recorder.
    pub fn install() -> Self {
        let handle = HANDLE
            .get_or_init(|| {
                let recorder = PrometheusBuilder::new()
                    .set_buckets_for_metric(Matcher::Suffix("_duration_seconds".into()), BUCKETS)
                    .expect("bucket list is not empty")
                    .build_recorder();
                let handle = recorder.handle();
                if metrics::set_global_recorder(recorder).is_err() {
                    tracing::warn!(
                        "a metrics recorder was already installed; /metrics may be empty"
                    );
                }
                handle
            })
            .clone();
        Self { handle }
    }

    /// Renders all metrics in the Prometheus text format.
    pub fn render(&self) -> String {
        self.handle.run_upkeep();
        self.handle.render()
    }
}

/// Middleware recording request count, latency and in-flight requests.
/// Added with `Router::layer` so it sees the matched route pattern.
pub async fn track(req: Request, next: Next) -> Response {
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "unmatched".to_owned(), |m| m.as_str().to_owned());
    let method = req.method().as_str().to_owned();
    let start = Instant::now();
    let in_flight = metrics::gauge!("filebrary_http_requests_in_flight");
    in_flight.increment(1);
    let resp = next.run(req).await;
    in_flight.decrement(1);
    let status = resp.status().as_u16().to_string();
    metrics::histogram!(
        "filebrary_http_request_duration_seconds",
        "method" => method.clone(),
        "route" => route.clone(),
    )
    .record(start.elapsed().as_secs_f64());
    metrics::counter!(
        "filebrary_http_requests_total",
        "method" => method,
        "route" => route,
        "status" => status,
    )
    .increment(1);
    resp
}

/// `GET /metrics`: the Prometheus scrape endpoint.
///
/// Returns 404 when `metrics.enabled` is false, and 401 without the right
/// bearer token when `metrics.token` is set.
pub async fn render(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let cfg = &state.config.metrics;
    if !cfg.enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Some(token) = &cfg.token {
        let sent = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or_default();
        if !crypto::ct_eq(sent.as_bytes(), token.as_bytes()) {
            return (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, "Bearer")],
                "metrics token required\n",
            )
                .into_response();
        }
    }
    record_gauges(&state);
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        state.metrics.render(),
    )
        .into_response()
}

/// Sets the gauges that are cheapest to read at scrape time.
fn record_gauges(state: &AppState) {
    metrics::gauge!("filebrary_build_info", "version" => env!("CARGO_PKG_VERSION")).set(1.0);
    metrics::gauge!("filebrary_uptime_seconds").set(state.started_at.elapsed().as_secs_f64());
    metrics::gauge!("filebrary_sources").set(state.sources.len() as f64);
    metrics::gauge!("filebrary_notification_targets")
        .set(state.notifier.as_ref().map_or(0, |n| n.sinks().len()) as f64);
    let size = state.db.size();
    let idle = state.db.num_idle() as u32;
    metrics::gauge!("filebrary_db_connections", "state" => "idle").set(f64::from(idle));
    metrics::gauge!("filebrary_db_connections", "state" => "busy")
        .set(f64::from(size.saturating_sub(idle)));
    if let Some(p) = process_stats() {
        metrics::gauge!("filebrary_process_resident_memory_bytes").set(p.rss as f64);
        metrics::gauge!("filebrary_process_peak_resident_memory_bytes").set(p.peak_rss as f64);
        metrics::gauge!("filebrary_process_open_fds").set(p.open_fds as f64);
    }
}

/// Process statistics from `/proc` (Linux).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessStats {
    /// Resident memory in bytes.
    pub rss: u64,
    /// Highest resident memory since start, in bytes.
    pub peak_rss: u64,
    /// Open file descriptors.
    pub open_fds: u64,
}

/// Reads [`ProcessStats`]; `None` on platforms without `/proc`.
pub fn process_stats() -> Option<ProcessStats> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kib = |key: &str| -> Option<u64> {
        let line = status.lines().find(|l| l.starts_with(key))?;
        let n: u64 = line[key.len()..]
            .trim()
            .trim_end_matches("kB")
            .trim()
            .parse()
            .ok()?;
        Some(n * 1024)
    };
    let open_fds = std::fs::read_dir("/proc/self/fd").map_or(0, |d| d.count() as u64);
    Some(ProcessStats {
        rss: kib("VmRSS:")?,
        peak_rss: kib("VmHWM:")?,
        open_fds,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(target_os = "linux")]
    fn reads_process_stats() {
        let p = super::process_stats().unwrap();
        assert!(p.rss > 0 && p.peak_rss >= p.rss && p.open_fds > 0);
    }
}
