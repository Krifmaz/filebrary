# Trash, tags, stars and the activity log

## Trash bin

Deleting a file or folder moves it into a hidden folder at the top of its
source, `/.filebrary-trash/<id>`. This is a rename on the same storage, so it
is instant even for large folders. The folder is reserved: it never appears in
listings, search or WebDAV, and paths inside it are refused.

| Setting | Env variable | Default | Meaning |
|---|---|---|---|
| `trash_retention_days` | `FILEBRARY_TRASH_RETENTION_DAYS` | `30` | Days before an item is purged automatically. `0` turns the trash off: deletes are permanent. |

A maintenance task runs every hour (first run 30 s after start-up) and purges
expired items. Items of a source that was removed from the configuration are
kept until the source returns.

Restoring puts the item back where it was, recreating missing parent folders.
If something already exists there, choose `on_conflict`: `fail` (default,
`409`), `rename` (`report (restored).pdf`, `report (restored 2).pdf`, ...) or
`overwrite` (needs the edit permission). `to` restores somewhere else in the
same source (needs upload there).

Who may do what: listing needs *view* on the original location; trashing,
restoring and purging need *delete*. Users only see items whose original path
is inside their scope.

## Tags and stars

Tags are shared names (case-insensitive, 1-64 characters, optional `#rrggbb`
colour). Anyone who can view a file sees its tags; tagging needs *edit* on the
file. Anyone with *edit* somewhere may create tag names; only administrators
rename, recolour or delete them. Stars are personal bookmarks and need only
*view*.

Tags and stars are stored per path and follow files through renames, moves,
copies (tags only) and the trash; permanent deletes remove them.

## Activity log

Uploads, downloads, moves, copies, trash and restore, permanent deletes, tag
changes, share links and logins are recorded with the user, the time, the path
(and target), optional details and the client address. Administrators see every
event and the addresses; other users see their own actions and events inside
the folders they can view, with paths relative to their scope.

| Setting | Env variable | Default | Meaning |
|---|---|---|---|
| `activity_retention_days` | `FILEBRARY_ACTIVITY_RETENTION_DAYS` | `365` | Days events are kept. `0` keeps them forever. |

## API

Paths are as the caller sees them. Unsafe methods need the `X-CSRF-Token`
header. Errors are `{"error": "...", "message": "..."}`.

| Method and path | Body / query | Returns |
|---|---|---|
| `GET /api/trash` | `?source=` | `[{id, source, path, name, kind, size, deleted_by, deleted_at, purge_at}]` |
| `POST /api/trash` | `{source, paths: [...]}` | `{trashed: [...], deleted: [...]}` |
| `POST /api/trash/{id}/restore` | `{to?, on_conflict?}` | `{source, path}` |
| `DELETE /api/trash/{id}` | | `204` |
| `DELETE /api/trash` | `?source=` | `{purged: n}` |
| `GET /api/tags` | | `[{id, name, color}]` |
| `POST /api/tags` | `{name, color?}` | `201 {id, name, color}` |
| `PATCH /api/tags/{id}` | `{name?, color?}` (`""` clears the colour) | tag |
| `DELETE /api/tags/{id}` | | `204` |
| `GET /api/tags/{id}/files` | `?source=` | `[{source, path}]` |
| `POST /api/tags/{id}/files` | `{source, paths}` | `204` |
| `DELETE /api/tags/{id}/files` | `{source, paths}` | `204` |
| `GET /api/tags/file` | `?source=&path=` | `[{id, name, color}]` |
| `PUT /api/tags/file` | `{source, path, tags: [names]}` (unknown names are created) | the file's tags |
| `GET /api/stars` | | `[{source, path, name, starred_at}]` |
| `PUT /api/stars` | `{source, path}` | `204` |
| `DELETE /api/stars` | `?source=&path=` | `204` |
| `GET /api/annotations` | `?source=&path=<folder>` | `{"<name>": {starred, tags}}` for entries that have any |
| `GET /api/activity` | `?source=&path=&recursive=&user=&action=&since=&until=&before=&limit=` | `{items: [...], next_before}` |

Times are Unix seconds. Page the activity log by passing `next_before` as
`before` until it is `null`.

## For contributors

* After any rename, move, copy or permanent delete, call
  `organize::meta::{on_move, on_copy, on_delete}` so tags and stars follow.
* To delete on a user's behalf, call `organize::trash::remove` (it honours the
  retention setting) instead of `Storage::delete`.
* Refuse and hide paths for which `organize::trash::is_reserved` is true.
* Record events with `activity::Activity::new(action::...)...record(&state)`.
