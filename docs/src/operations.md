# Upgrading and backups

## Backups

Everything Filebrary owns is in the data directory (`/data` in Docker):
the SQLite database, the configuration file and caches. Your files stay in
their sources and are never moved into the data directory.

To back up while running, use SQLite's online backup so the copy is
consistent:

```sh
sqlite3 /data/filebrary.db ".backup '/backup/filebrary.db'"
```

Or stop Filebrary and copy the whole data directory.

## Upgrading

Docker:

```sh
docker pull ghcr.io/krifmaz/filebrary:latest
docker stop filebrary && docker rm filebrary
# then run the same `docker run` command as before
```

Binary: replace the binary and restart the service. Database migrations run
automatically on start. Take a backup first.

## Monitoring

| Endpoint            | What                                                                    |
|---------------------|-------------------------------------------------------------------------|
| `/healthz`          | Liveness (also `/health`, `/api/health`); used by `filebrary healthcheck`. |
| `/readyz`           | Readiness: database and every source root answer within 2 s, else 503.  |
| `/metrics`          | Prometheus metrics (`filebrary_*`). Protect with `metrics.token`.       |
| `/api/openapi.json` | OpenAPI 3.1 description of the REST API.                                |
| `/api/docs`         | Built-in API reference page (no CDN, works offline).                    |

Logs are structured: set `FILEBRARY_LOG_FORMAT=json` for one JSON object per
line. Every request gets an `X-Request-Id` (kept from the client when sane,
otherwise generated) and one `request finished` line with `request_id`,
`method`, `route` (the route pattern, never the raw path), `status` and
`latency_ms`. `filebrary_process_peak_resident_memory_bytes` reports peak
memory, for example while indexing.

```toml
[metrics]
enabled = true                # default
token = "a long random string" # scrapers send Authorization: Bearer <token>
```

Metrics include `filebrary_http_requests_total{method,route,status}`,
`filebrary_http_request_duration_seconds`, `filebrary_http_requests_in_flight`,
`filebrary_build_info`, `filebrary_uptime_seconds`,
`filebrary_process_resident_memory_bytes`, `filebrary_process_open_fds`,
`filebrary_db_connections{state}`, `filebrary_events_published_total{kind}`,
`filebrary_notifications_sent_total` and `filebrary_notifications_failed_total`
(by `sink`, `target`, `event`), `filebrary_ftp_logins_total{result}` and
`filebrary_ftp_transfers_total{direction}`.

| TOML key | Environment | Default | Description |
| --- | --- | --- | --- |
| `metrics.enabled` | `FILEBRARY_METRICS_ENABLED` | `true` | Serve `/metrics` (404 when off). |
| `metrics.token` | `FILEBRARY_METRICS_TOKEN` | none | Bearer token scrapers must send; at least 16 characters. |

Prometheus scrape job:

```yaml
- job_name: filebrary
  bearer_token: a long random string
  static_configs: [{ targets: ["files.example.com:8080"] }]
```

Kubernetes probes: `livenessProbe` on `/healthz`, `readinessProbe` on
`/readyz`.

## API reference

The REST API is described by an OpenAPI 3.1 document at `/api/openapi.json`;
open `/api/docs` on your server for a readable reference. Import the JSON into
any OpenAPI tool to generate a client.
