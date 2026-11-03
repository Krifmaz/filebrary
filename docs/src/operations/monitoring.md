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
index.

Building the search index streams rows to the database in batches, so its
memory use barely grows with the size of the tree. Measured with
`cargo run --release --example index_bench -- N` on Linux (ext4), the whole
process including the server state:

| Files     | First scan | Rescan, nothing changed | Peak memory (RSS) | Database size |
|-----------|-----------:|------------------------:|------------------:|--------------:|
| 100 000   | 1.2 s      | 1.5 s                   | 13.3 MiB          | 25 MiB        |
| 1 000 000 | 12.6 s     | 17.9 s                  | 16.5 MiB          | 251 MiB       |

A name search over a million entries takes about 55 ms. Budget roughly
250 bytes of database per indexed file or folder. On Linux, watching a
source costs one inotify watch per folder (about 1 KiB of kernel memory
each).

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
