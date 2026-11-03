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

Content search looks inside documents: plain text and source code, PDF,
Word (`.docx`), PowerPoint (`.pptx`), Excel and other spreadsheets (`.xlsx`,
`.xls`, `.ods`) and OpenDocument text and presentations (`.odt`, `.odp`).
The text is extracted inside Filebrary, with no external programs, and kept
in a [tantivy](https://github.com/quickwit-oss/tantivy) index in the data
directory.

How to search:

* all words must appear: `quarterly budget`,
* `"quoted phrases"` must appear in that order,
* `-word` excludes documents containing the word,
* upper/lower case and accents do not matter (`creme` finds `crème`).

Content search combines with every other filter: search for `invoice`
inside PDFs modified this year in one folder, for example. Results are
ordered by relevance unless you choose another sort, and each result shows
a short excerpt with the matching words highlighted. As with name search,
you only find files you are allowed to view.

New and changed files become searchable within a few seconds of the
file-name index noticing them. When content search is first switched on
for a source, existing files are indexed in the background.

Content search is on for every source by default. Turn it off for a source
with [`content_search = false`](../configuration/reference.md#sourcescontent_search),
or for the whole server with
[`[fulltext] enabled = false`](../configuration/reference.md#fulltextenabled).
Files larger than [`max_file_mib`](../configuration/reference.md#fulltextmax_file_mib)
are skipped. Memory use while indexing is measured in
[Logs, health and metrics](../operations/monitoring.md#content-search).

### API

`GET /api/search?content=<query>` with any of the other search parameters.
Each hit then carries a `snippet` field: an HTML fragment in which the text
is escaped and matches are wrapped in `<b>…</b>`, so it is safe to insert
into a page. At most the 1000 best content matches are considered per
query; narrow the query to reach beyond them.
