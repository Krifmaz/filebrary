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

> **Planned for Phase 2.** See [Feature status](../status.md).

Text and code files open in a built-in editor (CodeMirror) with syntax
highlighting. Saving needs the **edit** permission.

## Archives

Create a zip or tar.gz from a selection with **Create archive**; it runs as
a [job](jobs.md#archives) and the archive is saved in the folder you choose.

Open a zip (also `.jar`, `.cbz`), tar, tar.gz or 7z file to browse its
folders and download single files without extracting the whole archive.
Only the archive's index and the file you open are read, even on remote
sources. Browsing needs the **download** permission. Entries with unsafe
names (such as `../`) are hidden, and limits on entry count and size
([`jobs.max_archive_entries`](../configuration/reference.md#jobsmax_archive_entries),
[`jobs.max_archive_entry_mib`](../configuration/reference.md#jobsmax_archive_entry_mib))
protect the server from hostile archives. Files from inside an archive are
always downloaded, never displayed in the page.

API: `GET /api/sources/{id}/archive?path=/a.zip&dir=docs` lists a folder
inside the archive (leave out `dir` for every entry);
`GET /api/sources/{id}/archive/entry?path=/a.zip&entry=docs/readme.txt`
downloads one file.
