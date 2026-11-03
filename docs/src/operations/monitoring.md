# Logs, health and metrics

## Logs

Filebrary logs to standard output. Docker keeps them for `docker logs`;
systemd sends them to the journal (`journalctl -u filebrary`).

* **Format:** human-readable text by default. Set
  [`log_format`](../configuration/reference.md#log_format) (or
  `FILEBRARY_LOG_FORMAT`) to `json` for one JSON object per line, with the
  level, target, message and fields such as the request method, path,
  status and duration.
* **Level:** set `FILEBRARY_LOG` to a filter in
  [`EnvFilter` syntax](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html).
  The default is `info,tower_http=info,sqlx=warn`. Useful values:

  | Value                         | Shows                                         |
  |-------------------------------|-----------------------------------------------|
  | `warn`                        | Only problems.                                |
  | `info`                        | Start-up, every request, background jobs.     |
  | `debug`                       | Everything above, plus internal detail.       |
  | `info,filebrary=debug`        | Debug detail from Filebrary only.             |

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

Kubernetes example:

```yaml
livenessProbe:
  httpGet: { path: /healthz, port: 8080 }
  periodSeconds: 30
readinessProbe:
  httpGet: { path: /healthz, port: 8080 }
```

## Metrics

A Prometheus `/metrics` endpoint is planned for a later release (see
[Feature status](../status.md)). Its settings and metric names will be
listed here when it lands.

## Memory and resource use

Filebrary aims to stay under 64 MB of memory when idle with no search
index. Peak use while building the search index will be documented here
with the search feature.

### Content search

Measured on Linux (release build) indexing 12 632 files (157 MiB of text
and source code): 6.9 s, queries about 4 ms.

| Allocator                                   | Peak anonymous memory while indexing |
|---------------------------------------------|-------------------------------------:|
| musl (static binary, Alpine image), or glibc with `MALLOC_ARENA_MAX=2` | about 77 MB |
| glibc defaults                              | about 154 MB                          |

The peak is roughly the index writer buffer
([`fulltext.writer_memory_mib`](../configuration/reference.md#fulltextwriter_memory_mib),
24 MiB) plus the largest file being read
([`fulltext.max_file_mib`](../configuration/reference.md#fulltextmax_file_mib))
and its extracted text. The buffer is freed after every commit, so an idle
server keeps only the memory-mapped index, which the operating system can
evict. Lower those two settings on small machines. To measure your own
data:

```sh
cargo run --release -p filebrary-fulltext --example index_dir -- /path/to/files "some words"
```
