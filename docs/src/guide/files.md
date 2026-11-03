# Working with files

## Browsing

Each [source](../configuration/sources.md) you can view appears in the
sidebar. Open folders by clicking them; the path bar shows where you are
and each part of it is a link. Sort by name, size or date.

## Uploading

Drag files or whole folders onto the window, or use **Upload**. Uploads are
split into chunks (at most [`max_chunk_mib`](../configuration/reference.md#max_chunk_mib)
each) and are **resumable**: if the connection drops, the upload carries on
from the last chunk instead of starting again. Unfinished uploads are kept
for [`upload_expiry_hours`](../configuration/reference.md#upload_expiry_hours)
and then removed. Folder structure is kept when you upload a folder.

You need the **upload** permission on the target folder.

## Downloading

Click a file to open or download it. Downloads support HTTP range requests,
so interrupted downloads can resume and video and audio can seek.

Select several files and folders and choose **Download** to get a single
zip file. It is built on the fly while it downloads, so it starts at once
and uses no disk space on the server.

## Rename, move, copy, delete

Use the context menu or the toolbar on a selection. Moving and copying work
within a source and between sources. You need **edit** to rename or move,
**upload** on the target to copy, and **delete** to delete.

## Trash, stars, tags and activity

Deleted items go to the trash and can be restored; it is emptied
automatically after [`trash_retention_days`](../configuration/reference.md#trash_retention_days).
**Star** items to bookmark them, **tag** them with any number of shared
tags, and see who did what in the **activity log**. All of this, with its
API, is on [Trash, tags, stars and activity](../organize.md).

## Comments

Select a file and choose **Comments** to read and add to its thread.
Anyone who can see the file can read the thread; posting needs
**download**. Authors can edit their own comments, and users with **edit**
on the file (and administrators) can delete any comment on it. Threads stay
attached when files are renamed, moved or trashed. API: `/api/comments`.

## Text editor

Text and code files open in a built-in editor (CodeMirror) when you click
them. It has syntax highlighting for about 100 languages, search and replace
(Ctrl/Cmd+F), line wrapping and Ctrl/Cmd+S to save.

* Opening a file needs **download**; saving changes needs **edit**. Without
  **edit** the file opens read-only.
* Files up to 5 MiB that are UTF-8 text open in the editor. Larger or binary
  files are offered as a download instead.
* Saving never silently overwrites someone else's change. If the file was
  changed since you opened it (in another tab, by another user, through
  WebDAV or directly on disk), the editor says so and lets you load their
  version, overwrite it with yours, or keep editing.
* The editor warns you before you close it, or leave the page, with unsaved
  changes.
* Every save is recorded in the [activity log](../organize.md) as `edit`.

API: `GET /api/sources/{id}/text?path=...` returns
`{path, content, etag, size, modified}`. `PUT` on the same URL saves the raw
body and needs `If-Match: <etag>` from the load (or `If-None-Match: *` to
create a new file). A stale ETag gets `412 precondition_failed` with the
current `etag`; a save without either header gets `428`.

## Archives

> **Planned for Phase 3.** See [Feature status](../status.md).

Create a zip or tar.gz from a selection, stored next to it. Open zip, tar
and 7z files to browse and download single entries without extracting the
whole archive.
