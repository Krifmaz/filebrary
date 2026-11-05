# Decisions

One line per non-obvious call made while rebuilding Filebrary on [Krifmaz/kit](https://github.com/Krifmaz/kit) (kit docs/DESIGN.md first, then common sense).

- 2026-11-05 · The rebuild runs as parallel tracks with separate file ownership; this file and CHANGELOG.md are kept by the integrator, from the lines each PR lists.
- 2026-11-05 · Accounts and roles stay in Filebrary (`filebrary-identity`) and reach the kit through its `Host` trait; the kit has no accounts crate.
- 2026-11-05 · The new Leptos web app is built in `ui/` beside the React `web/`, which keeps shipping until the cutover PR removes it.
- 2026-11-05 · CI/release use kit-ci, kit-release and kit-docker pinned to kit v0.3.0; Filebrary keeps its own jobs only where the kit can't do the work: Windows (kit-ci steps assume bash), the React build, real-backend tests, docs, Docker smoke test.
- 2026-11-05 · Release binaries are built by Filebrary's own job, not kit-release's server stage: Filebrary embeds the UI at compile time and ships macOS/Windows/musl binaries; artifacts are named `server-*` so kit-release publish attaches them. Revisit at cutover.
- 2026-11-05 · The `-full` image is a second kit-docker call with `VARIANT=full` and `extra-tags` (`<v>-full`, `latest-full`); the `X.Y` floating tag is dropped (kit tags `:<version>` and `:latest` only).
- 2026-11-05 · The release version is stamped at build time from `APP_VERSION`, so Cargo.toml's version is no longer the source of truth for releases.
