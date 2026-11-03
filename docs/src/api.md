# REST API

Everything the web UI does goes through a JSON API under `/api`, so anything
you can do in the UI you can script.

Every server describes its API in an OpenAPI 3.1 document at
`/api/openapi.json`, and shows it as a readable reference at `/api/docs`
(served by Filebrary itself, no CDN, works offline). Import the JSON into
any OpenAPI tool to generate a client. This page describes the conventions
and the main groups of endpoints.

## Conventions

* Requests and responses are JSON (`Content-Type: application/json`),
  except file uploads and downloads.
* File paths are given per source, relative to the source root, and are
  validated as described in [Path safety](configuration/sources.md#path-safety).
* Errors return an HTTP status and a body like
  `{"error": "not_found", "message": "..."}`.
* Requests from the browser use the session cookie and must send the CSRF
  token on every request that changes something.
* Scripts use [API keys](guide/authentication.md#api-keys) in the
  `Authorization: Bearer fbk_...` header instead of a cookie; no CSRF token
  is needed. A key can do only what its scopes and its owner allow.

## Endpoint groups

| Path | Purpose |
|---|---|
| `GET /healthz`, `GET /api/health` | Health check, no authentication. Returns `{"status":"ok","version":"..."}`. |
| `GET /readyz` | Readiness: database and sources. See [Logs, health and metrics](operations/monitoring.md#health-check). |
| `GET /metrics` | Prometheus metrics. See [Metrics](operations/monitoring.md#metrics). |
| `/api/admin/notifications` | List notification targets and send a test (administrators). |
| `GET /api/openapi.json`, `/api/docs` | This API, as OpenAPI and as a page. |
| `GET/POST /api/setup` | First-run setup: whether it is waiting, and creating the first administrator with the setup token (no authentication; works once). |
| `/api/auth/...` | Sign in (`remember` for a persistent session), sign out, current user, available sign-in methods (`/api/auth/methods`), SSO (`/api/auth/oidc/login`, `/api/auth/oidc/callback`). |
| `/api/me/password`, `/api/me/preferences`, `/api/me/theme` | Change your password; your default view mode and theme. |
| `/api/me/totp...`, `/api/me/api-keys...`, `/api/me/quota` | Your two-factor settings, API keys and quota. |
| `/api/users`, `/api/groups`, `/api/permissions` | Users (roles, home folders, password resets), groups and permission rules (administrators). |
| `/api/shares` | Managing your share links. |
| `/api/public/shares/{token}` | Anonymous access to a share link. |
| files, uploads, downloads | Browsing, chunked uploads, downloads with ranges, zip of a selection. |
| `/api/sources/{id}/text` | Load and save text files for the editor, with ETag conflict detection ([details](guide/files.md#text-editor)). |

Exact routes, request bodies and responses are in `/api/openapi.json`.
