# Filebrary

A small, fast, self-hosted file manager written in Rust, shipped as one
binary (web UI embedded) and one Docker image. Licensed under Apache-2.0.

> Status: Phase 1 in progress (v0.1). This README is expanded as features land.

## Layout

| Path                       | What                                                          |
|----------------------------|---------------------------------------------------------------|
| `crates/filebrary`         | The server: axum app, config, auth, API, shares, embedded UI. |
| `crates/filebrary-storage` | `Storage` trait, `VPath` path validation, local backend.      |
| `crates/filebrary-preview` | Thumbnails, previews, media info, transcoding, preview cache. |
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
(`365`; `0` keeps events forever), `FILEBRARY_PREVIEWS` (`true`|`false`),
`FILEBRARY_PREVIEW_CACHE_MIB` (default `2048`). `filebrary healthcheck` probes
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

## Previews

| Kind | Thumbnail | Needs |
|------|-----------|-------|
| JPEG, PNG, GIF, WebP, TIFF, BMP, ICO | resized in Rust | nothing |
| Camera RAW (CR2, CR3, NEF, ARW, DNG, RAF, ORF, RW2, ...) | embedded JPEG, in Rust | nothing |
| HEIC, HEIF, AVIF | decoded and resized | `heif-dec` or `ffmpeg` |
| Video | poster frame | `ffmpeg`, `ffprobe` |
| Audio | embedded cover art | `ffmpeg`, `ffprobe` |
| PDF | first page | `pdftoppm` (poppler-utils) |
| Word, Excel, PowerPoint, OpenDocument | first page; full PDF via `variant=pdf` | `soffice`, `pdftoppm` |

The slim image links no C libraries for previews, so it covers images and
RAW; the `-full` image carries the tools for the rest. Tools are found on
`PATH` at startup and logged. Previews are cached in
`<data_dir>/cache/previews` (LRU, `[previews] cache_mib`). Video and audio
the browser cannot play (HEVC, AC-3, WMA, ...) stream through
`GET /api/sources/{id}/stream?path=&start=` as fragmented MP4; everything else
plays from the download route with range requests. External tools always run
with an argument list (no shell), a time limit, and a fresh LibreOffice
profile per conversion.
