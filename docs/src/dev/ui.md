# Web UI design system

The web UI lives in `ui/`: a [Leptos](https://leptos.dev) app (client-side,
WebAssembly) built by [trunk](https://trunkrs.dev) into `ui/dist`, which the
server embeds and serves at `/`. It is built on
[Krifmaz/kit](https://github.com/Krifmaz/kit): the kit's design tokens, its
components (`kit-leptos`) and its Encore theme. Read the kit's
`docs/DESIGN.md` before adding a screen, and use the kit rather than new
one-off widgets, so every page looks and behaves the same in every theme.
What the kit lacks and is generic belongs in the kit, not here.

## Design tokens

Colours, radii, spacing and type are the kit's CSS variables, in
`ui/static/css/tokens.css` (a copy of the pinned kit's; `ui/scripts/sync-kit.sh`
copies it again after a kit tag change). The themes are `encore` (the
default), `dark`, `oled` and `light` on `<html data-theme>`, plus `system`,
which follows the device. Filebrary's own rules use tokens only: a test fails
when `search.css`, `shares.css` or `admin.css` carries a literal colour.

## Stylesheets

`ui/index.html` links them in this order:

| File | What |
|------|------|
| `tokens.css`, `kit.css` | The kit's tokens and components (copies of the pinned kit; a test checks `kit.css`). |
| `app.css` | The kit template's app frame. |
| `filebrary.css` | The top bar, the phone drawer, the overview and the sign-in card. |
| `files.css`, `search.css`, `shares.css`, `admin.css` | One file per group of pages. |

## Components and icons

Buttons, inputs, cards, tabs, modals, confirm dialogs, toasts (errors stay,
with Copy), the dropzone and banners come from `kit-leptos`. Icons are the
kit's; `ui/src/icons.rs` draws the ones the kit does not have yet, the same
way (stroked 24 × 24 outlines), and `crate::icons::icon(name)` returns either.

## Layout

The signed-in frame is `ui/src/shell.rs`: a sidebar with the sources, the
library links, Admin and Help, and a top bar with search and the theme.

* **Desktop** (wider than 900px): the full sidebar.
* **Phone and small tablet** (900px and below): the sidebar becomes a drawer
  behind the menu button, menus become sheets, and the file browser's
  selection actions move to a bar at the bottom of the screen.

## Checking a change

From `ui/`:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
trunk build --release
```

then look at the screens in each theme at phone and desktop width.
