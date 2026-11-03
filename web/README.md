# Filebrary web UI

React + TypeScript + Vite single-page app, installable as a PWA. The
production build in `dist/` is embedded into the server binary with
rust-embed, so the server must be built after `npm run build`.

```sh
npm ci
npm run dev        # http://localhost:5173, proxies /api to 127.0.0.1:8080
                   # (override with FILEBRARY_DEV_API=http://host:port)
npm run build      # type-check and build into dist/
npm test           # unit tests (vitest + jsdom)
npm run lint       # eslint
```

## Layout

| Path | What it is |
| --- | --- |
| `src/api.ts` | Typed client for every API call; adds the CSRF header |
| `src/upload.ts` | Chunked, resumable upload queue (8 MiB chunks, 3 files at once, retries) |
| `src/router.ts` | Tiny History-API router (`/login`, `/files/{source}/…`, `/shares`, `/s/{token}`) |
| `src/theme.ts` | Light / dark / system theme, stored per browser |
| `src/viewers.ts` | Registry where feature modules plug in file viewers (editor, previews) |
| `src/features/` | Feature modules; import each from `features/index.ts` |
| `src/components/` | Pages and dialogs |
| `public/sw.js` | Service worker: caches the app shell, never `/api` |

Theme colours are CSS variables on `:root` in `src/styles.css`
(`--bg`, `--surface`, `--surface-2`, `--text`, `--muted`, `--border`,
`--accent`, `--accent-contrast`, `--danger`, `--radius`).
