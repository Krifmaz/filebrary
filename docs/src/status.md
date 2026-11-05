# Feature status

Filebrary was built in four phases, then its web UI was rebuilt on the kit. This page says what each release
contains, so the rest of the documentation can describe features before
they ship. Pages for features that have not shipped yet carry a note at the
top linking here.

| Feature | Phase | Status |
|---|---|---|
| Local folder sources (including FUSE / mergerfs), path safety | 1 | Done |
| Users, groups, per-source and per-folder permissions | 1 | Done |
| First-run setup (no default password), roles (admin, user, read-only), home folders, remember me, Users and Settings pages | 1 | Done |
| Local passwords (argon2id), secure session cookies, CSRF protection, login rate limiting | 1 | Done |
| Browse, chunked resumable upload, download, zip of a selection | 1 | Done |
| Share links (password, expiry, download limit, upload-only drop links) | 1 | Done |
| Docker image, standalone binaries for Linux, macOS, Windows | 1 | Done |
| Installable web app (PWA) | 1 | Done |
| Indexed search with filters, kept fresh with inotify | 2 | Done |
| Thumbnails and previews (images incl. HEIC/RAW, video, audio, PDF, Office) | 2 | Done |
| Trash bin with restore and automatic purging | 2 | Done |
| Tags, stars, activity log | 2 | Done |
| Text editor | 2 | Done |
| WebDAV server | 2 | Done |
| S3-compatible, SFTP and SMB sources | 3 | Done |
| OIDC single sign-on, LDAP, TOTP two-factor, API keys, storage quotas | 3 | Done |
| Job queue, conversions (documents, video, photos), custom admin jobs | 3 | Done |
| Archives: create zip / tar.gz, browse inside zip / tar / 7z | 3 | Done |
| Full-text content search | 4 | Done |
| FTP / FTPS server | 4 | Done |
| Notifications (webhook, ntfy, email) | 4 | Done |
| Prometheus metrics | 4 | Done |
| OpenAPI spec and API docs page, readiness check, request ids in logs | 4 | Done |
| Comments, collections, casting (Chromecast and AirPlay), themes and branding | 4 | Done |
| Web UI rebuilt on [Krifmaz/kit](https://github.com/Krifmaz/kit) (Leptos, Encore theme, What's new, self-update, bug reports, diagnostics) | 5 | Done in v0.3.0 |
| This documentation site | 4 | Done |

Settings for a feature appear in the [settings reference](configuration/reference.md)
in the same release as the feature; the test suite will not let a setting
ship undocumented.
