# Logs, health and metrics

## Logs

Filebrary logs to standard output. Docker keeps them for `docker logs`;
systemd sends them to the journal (`journalctl -u filebrary`).

* **Format:** human-readable text by default. Set
  [`log_format`](../configuration/reference.md#log_format) (or
  `FILEBRARY_LOG_FORMAT`) to `json` for one JSON object per line, with the
  level, target, message and fields.
* **Level:** set `FILEBRARY_LOG` to a filter in
  [`EnvFilter` syntax](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html).
  The default is `info,tower_http=info,sqlx=warn`. Useful values:

  | Value                         | Shows                                         |
  |-------------------------------|-----------------------------------------------|
  | `warn`                        | Only problems.                                |
  | `info`                        | Start-up, every request, background jobs.     |
  | `debug`                       | Everything above, plus internal detail.       |
  | `info,filebrary=debug`        | Debug detail from Filebrary only.             |

Every request gets an `X-Request-Id` header (kept from the client or proxy
when it is 1 to 64 visible ASCII characters, otherwise generated) and one
`request finished` line with `request_id`, `method`, `route` (the route
pattern such as `/api/files/{source}`, never the raw path), `status` and
`latency_ms`. Search the logs for a request id to follow one request.

Filebrary is written not to log passwords, session tokens or share
passwords. Request logs include paths, so file names appear in `info` logs.

## Health check

`GET /healthz` (also `/health`) returns `200 OK` with
`{"status":"ok","version":"<version>"}` while the server is running. It needs no authentication, so load balancers and
orchestrators can call it.

`filebrary healthcheck` makes the same request against the configured
[`listen`](../configuration/reference.md#listen) address (using `localhost`
when it listens on `0.0.0.0` or `[::]`) and exits with status `0` when the
answer is `200`, `1` otherwise. The Docker image uses it as its
`HEALTHCHECK`, so `docker ps` shows `healthy` or `unhealthy`.

`GET /readyz` is the readiness check: it also runs a query on the database
and checks that every source root answers, each within 2 seconds. It
returns `200` with `{"status":"ok","version":"...","checks":{...}}`, or
`503` with the failing check marked, so a proxy or orchestrator can stop
sending traffic while a disk or mount is gone.

Kubernetes example:

```yaml
livenessProbe:
  httpGet: { path: /healthz, port: 8080 }
  periodSeconds: 30
readinessProbe:
  httpGet: { path: /readyz, port: 8080 }
```

## Metrics

`GET /metrics` serves Prometheus metrics. It is on by default
([`metrics.enabled`](../configuration/reference.md#metricsenabled),
`FILEBRARY_METRICS_ENABLED`). Set a token with
[`metrics.token`](../configuration/reference.md#metricstoken)
(`FILEBRARY_METRICS_TOKEN`, at least 16 characters) so only your scraper
can read it:

```toml
[metrics]
token = "a long random string"
```

```yaml
# prometheus.yml
- job_name: filebrary
  authorization: { credentials: "a long random string" }
  static_configs: [{ targets: ["files.example.com:8080"] }]
```

| Metric | Meaning |
|---|---|
| `filebrary_http_requests_total{method,route,status}` | Requests served. |
| `filebrary_http_request_duration_seconds{method,route}` | Request latency histogram. |
| `filebrary_http_requests_in_flight` | Requests being served now. |
| `filebrary_build_info{version}` | Always 1; carries the version. |
| `filebrary_uptime_seconds` | Seconds since start. |
| `filebrary_sources` | Configured sources. |
| `filebrary_db_connections{state}` | Database pool connections (`idle`, `active`). |
| `filebrary_process_resident_memory_bytes`, `filebrary_process_peak_resident_memory_bytes` | Memory now and at its peak (Linux). |
| `filebrary_process_open_fds` | Open file descriptors (Linux). |
| `filebrary_events_published_total{kind}` | Events raised (uploads, shares opened...). |
| `filebrary_notification_targets` | Configured notification targets. |
| `filebrary_notifications_sent_total`, `filebrary_notifications_failed_total` | Deliveries, by `sink`, `target` and `event`. |
| `filebrary_ftp_logins_total{result}` | FTP logins (`ok`, `failed`). |
| `filebrary_ftp_transfers_total{direction}` | FTP uploads and downloads. |

## Memory and resource use

Filebrary aims to stay under 64 MB of memory when idle with no search
index. Peak use while building the search index will be documented here
with the search feature.
