# Feature status

Filebrary is built in four phases. This page says what each release
contains, so the rest of the documentation can describe features before
they ship. Pages for features that have not shipped yet carry a note at the
top linking here.

| Feature | Phase | Status |
|---|---|---|
| Local folder sources (including FUSE / mergerfs), path safety | 1 | In progress for v0.1 |
| Users, groups, per-source and per-folder permissions | 1 | Done, in the next release |
| Local passwords (argon2id), secure session cookies, CSRF protection, login rate limiting | 1 | Done, in the next release |
| Browse, chunked resumable upload, download, zip of a selection | 1 | In progress for v0.1 |
| Share links (password, expiry, download limit, upload-only drop links) | 1 | In progress for v0.1 |
| Docker image, standalone binaries for Linux, macOS, Windows | 1 | In progress for v0.1 |
| Installable web app (PWA) | 1 | In progress for v0.1 |
| Indexed search with filters, kept fresh with inotify | 2 | Done, in the next release |
| Thumbnails and previews (images incl. HEIC/RAW, video, audio, PDF, Office) | 2 | Planned |
| Trash bin with restore and automatic purging | 2 | Done, in the next release |
| Tags, stars, activity log | 2 | Done, in the next release |
| Text editor | 2 | Planned |
| WebDAV server | 2 | Planned |
| S3-compatible, SFTP and SMB sources | 3 | Done, in the next release |
| OIDC single sign-on, LDAP, TOTP two-factor, API keys, storage quotas | 3 | Planned |
| Job queue, conversions (documents, video, photos), custom admin jobs | 3 | Planned |
| Archives: create zip / tar.gz, browse inside zip / tar / 7z | 3 | Planned |
| Full-text content search | 4 | Done, in the next release |
| FTP / FTPS server | 4 | Planned |
| Notifications (webhook, ntfy, email) | 4 | Planned |
| Prometheus metrics | 4 | Planned |
| Comments, collections, casting (Chromecast and AirPlay), themes and branding | 4 | Done, in the next release |
| This documentation site | 4 | In progress |

Settings for a feature appear in the [settings reference](configuration/reference.md)
in the same release as the feature; the test suite will not let a setting
ship undocumented.
