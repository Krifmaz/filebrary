# Web UI design system

The web UI (`web/`) is built from one set of design tokens and a small kit
of shared components. New screens should use them rather than their own
colours, sizes or one-off widgets, so every page looks and behaves the same
and themes and branding apply everywhere.

## Design tokens

All tokens are CSS variables defined once in `web/src/ui/tokens.css`.

| Group | Tokens | Notes |
|-------|--------|-------|
| Colour | `--bg`, `--surface`, `--surface-2`, `--surface-3`, `--text`, `--muted`, `--faint`, `--border`, `--border-strong`, `--accent`, `--accent-contrast`, `--accent-soft`, `--danger`, `--ok`, `--warn`, `--info` and their `-soft` tints, `--overlay` | Light, dark and custom themes. The ones listed in `THEME_VARS` can be changed by branding; the rest are derived from them. |
| Spacing | `--space-1` (4px) … `--space-16` (64px) | A 4px grid. |
| Type | `--font-sans`, `--font-mono`, `--text-xs` … `--text-3xl`, `--weight-regular` … `--weight-heavy`, `--leading-*`, `--tracking-*` | Inter when installed, else the system font. |
| Radii | `--radius-sm` (6), `--radius-md` (10), `--radius-lg` (14), `--radius-xl` (20), `--radius-full` | Controls use `md`, cards `lg`, dialogs `xl`. |
| Shadows | `--shadow-sm`, `--shadow-md`, `--shadow-lg`, `--focus-ring` | |
| Motion | `--duration-fast`, `--duration`, `--ease` | Turned off for people who ask for reduced motion. |
| Layers | `--z-sticky`, `--z-drawer`, `--z-overlay`, `--z-menu`, `--z-toast` | |
| Layout | `--sidebar-w`, `--topbar-h`, `--content-max`, `--control-h`, `--control-h-sm`, `--control-h-lg`, `--icon-sm/md/lg` | |

Every text and background pair used by the components meets WCAG AA
contrast (4.5:1) in both built-in themes.

## Components

Import them from `web/src/ui`:

```tsx
import { Button, Field, Dialog, toast, EmptyState } from "../ui";
```

| Component | Use it for |
|-----------|-----------|
| `Button`, `IconButton` | Every button. Variants: `primary`, `secondary` (default), `ghost`, `danger`, `danger-solid`; sizes `sm`, `md`, `lg`; `icon`, `iconAfter`, `busy`, `block`. `IconButton` needs a `label`, used for screen readers and the tooltip. |
| `Field` | A label, control, hint and error, with `id`, `aria-describedby` and `aria-invalid` wired up. Wrap one `<input>`, `<select>` or `<textarea>`. |
| `SearchInput`, `Switch` | A search box with an icon and clear button; an accessible on/off switch. |
| `Menu`, `MenuButton` | Dropdowns and right-click menus, with full keyboard support. |
| `Dialog` | Modal dialogs (native `<dialog>`): title, description, body and actions. Becomes a bottom sheet on phones. |
| `toast()` | Short notices: `toast("…")`, `toast.success("…")`, `toast.error("…")`, with an optional action such as Undo. `<Toaster />` is mounted once by the app. |
| `EmptyState`, `ErrorState` | A page or list with nothing in it, or that failed to load (with "Try again"). |
| `Skeleton`, `SkeletonRows`, `SkeletonTiles`, `Spinner` | Loading placeholders. Prefer skeletons that match the final layout. |
| `PageHeader`, `Card`, `Badge`, `Avatar` | The title row of a page, raised panels, pill labels and user initials. |
| `Icon` | The built-in line icon set (`fileIcon(name, isDir)` picks one for a file). |

Plain `<button>`, `<input>`, `<select>` and `<textarea>` elements are styled
too, so older markup matches without changes.

## Layout

The signed-in app frame is `components/Shell.tsx`: a sidebar with the
sources and library links, and a top bar with search and the user menu.

* **Desktop** (wider than 1024px): full sidebar.
* **Tablet** (641–1024px): the sidebar collapses to icons.
* **Phone** (640px and below): the sidebar becomes a drawer behind the menu
  button, dialogs become bottom sheets, and the file browser's actions move
  to a bar at the bottom of the screen.

## Checking a change

Run the UI tests, lint and type checks from `web/`:

```sh
npm test
npm run lint
npm run build
```

and look at the screens in both themes at phone and desktop width.
