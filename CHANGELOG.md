# Changelog

## v0.2.4

- CI builds and tests the new Leptos web UI, release binaries include it, and merged branches are deleted automatically.

## v0.2.3

- Added: the kit's server features on Filebrary's accounts: What's new, notice banners (data folder, unreachable sources), bug reports and crash logs, Diagnostics, settings from a schema and the update card. `/api/health`, `/healthz` and `/health` answer in the kit's format and keep `status: "ok"`.

## v0.2.2

- Added: a new web UI built on the kit (Leptos, Encore theme) to try at `/next/` with `FILEBRARY_NEXT_UI=1`: first-run setup, sign-in, the app frame and the overview.

## v0.2.1

- Releases now ship automatically on every merge through the kit's shared release workflows; the `:X.Y` image tag is no longer published.
