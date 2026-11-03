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
* Scripts will use [API keys](guide/authentication.md#api-keys) in the
  `Authorization: Bearer <key>` header once they are available.

## Endpoint groups

| Path | Purpose |
|---|---|
| `GET /healthz`, `GET /api/health` | Health check, no authentication. Returns `{"status":"ok","version":"..."}`. |
| `GET /readyz` | Readiness: database and sources. See [Logs, health and metrics](operations/monitoring.md#health-check). |
| `GET /metrics` | Prometheus metrics. See [Metrics](operations/monitoring.md#metrics). |
| `/api/admin/notifications` | List notification targets and send a test (administrators). |
| `GET /api/openapi.json`, `/api/docs` | This API, as OpenAPI and as a page. |
| `/api/auth/...` | Sign in, sign out, current user. |
| `/api/users`, `/api/groups` | User and group management (administrators). |
| `/api/shares` | Managing your share links. |
| `/api/public/shares/{token}` | Anonymous access to a share link. |
| files, uploads, downloads | Browsing, chunked uploads, downloads with ranges, zip of a selection. |

Exact routes, request bodies and responses are in `/api/openapi.json`.
