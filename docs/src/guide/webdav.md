# WebDAV

Filebrary includes a WebDAV server (class 2, with locking), so you can mount
your sources as a network drive. It is on by default at `/dav/` and uses the
same accounts, permissions, per-folder rules, scopes and quotas as the web
UI.

`/dav/` lists the sources you can see; `/dav/<source>/...` is one source.

## Signing in

WebDAV clients use HTTP Basic authentication, so always use HTTPS.

* Use your Filebrary user name and password, or
* an [API key](authentication.md) as the password. Accounts with
  two-factor authentication or single sign-on must use an API key. The key's
  scopes cap what the client may do.

The web session cookie is never accepted on `/dav/`. Failed sign-ins count
towards the same rate limit as the web login
([`login_max_attempts`](../configuration/reference.md#login_max_attempts)).

## Clients

| Client | How to connect |
|---|---|
| Windows Explorer | *This PC → Map network drive*, address `https://files.example.com/dav/files/` |
| macOS Finder | *Go → Connect to Server*, address `https://files.example.com/dav/` |
| rclone | `rclone config`, type `webdav`, vendor `other`, URL `https://files.example.com/dav/` |
| GNOME Files, Dolphin | `davs://files.example.com/dav/` |

Windows only sends Basic credentials over HTTPS unless `BasicAuthLevel` is
raised in the registry.

## Behaviour

* Deleting moves items to the [trash](../organize.md) and is recorded in the
  activity log with the client's address. The trash folder itself is not
  shown over WebDAV.
* Overwriting a file with `COPY` or `MOVE` replaces it permanently, as in the
  web UI.
* Locks and custom properties are kept in memory and are cleared when the
  server restarts.
* `PROPFIND` with `Depth: infinity` is refused; clients fall back to walking
  folders one level at a time.

## Turning it off

Set [`webdav = false`](../configuration/reference.md#webdav) or
`FILEBRARY_WEBDAV=false`.
