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
| `crates/filebrary-identity` | TOTP 2FA, API keys, quotas, LDAP and OIDC logic and storage. |
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

## Text editor

Text and code files open in a CodeMirror editor in the web UI, with syntax
highlighting for about 100 languages, search (Ctrl/Cmd+F), line wrapping and
Ctrl/Cmd+S to save. Users with the `edit` permission can save; everyone else
with `download` gets a read-only view. Files up to 5 MiB that are valid UTF-8
open in the editor; others are offered as a download.

Saving never silently overwrites someone else's change. The server hands out a
strong ETag (from the file's SHA-256) when the file is opened, and a save must
carry it in `If-Match`. If the file changed in the meantime (another tab, user,
WebDAV client or a change on disk), the save is refused with `412` and the
editor offers to load the other version or overwrite it.

API: `GET /api/sources/{id}/text?path=...` returns
`{path, content, etag, size, modified}`; `PUT` on the same URL takes the raw
text and `If-Match: <etag>` (or `If-None-Match: *` to create a file). A `PUT`
without either header is refused with `428`.

## Configuration (short)

`--config file.toml` / `FILEBRARY_CONFIG`, plus environment overrides:
`FILEBRARY_LISTEN` (`0.0.0.0:8080`), `FILEBRARY_DATA_DIR`, `FILEBRARY_SOURCES`
(`files=/srv;media=/mnt/media`), `FILEBRARY_PUBLIC_URL`,
`FILEBRARY_SECURE_COOKIES`, `FILEBRARY_LOG` (filter, e.g. `debug`),
`FILEBRARY_LOG_FORMAT` (`text`|`json`), `FILEBRARY_TRASH_RETENTION_DAYS`
(`30`; `0` makes deletes permanent), `FILEBRARY_ACTIVITY_RETENTION_DAYS`
(`365`; `0` keeps events forever). `filebrary healthcheck` probes
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

## Single sign-on, 2FA, API keys and quotas

- **OpenID Connect** (`[oidc]`): sign in with Authelia, Authentik, Keycloak,
  Google, Entra and others. Accounts link by issuer and `sub`; admin rights
  and group membership can follow the provider's groups.
- **LDAP** (`[ldap]`): search-then-bind against OpenLDAP, Active Directory,
  FreeIPA or lldap, over `ldaps://` or StartTLS.
- **TOTP 2FA**: any authenticator app, with ten single-use recovery codes.
- **API keys**: `Authorization: Bearer fbk_...`, with scopes
  (`files:read`, `files:write`, `files:delete`, `shares`, `admin`) that cap
  what the key can do; never more than its owner.
- **Quotas**: per-user limits (`default_quota = "10G"`); uploads over the
  limit get `507 quota_exceeded`.

The logic lives in `crates/filebrary-identity`; details in
`docs/src/guide/authentication.md`.

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
