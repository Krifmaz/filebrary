# Changelog

## v0.3.2

- Fixed: a release run overtaken by a merge that changes the workflows no longer fails with HTTP 403; it skips publishing with a notice and the newer run releases everything (kit workflows v0.7.3).

## v0.3.0

- The new web UI replaces the old one and is served at `/`: files, uploads, previews, search, share links, settings and administration, all on the kit in the Encore theme (see What's new). `/next/` and `FILEBRARY_NEXT_UI` are gone.
- Themes are the kit's (Encore, dark, black for OLED, light, follow the device); the old colour palettes for branding and per user are no longer used, and `/api/me/theme` no longer takes `colours`.
- Builds, the Docker image and releases build the web UI from `ui/` with trunk; Node.js is no longer needed.
- The web UI opens with no connection through the kit's service worker, which also replaces the old UI's worker.
- Stars made while the server is unreachable wait and are sent once it's back (kit v0.7.2).

## v0.2.3

- Added: the kit's server features on Filebrary's accounts: What's new, notice banners (data folder, unreachable sources), bug reports and crash logs, Diagnostics, settings from a schema and the update card. `/api/health`, `/healthz` and `/health` answer in the kit's format and keep `status: "ok"`.
- New web UI: search by name or by the text inside files, with type, source, date, size and extension filters, sorting, highlighted matches and shareable search links.
- Share links that have used up their download limit now refuse their page and folder listings too, not only downloads.
- New web UI: share links. A Shared page with a filter and one-click cleanup of dead links, a share dialog with expiry presets and drop links, and a public share page with password, browsing, previews and drop uploads with progress.
- New web UI: Settings (account, security, preferences, appearance), administration (users, groups, per-source permissions, jobs and the search index, branding) and Collections.
- The new web UI can browse files: list and grid views, selection, a context menu, move, copy, rename, delete with Undo, the trash, stars, tags, resumable uploads with folders, previews and a text editor.
- The new web UI has What's new, Bug reports (with crash logs and export for admins), Diagnostics, server settings, notice banners, self-update and offline start, all from the kit.
- Uploads to drop links are now chunked and resume after a dropped connection or a reload; public links read `inline=1` as well as `inline=true`.
- CI builds and tests the new Leptos web UI, release binaries include it, and merged branches are deleted automatically.

## v0.2.2

- Added: a new web UI built on the kit (Leptos, Encore theme) to try at `/next/` with `FILEBRARY_NEXT_UI=1`: first-run setup, sign-in, the app frame and the overview.

## v0.2.1

- Releases now ship automatically on every merge through the kit's shared release workflows; the `:X.Y` image tag is no longer published.
