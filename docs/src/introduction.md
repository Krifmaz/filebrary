# Introduction

Filebrary is a self-hosted file manager written in Rust. It ships as a
single static binary with the web interface embedded, and as a small Docker
image. It needs no external services: users, shares, tags, the activity log
and the search index all live in one SQLite database.

- **Several storage sources** at once (local folders and FUSE pools; S3,
  SFTP and SMB later), each with its own permissions.
- **Users and groups** with per-source and per-folder permissions: view,
  download, upload, edit, delete, share.
- **Chunked, resumable uploads**, drag-and-drop of whole folders, and zip
  downloads of a selection built on the fly.
- **Share links** with password, expiry, download limit, and upload-only
  drop links.
- An **installable web app** (PWA) for phones and computers.
- Later: indexed and full-text search, previews and conversions, trash,
  tags, WebDAV and FTP, single sign-on, notifications and metrics. See
  [Feature status](status.md) for what each release contains.

## Where to start

* [Docker](install/docker.md) if you can run containers, or a
  [standalone binary](install/binary.md) for Linux, macOS or Windows.
* Then [First steps](install/first-steps.md).
* All settings: [Settings reference](configuration/reference.md).

Filebrary is licensed under the [Apache License, version 2.0](licence.md).
