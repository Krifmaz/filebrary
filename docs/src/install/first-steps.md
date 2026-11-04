# First steps

After [installing](docker.md), open Filebrary in a browser:
<http://localhost:8080> by default, or your
[`public_url`](../configuration/reference.md#public_url).

## 1. Create the administrator

There is no default password. On first start, with no users in the
database, Filebrary waits for you to create the administrator:

1. Open Filebrary in a browser. It shows **Set up your server**.
2. Find the one-time **setup token** in the log:

   ```sh
   docker logs filebrary 2>&1 | grep setup_token
   # or: journalctl -u filebrary | grep setup_token
   ```

3. Paste the token, choose the administrator's name and password, and press
   **Create administrator**. You are signed in straight away.

The token proves that whoever claims the server can read its log, so a
stranger who reaches a fresh install first cannot take it over. A new token
is printed each time the server starts while no account exists; it stops
working once the administrator is created. Wrong tokens are rate limited
like logins.

For unattended installs you can instead set
[`admin_password`](../configuration/reference.md#admin_password)
(`FILEBRARY_ADMIN_PASSWORD`): the administrator
[`admin_username`](../configuration/reference.md#admin_username) (default
`admin`) is then created with that password and setup is skipped. Sign in and
change it under **Settings → Account**; you can remove the variable
afterwards.

## 2. Check your sources

The sidebar lists every configured [source](../configuration/sources.md).
With the defaults there is one, `files`. If it is empty but you expected
files, check that the folder is mounted where the source points (in Docker,
inside the container) and that the Filebrary user can read it.

## 3. Add users

Under **Users** in the sidebar, create accounts for other people. Pick a
role (administrator, user or read-only), optionally give each a home
folder, and tick their permissions on each source: view, download, upload,
edit, delete and share. See [Users and permissions](../configuration/users.md).

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
