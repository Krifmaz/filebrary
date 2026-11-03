//! Structured logging.
//!
//! Logs go to standard output through `tracing`. `FILEBRARY_LOG` sets the
//! filter (`info` by default, for example `debug` or
//! `info,filebrary::ops=debug`) and `FILEBRARY_LOG_FORMAT` (or `log_format`)
//! chooses `text` or `json`. In JSON mode each line is one object with
//! `timestamp`, `level`, `target`, `fields` and the active `span`s, ready
//! for Loki, Elasticsearch or `jq`.
//!
//! Each HTTP request runs in a `request` span carrying `request_id`,
//! `method` and `route` (the route *pattern*, so share tokens and file names
//! never reach the logs), and ends with one `request finished` line with
//! `status` and `latency_ms`. The request id comes from an incoming
//! `X-Request-Id` header (trusted only up to 64 visible ASCII characters)
//! or is generated, and is echoed in the response.

use std::time::Duration;

use axum::{
    Router,
    extract::MatchedPath,
    http::{HeaderName, Request, Response},
};
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, RequestId, SetRequestIdLayer},
    trace::TraceLayer,
};
use tracing::Span;
use tracing_subscriber::EnvFilter;

use crate::config::LogFormat;

/// Header carrying the request id.
pub const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// Installs the global subscriber. Call once, at start-up.
pub fn init(format: LogFormat) {
    let filter = EnvFilter::try_from_env("FILEBRARY_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info,sqlx=warn,libunftp=warn"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    let result = match format {
        LogFormat::Json => builder
            .json()
            .with_current_span(true)
            .with_span_list(false)
            .try_init(),
        LogFormat::Text => builder.try_init(),
    };
    if result.is_err() {
        // Already installed (tests); keep the existing one.
    }
}

/// Wraps `router` with request ids and per-request spans and log lines.
pub fn layer<S: Clone + Send + Sync + 'static>(router: Router<S>) -> Router<S> {
    router
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|req: &Request<_>| {
                    let route = req
                        .extensions()
                        .get::<MatchedPath>()
                        .map_or("unmatched", |m| m.as_str());
                    let id = req
                        .extensions()
                        .get::<RequestId>()
                        .and_then(|id| id.header_value().to_str().ok())
                        .unwrap_or("-");
                    tracing::info_span!(
                        "request",
                        request_id = %id,
                        method = %req.method(),
                        route = %route,
                    )
                })
                .on_request(())
                .on_response(|resp: &Response<_>, latency: Duration, _span: &Span| {
                    let status = resp.status().as_u16();
                    let ms = latency.as_secs_f64() * 1000.0;
                    if status >= 500 {
                        tracing::error!(status, latency_ms = ms, "request finished");
                    } else {
                        tracing::info!(status, latency_ms = ms, "request finished");
                    }
                })
                .on_failure(()),
        )
        .layer(PropagateRequestIdLayer::new(REQUEST_ID))
        .layer(SetRequestIdLayer::new(REQUEST_ID, MakeRequestUuid))
        .layer(axum::middleware::map_request(sanitise_request_id))
}

/// Drops a client-supplied request id unless it is 1 to 64 visible ASCII
/// characters, so a new one is generated. Stops log injection through the
/// header.
async fn sanitise_request_id(mut req: Request<axum::body::Body>) -> Request<axum::body::Body> {
    let ok = req.headers().get(&REQUEST_ID).is_some_and(|v| {
        let b = v.as_bytes();
        !b.is_empty() && b.len() <= 64 && b.iter().all(|c| c.is_ascii_graphic())
    });
    if !ok {
        req.headers_mut().remove(&REQUEST_ID);
    }
    req
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};

    use super::*;

    #[tokio::test]
    async fn keeps_only_sane_request_ids() {
        let ok = Request::get("/")
            .header(REQUEST_ID, "abc-123")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            sanitise_request_id(ok).await.headers()[&REQUEST_ID],
            "abc-123"
        );
        for bad in ["a b", &"x".repeat(65), "tab\there"] {
            let req = Request::get("/")
                .header(REQUEST_ID, bad)
                .body(Body::empty())
                .unwrap();
            assert!(
                sanitise_request_id(req)
                    .await
                    .headers()
                    .get(&REQUEST_ID)
                    .is_none()
            );
        }
    }
}
