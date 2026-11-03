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

## Users and permissions

On first start, with no users in the database, Filebrary creates an
administrator named `admin` (`FILEBRARY_ADMIN_USERNAME` / `admin_username`).
Its password is `FILEBRARY_ADMIN_PASSWORD` if set; otherwise a random one is
generated and printed once in the log. Log in and change it.

- Passwords are hashed with argon2id. Sessions live in an `HttpOnly`,
  `SameSite=Lax` cookie (`Secure` behind HTTPS); state-changing requests
  must echo the session's CSRF token in `X-CSRF-Token`.
- Failed logins are limited per username and per client address
  (`login_max_attempts` within `login_window_secs`; answers 429 with
  `Retry-After`). Behind a reverse proxy set `FILEBRARY_TRUST_PROXY_HEADERS`.
- Administrators can do everything. Other users get permissions from
  rules: each rule gives a user or a group some of `view`, `download`,
  `upload`, `edit`, `delete`, `share` on a folder of a source and everything
  below it. The most specific folder wins; at the same folder a user's own
  rule beats their groups' (which are combined). An empty rule hides a
  folder. Rules are checked on the symlink-resolved path, so a link cannot
  lead into a hidden folder.
- A user can also be confined to a folder (scope) per source; the paths
  they see start there.

The admin API is under `/api/users`, `/api/groups` and `/api/permissions`
(see `crates/filebrary/src/auth/routes.rs`).
