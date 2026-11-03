# First steps

After [installing](docker.md), open Filebrary in a browser:
<http://localhost:8080> by default, or your
[`public_url`](../configuration/reference.md#public_url).

## 1. Create the administrator

On first start, with no users in the database, Filebrary prints how to
create the first administrator account to its log. Read it with
`docker logs filebrary` or `journalctl -u filebrary`, and follow the
instructions. Sign in with that account.

## 2. Check your sources

The sidebar lists every configured [source](../configuration/sources.md).
With the defaults there is one, `files`. If it is empty but you expected
files, check that the folder is mounted where the source points (in Docker,
inside the container) and that the Filebrary user can read it.

## 3. Add users

Under **Settings → Users**, create accounts for other people and give them
permissions on each source: view, download, upload, edit, delete and share.
See [Users, groups and permissions](../guide/users.md).

## 4. Put it on the internet safely

Before exposing Filebrary beyond your network:

* serve it over HTTPS through a [reverse proxy](../configuration/reverse-proxy.md),
* set [`public_url`](../configuration/reference.md#public_url) to the HTTPS
  address so cookies are `Secure`,
* set [`trust_proxy_headers`](../configuration/reference.md#trust_proxy_headers)
  only if the proxy is the only way in,
* set up [backups](../operations.md) of the data directory.

## 5. Install the app

On a phone or desktop, use the browser's **Install** or **Add to Home
Screen** option to get Filebrary as an app. See
[Installing the app](../guide/pwa.md).
