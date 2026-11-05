# ui/ — Filebrary's Leptos web UI

Filebrary's web UI: [Leptos](https://leptos.dev) 0.8 (client-side, WebAssembly) on the kit's components
([kit-leptos](https://github.com/Krifmaz/kit/tree/v0.3.2/crates/kit-leptos)) and the kit's Encore theme. It is a
Cargo workspace of its own, like the kit template's `web/`, so the server's dependency graph stays as it is. The
server embeds `ui/dist` and serves it at `/`.

## Build and try it

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
cd ui && trunk build --release              # → ui/dist
cd .. && cargo build --release -p filebrary  # embeds ui/dist
./target/release/filebrary
```

Then open `http://localhost:8080/`. `trunk serve` works too, with `/api` proxied to a running server (add a `[[proxy]]` to `Trunk.toml`).

## Layout

| File | What |
| --- | --- |
| `src/main.rs` | start-up and the session (setup → sign in → app; public share links need no account) |
| `src/api.rs` | typed client for the REST API on the kit's HTTP backend, with the CSRF header |
| `src/route.rs` | real-path routes under the base read from the script's address (`/` when served at the root) |
| `src/shell.rs` | the frame: sidebar with sources, top bar with search and theme, phone drawer |
| `src/pages/` | `auth` (sign-in, setup), `home`, `files/`, `search/`, `shares/`, `admin/` |
| `src/kit.rs` | the kit's features: toasts, What's new, notices, bug reports, diagnostics, self-update, offline start |
| `src/theme.rs` · `src/icons.rs` | the kit theme on `<html data-theme>`; every icon the pages use that the kit doesn't have yet |
| `static/css/` | `tokens.css` and `kit.css` copied from the pinned kit (`scripts/sync-kit.sh`), the kit template's `app.css`, then `filebrary.css` and one file per page (`files`, `search`, `shares`, `admin`) |
| `static/` | the icons and the PWA manifest (the server fills in the branding) |

## Checks

```sh
cd ui && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && trunk build --release
```

`cargo test` also fails when `static/css/kit.css` differs from the pinned kit's; `scripts/sync-kit.sh` copies it
again after a kit tag change.
