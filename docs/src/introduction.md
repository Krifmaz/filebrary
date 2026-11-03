# Introduction

Filebrary is a self-hosted file manager written in Rust. It ships as a
single static binary with the web interface embedded, and as a small Docker
image. It needs no external services: users, shares, tags, the activity log
and the search index all live in one SQLite database.

Highlights (see the roadmap in the README for what each release includes):

- Several storage sources at once, each with its own permissions.
- Multiple users and groups with per-source and per-folder permissions.
- Chunked, resumable uploads; zip downloads of a selection on the fly.
- Share links with password, expiry and download limits.
- An installable web app (PWA) for desktop and mobile.

Start with [Docker](install/docker.md) if you can run containers, or grab a
[standalone binary](install/binary.md) for Linux, macOS or Windows.

Filebrary is licensed under the Apache License, version 2.0.
