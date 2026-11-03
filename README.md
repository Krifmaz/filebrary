# Filebrary

A small, fast, self-hosted file manager written in Rust, shipped as one
binary (web UI embedded) and one Docker image. Licensed under Apache-2.0.

> Status: Phase 1 in progress (v0.1). This README is expanded as features land.

## Layout

| Path                       | What                                                          |
|----------------------------|---------------------------------------------------------------|
| `crates/filebrary`         | The server: axum app, config, auth, API, shares, embedded UI. |
| `crates/filebrary-events`  | Event bus and notifications (webhook, ntfy, email).           |
| `crates/filebrary-ftp`     | Optional FTP/FTPS server over the storage trait.              |
| `crates/filebrary-storage` | `Storage` trait, `VPath` path validation, local backend.      |
| `migrations/`              | SQLite migrations for the whole workspace (one history).      |
| `web/`                     | React + TypeScript + Vite PWA, built into `web/dist`.         |
| `docs/`                    | mdBook documentation.                                         |

Migration numbers: `0001-0009` core, `0010-0019` auth, `0020-0029` shares,
`0030-0039` search, `0040+` later features.

## Build

```sh
npm --prefix web ci && npm --prefix web run build   # optional: embeds the UI
cargo build --release
./target/release/filebrary            # serves ./data/files on 127.0.0.1:8080
```

## Configuration (short)

`--config file.toml` / `FILEBRARY_CONFIG`, plus environment overrides:
`FILEBRARY_LISTEN` (`0.0.0.0:8080`), `FILEBRARY_DATA_DIR`, `FILEBRARY_SOURCES`
(`files=/srv;media=/mnt/media`), `FILEBRARY_PUBLIC_URL`,
`FILEBRARY_SECURE_COOKIES`, `FILEBRARY_LOG` (filter, e.g. `debug`),
`FILEBRARY_LOG_FORMAT` (`text`|`json`), `FILEBRARY_TRASH_RETENTION_DAYS`
(`30`; `0` makes deletes permanent), `FILEBRARY_ACTIVITY_RETENTION_DAYS`
(`365`; `0` keeps events forever). `filebrary healthcheck` probes
`/healthz` for container health checks.

## Trash, tags, stars and activity

Deleting moves items to a hidden per-source folder (`/.filebrary-trash`), from
which they can be restored (to the original place or elsewhere, with optional
auto-rename) until they are purged after `trash_retention_days`. Files can carry
any number of shared tags; stars are personal bookmarks. Tags and stars follow
files through moves, copies and the trash. Every change is written to an
activity log (who, what, where, when, from which address) that admins see in
full and other users see for the folders they can view. Endpoints:
`/api/trash`, `/api/tags`, `/api/stars`, `/api/annotations`, `/api/activity`;
see [docs/src/organize.md](docs/src/organize.md).

## Monitoring, notifications and FTP

- `/healthz` (liveness), `/readyz` (database and sources), `/metrics`
  (Prometheus, optional `metrics.token`), `/api/openapi.json` and the
  built-in API reference at `/api/docs`.
- `FILEBRARY_LOG_FORMAT=json` gives one JSON object per line; every request
  carries an `X-Request-Id` and is logged by route pattern, status and latency.
- Upload, share-opened, quota-near-full and job-failed events go to webhooks
  (HMAC-signed), ntfy and email: see `docs/src/notifications.md`.
- An FTP/FTPS server using the same users and permissions is built in and
  off by default (`[ftp] enabled = true`): see `docs/src/ftp.md`.

Idle memory of a release build with one local source is about 12 MB RSS
(`filebrary_process_resident_memory_bytes` and
`filebrary_process_peak_resident_memory_bytes` report it live).

