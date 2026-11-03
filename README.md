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

## Search

Every source is indexed into SQLite (names, type, size, date) and kept fresh
by file-system notifications (inotify/FSEvents/ReadDirectoryChangesW) plus a
periodic rescan. `GET /api/search?q=report&type=document&min_size=1M&after=2024-01-01&tag=work`
filters by name, type, extension, size, date and tag; results only include
what the caller may view.

```toml
[search]
enabled = true              # FILEBRARY_SEARCH_ENABLED
watch = true                # FILEBRARY_SEARCH_WATCH
rescan_minutes = 60         # unwatched sources; FILEBRARY_SEARCH_RESCAN_MINUTES
watched_rescan_hours = 24   # safety net for watched sources (0 = off)
exclude_sources = []
batch_size = 500
```

Indexing cost, measured with `cargo run --release --example index_bench -- N`
(Linux, ext4, release build):

| Files     | First scan | Rescan (no changes) | Peak RSS (`VmHWM`) | Database |
|-----------|-----------:|--------------------:|-------------------:|---------:|
| 100 000   | 1.2 s      | 1.5 s               | 13.3 MiB           | 25 MiB   |
| 1 000 000 | 12.6 s     | 17.9 s              | 16.5 MiB           | 251 MiB  |

Memory stays flat as the tree grows because the walker streams batches of
`batch_size` rows through a short channel; a substring name query on one
million entries takes about 55 ms. On Linux each watched folder uses one
inotify watch: raise `fs.inotify.max_user_watches` for very large trees (if
the watcher cannot start, the source falls back to periodic rescans).
