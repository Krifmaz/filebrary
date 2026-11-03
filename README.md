# Filebrary

A small, fast, self-hosted file manager written in Rust, shipped as one
binary (web UI embedded) and one Docker image. Licensed under Apache-2.0.

> Status: Phase 1 in progress (v0.1). This README is expanded as features land.

## Layout

| Path                       | What                                                          |
|----------------------------|---------------------------------------------------------------|
| `crates/filebrary`         | The server: axum app, config, auth, API, shares, embedded UI. |
| `crates/filebrary-storage` | `Storage` trait, `VPath` path validation, local backend.      |
| `crates/filebrary-webdav`  | WebDAV class 1 and 2 server over the `Storage` trait.         |
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
`FILEBRARY_LOG_FORMAT` (`text`|`json`), `FILEBRARY_WEBDAV` (`true`|`false`). `filebrary healthcheck` probes
`/healthz` for container health checks.

## WebDAV

Every source is also served over WebDAV (class 2, with locking) at `/dav/`,
with the same accounts, permissions and folder scopes as the web UI. Log in
with your Filebrary user name and password (HTTP Basic, so use HTTPS).

| Client          | How to connect                                                        |
|-----------------|-----------------------------------------------------------------------|
| Windows Explorer| *Map network drive* → `https://files.example.com/dav/files/`          |
| macOS Finder    | *Go → Connect to Server* → `https://files.example.com/dav/`           |
| rclone          | `rclone config`, type `webdav`, vendor `other`, URL `https://files.example.com/dav/` |
| GNOME / KDE     | `davs://files.example.com/dav/`                                       |

`/dav/` lists your sources; `/dav/<source>/...` is a source. Windows only
sends Basic credentials over HTTPS unless `BasicAuthLevel` is raised in the
registry. Turn WebDAV off with `webdav = false` or `FILEBRARY_WEBDAV=false`.
See [crates/filebrary-webdav](crates/filebrary-webdav/README.md) for what is
implemented and how methods map to permissions.
