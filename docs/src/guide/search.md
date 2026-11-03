# Search

Search finds files and folders by name across every source you can view,
and narrows the results by type, size, date and tag.

## How the index stays fresh

Filebrary keeps an index of every file and folder name in its SQLite
database, so search answers quickly even on very large libraries.

* At start-up every source is scanned. Scanning a million files takes
  around 15 seconds on a local disk; search works meanwhile, with results
  filling in as the scan progresses.
* Local sources are then **watched** (inotify on Linux, FSEvents on macOS,
  ReadDirectoryChangesW on Windows): changes made outside Filebrary, by
  another program or over SMB, show up within about a second.
* Changes made through Filebrary itself (uploads, new folders, moves,
  copies, deletes, restores from the trash) are indexed at once.
* Every source is also **rescanned** periodically: every
  `search.rescan_minutes` (60) for sources that are not watched (network
  sources and some FUSE mounts), and every `search.watched_rescan_hours`
  (24) as a safety net for watched ones. An administrator can start a
  rescan at any time.

The trash folder is never indexed. Symbolic links that point outside a
source are not indexed, and linked folders are not followed (their target,
if inside the source, is indexed at its real place).

## What you can find

Search only returns what you are allowed to view: results stay inside
your folder, folders where your permissions are taken away are left out,
and a symbolic link is only shown if you could also open what it points to.

| Filter | Example | Meaning |
|--------|---------|---------|
| `q` | `q=tax 2024` | every word appears in the name (any case, any position) |
| `type` | `type=image,video` | `dir`, `image`, `video`, `audio`, `document`, `text`, `code`, `archive`, `other`, or `file` for anything but folders |
| `ext` | `ext=jpg,heic` | file extensions |
| `min_size`, `max_size` | `min_size=100MB` | bytes or `k`, `M`, `G`, `T` (powers of 1024) |
| `after`, `before` | `after=2024-01-01` | modified on or after / before; a date, RFC 3339 time or Unix seconds |
| `tag` | `tag=work,urgent` | carries every listed tag |
| `source`, `path` | `source=files&path=/photos` | only these sources, or only under this folder |
| `sort`, `order` | `sort=modified` | `name`, `path`, `size` or `modified`; `asc` or `desc` |
| `limit`, `offset` | `limit=50&offset=50` | paging (at most 1000 per page) |

## API

* `GET /api/search?…` with the filters above returns
  `{"hits": [...], "has_more": bool, "indexing": bool}`. Each hit has
  `source`, `path` (as you see it), `name`, `type`, `is_dir`, `size`,
  `modified` and `mime`.
* `GET /api/search/status` shows, for each source you can view, how many
  entries are indexed, whether a scan is running, whether it is watched,
  and when the last scan finished.
* `POST /api/search/reindex` with `{"source": "name"}` (or no body for all
  sources) starts a full rescan. Administrators only.

Settings: [`[search]`](../configuration/reference.md#search). Memory use while
indexing: [Logs, health and metrics](../operations/monitoring.md#memory-and-resource-use).

## Content search

Full-text search inside text files, PDFs and Office documents, using a
tantivy index stored in the data directory. It can be turned on or off per
source, because indexing contents takes disk space and CPU time.

> **Planned for Phase 4.** See [Feature status](../status.md).
