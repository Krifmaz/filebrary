# Filebrary

A small, fast, self-hosted file manager written in Rust, shipped as one
binary (web UI embedded) and one Docker image. Licensed under Apache-2.0.

> Status: Phase 1 in progress (v0.1). This README is expanded as features land.

## Layout

| Path                       | What                                                          |
|----------------------------|---------------------------------------------------------------|
| `crates/filebrary`         | The server: axum app, config, auth, API, shares, embedded UI. |
| `crates/filebrary-storage` | `Storage` trait, `VPath` path validation, local backend.      |
| `crates/filebrary-archive` | Browse zip/tar/tar.gz/7z; create zip and tar.gz (pure Rust). |
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
(`365`; `0` keeps events forever), `FILEBRARY_JOB_WORKERS`,
`FILEBRARY_JOB_RETAIN_DAYS`, `FILEBRARY_FFMPEG`, `FILEBRARY_FFPROBE`,
`FILEBRARY_SOFFICE`. `filebrary healthcheck` probes
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

## Jobs, conversions and archives

Long-running work runs as background jobs (`/api/jobs`) with progress, cancel
and retry: archive creation (zip, tar.gz), documents to PDF (LibreOffice),
video transcode/remux and audio extraction (ffmpeg), and image resize/format
changes (built in). Administrators can define custom jobs in the config file
(`[[jobs.custom]]`); they run a program directly with an argument list, never
through a shell, on a private copy of the file. Archives (zip, tar, tar.gz,
7z) can be browsed and single files downloaded without extracting
(`/api/sources/{id}/archive`). Conversions need the `full` Docker image (or
ffmpeg/LibreOffice installed); everything else works in the small image.
