# REST API

Everything the web UI does goes through a JSON API under `/api`, so anything
you can do in the UI you can script.

> An OpenAPI specification and a built-in API documentation page are
> planned for Phase 4. Until then, this page describes the conventions and
> the main groups of endpoints. See [Feature status](status.md).

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
| `/api/auth/...` | Sign in, sign out, current user. |
| `/api/users`, `/api/groups` | User and group management (administrators). |
| `/api/shares` | Managing your share links. |
| `/api/public/shares/{token}` | Anonymous access to a share link. |
| files, uploads, downloads | Browsing, chunked uploads, downloads with ranges, zip of a selection. |

Exact routes and request bodies will be published in the OpenAPI
specification.
