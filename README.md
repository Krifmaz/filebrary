# Filebrary

A small, fast, self-hosted file manager written in Rust, shipped as one
binary (web UI embedded) and one Docker image. Licensed under Apache-2.0.

> Status: Phase 1 in progress (v0.1). This README is expanded as features land.

## Documentation

The full documentation (install, configuration and every setting) is at
<https://krifmaz.github.io/filebrary/>, built from `docs/` with mdBook.
Preview it locally with `mdbook serve docs`.

## Layout

| Path                       | What                                                          |
|----------------------------|---------------------------------------------------------------|
| `crates/filebrary`         | The server: axum app, config, auth, API, shares, embedded UI. |
| `crates/filebrary-events`  | Event bus and notifications (webhook, ntfy, email).           |
| `crates/filebrary-ftp`     | Optional FTP/FTPS server over the storage trait.              |
| `crates/filebrary-storage` | `Storage` trait, `VPath` path validation; local, S3, SFTP and SMB backends. |
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

`FILEBRARY_SOURCES` makes local sources only; S3, SFTP and SMB sources are set
in the config file (`kind = "s3"` with a `[sources.s3]` table, and so on). See
[Storage sources](docs/src/configuration/sources.md).

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

## Share links

Users with the `share` permission can create public links to a file, a
folder, or several items at once (`POST /api/shares`). A link can have a
password (argon2id), an expiry time, a download limit, or be an
upload-only *drop link* where visitors add files to a folder without seeing
its contents. Links are served at `/s/{token}` (API: `/api/public/shares/{token}`).

A link never grants more than its creator holds: if the creator is disabled
or loses the rights on the shared path, the link stops working at once, and
folders that a rule hides from the creator are hidden from visitors too.
Shared folders are confined, so symlinks inside them cannot reach other
files. Wrong share passwords are rate limited with the login limits, and
drop-link uploads are capped by `max_drop_upload_mib` and never overwrite.

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
  (HMAC-signed), ntfy and email: see `docs/src/guide/notifications.md`.
- An FTP/FTPS server using the same users and permissions is built in and
  off by default (`[ftp] enabled = true`): see `docs/src/guide/ftp.md`.

Idle memory of a release build with one local source is about 12 MB RSS
(`filebrary_process_resident_memory_bytes` and
`filebrary_process_peak_resident_memory_bytes` report it live).

## Comments, collections, casting, themes

* **Comments**: select a file and press *Comments*. Anyone who can see a file
  reads its thread; posting needs `download`; authors edit their own, and
  holders of `edit` (and admins) can delete any comment on the file.
  API: `/api/comments`.
* **Collections**: select files or folders from any source and press *Add to
  collection*; the *Collections* page lists them and creates one share link
  for a whole collection (password, expiry and download limit as usual). The
  link only serves items the owner may still share. API: `/api/collections`.
* **Casting**: video and audio open in a player with a *Cast* button when a
  Chromecast (Chrome) or AirPlay device (Safari) is nearby. It uses the
  browser's Remote Playback API, so no third-party script is loaded; the
  device streams from a signed URL that expires after 6 hours and stops
  working as soon as the user loses access. Set `public_url` so devices
  reach the right address.
* **Themes and branding**: *Appearance* lets each user pick light, dark,
  system or a custom palette; administrators set the product name, logo,
  default theme and the colours of each theme, which also brand the login
  page, share pages and the installed PWA.

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
