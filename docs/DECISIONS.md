# Decisions

One line per non-obvious call made while rebuilding Filebrary on [Krifmaz/kit](https://github.com/Krifmaz/kit) (kit docs/DESIGN.md first, then common sense).

- 2026-11-05 · The rebuild runs as parallel tracks with separate file ownership; this file and CHANGELOG.md are kept by the integrator, from the lines each PR lists.
- 2026-11-05 · Accounts and roles stay in Filebrary (`filebrary-identity`) and reach the kit through its `Host` trait; the kit has no accounts crate.
- 2026-11-05 · The new Leptos web app is built in `ui/` beside the React `web/`, which keeps shipping until the cutover PR removes it.
- 2026-11-05 · CI/release use kit-ci, kit-release and kit-docker pinned to kit v0.3.2; Filebrary keeps its own jobs only where the kit can't do the work: Windows (kit-ci steps assume bash), the React build, real-backend tests, docs, Docker smoke test.
- 2026-11-05 · Release binaries are built by Filebrary's own job, not kit-release's server stage: Filebrary embeds the UI at compile time and ships macOS/Windows/musl binaries; artifacts are named `server-*` so kit-release publish attaches them. Revisit at cutover.
- 2026-11-05 · The `-full` image is a second kit-docker call with `VARIANT=full` and `extra-tags` (`<v>-full`, `latest-full`); the `X.Y` floating tag is dropped (kit tags `:<version>` and `:latest` only).
- 2026-11-05 · The release version is stamped at build time from `APP_VERSION`, so Cargo.toml's version is no longer the source of truth for releases.
- 2026-11-05 · Filebrary pins kit v0.3.2: its rusqlite 0.39 and sqlx 0.9 share libsqlite3-sys 0.37, so kit's SQLite stores and Filebrary's sqlx pool live in one Cargo graph.
- 2026-11-05 · The Leptos UI uses real-path routes like the React UI (not the kit template's `#` routes), so share links and deep links keep their addresses; the base path is read from the script's address.
- 2026-11-05 · `/next/` gets its own CSP with `'wasm-unsafe-eval'` and the hash of trunk's inline loader, computed from the embedded `index.html` at first use, so the strict site-wide CSP stays as it is.
- 2026-11-05 · Icons the kit lacks (folder, star, share, menu, search, …) live in `ui/src/icons.rs` for now; they're generic, so they're a candidate to move into kit-leptos.
- 2026-11-05 · Placeholder pages ship for files/search/shares/admin so each later track only edits its own folder.
- 2026-11-05 · Kit data lives in its own `<data_dir>/kit.db` (`kit_*` tables only), not in `filebrary.db`, so sqlx's pool and the kit's single rusqlite connection never contend for one lock.
- 2026-11-05 · Filebrary has no owner role, so every administrator is the kit's admin and owner; an API key counts as admin only with the `admin` scope.
- 2026-11-05 · The kit settings table is read from `src/kit/settings.toml` at start (`file::parse`), and saved values are read synchronously, because `AppState::new` is not async.
- 2026-11-05 · "Update now" answers 409 with the `docker compose pull` command (Filebrary ships as an image); "Restart server" exits with code 75 for the container's restart policy.
