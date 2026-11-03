# Search

> **Planned for Phases 2 and 4.** See [Feature status](../status.md).

## Indexed search

Filebrary keeps an index of every file name across all sources in its
SQLite database, so search is instant even on large libraries. Search
results respect permissions: you only find what you can view.

Filters:

* **name** (words or part of a name),
* **type** (image, video, audio, document, archive, folder, …),
* **size** (larger or smaller than),
* **date** modified (before, after, between),
* **tag**.

The index is kept up to date with file-system notifications (inotify on
Linux) for local sources, and by a periodic rescan for network sources (S3,
SFTP, SMB) and file systems that do not send notifications (some FUSE and
network mounts). Changes made outside Filebrary appear within seconds on
local disks and after the next rescan elsewhere.

## Content search

Full-text search inside text files, PDFs and Office documents, using a
tantivy index stored in the data directory. It can be turned on or off per
source, because indexing contents takes disk space and CPU time.

The settings for both, and measured memory use while indexing, will be in
the [settings reference](../configuration/reference.md) and
[Logs, health and metrics](../operations/monitoring.md) when search ships.
