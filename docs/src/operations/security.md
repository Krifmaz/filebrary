# Security model

What Filebrary does to keep your files safe, and what it expects from you.

## Paths and storage

* Every client path is validated before it reaches a storage backend;
  traversal with `..`, encoded variants and outside-pointing symbolic links
  are refused. Details: [Path safety](../configuration/sources.md#path-safety).
* No user input is ever passed to a shell. External programs (ffmpeg,
  LibreOffice, custom jobs) are started directly with an argument list.
* Custom jobs can only be defined in the configuration file by an
  administrator, never from the web UI.

## Accounts and sessions

* Passwords are hashed with argon2id.
* Session cookies are `HttpOnly`, `SameSite`, and `Secure` over HTTPS.
* Changing requests need a CSRF token.
* Failed logins are rate limited per username and per client address.
* Share-link passwords are hashed, and share tokens are long random values.

## HTTP headers

Every response carries `X-Content-Type-Options: nosniff`,
`Referrer-Policy: no-referrer`, `X-Frame-Options: DENY` and a restrictive
`Content-Security-Policy` (`default-src 'self'`, no framing).

## What you should do

* Serve Filebrary over **HTTPS** if it is reachable from outside your
  network, and set [`public_url`](../configuration/reference.md#public_url)
  to the `https://` address.
* Only enable [`trust_proxy_headers`](../configuration/reference.md#trust_proxy_headers)
  behind a proxy that is the only way in.
* Keep the data directory, especially `secret.key`, readable only by the
  Filebrary user, and outside every source.
* Run as an unprivileged user (the Docker image runs as `65532`), and give
  it write access only where uploads are wanted.
* Keep Filebrary up to date.

## Reporting a vulnerability

Please report security problems privately through
[GitHub security advisories](https://github.com/Krifmaz/filebrary/security/advisories/new)
rather than in a public issue.
