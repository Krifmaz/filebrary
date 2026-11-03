# filebrary-webdav

The WebDAV server (RFC 4918, classes 1 and 2) used by Filebrary at `/dav/`.
It runs on the `filebrary_storage::Storage` trait, so it serves every backend
the web UI serves, and it gets users, permissions and confined backends from
the `DavAuth` trait, which the server crate implements on its user database.

## Methods

| Method | Notes | Permission |
|---|---|---|
| `OPTIONS` | `DAV: 1, 2`, `MS-Author-Via: DAV`; answered without login (Windows asks first) | none |
| `PROPFIND` | Depth 0 and 1; `Depth: infinity` gets 403 `propfind-finite-depth` | view |
| `GET`, `HEAD` | single byte ranges, `If-None-Match`, `If-Modified-Since`, `If-Range`; collections get a small HTML index | download (view for collections) |
| `PUT` | streamed to storage (chunked works); `Content-Range` refused; quota checked against `Content-Length` / `X-Expected-Entity-Length` | upload (new), edit (existing) |
| `MKCOL` | 405 if it exists, 409 without a parent, 415 with a body | upload |
| `DELETE` | recursive; the source root cannot be deleted | delete |
| `COPY` | Depth 0 or infinity, `Overwrite`, across sources too | download on the source; upload (new) or edit + delete (replace) on the destination |
| `MOVE` | `Overwrite`, across sources too | edit on the source; as `COPY` on the destination |
| `PROPPATCH` | dead properties (Windows `Win32*`, Finder, anything); live properties are protected (403, others 424) | edit |
| `LOCK`, `UNLOCK` | exclusive and shared write locks, depth 0 or infinity, refresh, timeouts up to an hour, locking an unmapped URL creates an empty file | edit (existing) or upload (new) / view |

The full `If` header is supported (tagged and untagged lists, `Not`, entity
tags). Lock tokens only count when submitted by the user who holds the lock.
Quota properties (RFC 4331) are reported when the user has a quota.

## Safety

* URLs are percent-decoded exactly once and every segment goes through
  `VPath::join`, so `..`, `%2e%2e`, `%2f`, `%5c` and NUL never reach storage.
  `Destination` and tagged `If` URLs go through the same parser.
* Storage calls use the user-visible path on a backend confined to the
  user's scope with `Storage::sub`, so symlinks cannot lead out of a scope.
* XML bodies are capped at 1 MiB and parsed with `roxmltree`, which refuses
  DTDs (no entity expansion). Dead properties are capped per resource and in
  total.
* Downloads carry `Content-Security-Policy: sandbox` and `nosniff`, so a
  browser never runs user content on the server's origin.
* Nothing goes through a shell.

## State

Locks and dead properties live in memory and are keyed by source and real
path, so users with different scopes see each other's locks. They are lost on
restart: clients take locks again, and only cosmetic properties such as
Windows' `Win32FileAttributes` are forgotten. `PropStore` can be implemented
on a database to keep properties.

## Tests

`tests/litmus.rs` follows the litmus suite's basic, copymove, props, locks
and http groups; `tests/security.rs` covers traversal, scopes, symlinks and
permissions. Both run against temporary local sources:

```sh
cargo test -p filebrary-webdav
```
