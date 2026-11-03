# Filebrary

A small, fast, self-hosted file manager written in Rust, shipped as one
binary (web UI embedded) and one Docker image. Licensed under Apache-2.0.

> Status: Phase 1 in progress (v0.1). This README is expanded as features land.

## Layout

| Path                       | What                                                          |
|----------------------------|---------------------------------------------------------------|
| `crates/filebrary`         | The server: axum app, config, auth, API, shares, embedded UI. |
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
`FILEBRARY_LOG_FORMAT` (`text`|`json`). `filebrary healthcheck` probes
`/healthz` for container health checks.

## Share links

Users with the `share` permission can create public links to a file, a
folder, or several items at once (`POST /api/shares`). A link can have a
password (argon2id), an expiry time, a download limit, or be an
upload-only *drop link* where visitors add files to a folder without seeing
its contents. Links are served at `/s/{token}` (API: `/api/public/shares/{token}`).

A link never grants more than its creator holds: if the creator is disabled
or loses the rights on the shared path, the link stops working at once.
Shared folders are confined, so symlinks inside them cannot reach other
files. Wrong share passwords are rate limited with the login limits, and
drop-link uploads are capped by `max_drop_upload_mib` and never overwrite.
