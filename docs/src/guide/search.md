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
