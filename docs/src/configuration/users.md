# Users and permissions

## The first administrator

On first start, when the database has no users, Filebrary creates an
administrator. Its name is `admin` (or `FILEBRARY_ADMIN_USERNAME`). Its password
is `FILEBRARY_ADMIN_PASSWORD` if set; otherwise a random password is printed
once to the log:

```sh
docker logs filebrary 2>&1 | grep "first administrator"
```

Log in and change it under your account settings (`POST /api/me/password`).

## Logins and sessions

- Passwords are hashed with argon2id and must be at least 8 characters.
- A login sets an `HttpOnly`, `SameSite=Lax` session cookie, `Secure` when
  `public_url` is `https://` (or `secure_cookies = true`). Sessions end after
  `session_ttl_hours` without activity.
- Every state-changing request must send the session's CSRF token (returned
  by the login and by `GET /api/me`) in the `X-CSRF-Token` header.
- After `login_max_attempts` failed logins for one username, or from one
  client address, within `login_window_secs`, further attempts get
  `429 Too Many Requests` with a `Retry-After` header. Behind a reverse proxy
  set `trust_proxy_headers` so the real client address is used.
- Disabling a user or resetting their password ends all their sessions.

## Permissions

Administrators can do everything. Everyone else gets permissions from
**rules**. A rule gives one user or one group some of these permissions on a
folder of a source, and on everything below it:

| Permission | Allows |
| --- | --- |
| `view` | listing folders, seeing names, sizes and previews |
| `download` | reading file contents, zips |
| `upload` | creating files and folders |
| `edit` | renaming, moving, overwriting |
| `delete` | deleting |
| `share` | creating share links |

Any permission implies `view`. To work out what a user may do at a path:

1. Take the user's own rules and their groups' rules that cover the path.
2. Keep the rules on the deepest (most specific) folder.
3. If one of them is the user's own rule, it decides. Otherwise the groups'
   rules there are combined.
4. With no rule, the user can do nothing (the folder is invisible).

A rule with no permissions therefore hides a folder. Rules are checked on the
path a request really reaches after following symlinks, so a link in a
readable folder cannot lead into a hidden one.

A user can also be confined to one folder of a source (a **scope**); the
paths they see start there.

Example: the group `staff` gets `view, download` on `/`, `upload` on
`/drop` and nothing on `/hr`. Staff can browse and download everything
except `/hr`, which they cannot see. In `/drop` they can upload but not
download.

## Admin API

| Endpoint | Purpose |
| --- | --- |
| `GET/POST /api/users`, `GET/PUT/DELETE /api/users/{id}` | Manage users (`PUT` takes `username`, `is_admin`, `disabled`, `password`). |
| `GET/PUT /api/users/{id}/scopes` | Confine a user: `{"source": "files", "scope": "/home/bob"}`. |
| `GET/POST /api/groups`, `DELETE /api/groups/{id}` | Manage groups. |
| `PUT/DELETE /api/groups/{id}/members/{user_id}` | Group membership. |
| `GET /api/permissions?source=`, `PUT /api/permissions`, `DELETE /api/permissions/{id}` | Rules: `{"source": "files", "path": "/drop", "subject": {"type": "group", "id": 2}, "perms": ["upload"]}`. |

The server refuses to remove, demote or disable the last enabled administrator.

## Storage quotas

Each user can have a storage quota, and
[`default_quota`](reference.md#default_quota) sets one for everyone else.
Uploads that would exceed it are refused with `507 quota_exceeded`. See
[Storage quotas](../guide/authentication.md#storage-quotas).

| Endpoint | What it does |
|---|---|
| `GET /api/me/quota` | Your `{limit, used, free}`. |
| `GET/PUT /api/users/{id}/quota` | Read or set a user's limit: `{"limit": "50G"}`, bytes, or `null` for unlimited. |
| `POST /api/users/{id}/quota/rescan` | Measure the user's usage now. |
| `DELETE /api/users/{id}/totp` | Turn off two-factor for a user who lost their device. |
| `GET /api/users/{id}/api-keys`, `DELETE /api/api-keys/{id}` | See and revoke a user's API keys. |
