# Users and permissions

## The first administrator

There is no default password. When the database has no users and
`FILEBRARY_ADMIN_PASSWORD` is not set, the web UI shows a setup page. You
choose the administrator's name and password there, and confirm with the
one-time setup token that the server prints to its log
(`docker logs filebrary 2>&1 | grep setup_token`). See
[First steps](../install/first-steps.md#1-create-the-administrator).

With `FILEBRARY_ADMIN_PASSWORD` set, the administrator `admin` (or
`FILEBRARY_ADMIN_USERNAME`) is created with it instead. Change it under
**Settings → Account** after signing in.

## Roles

Every account has one role:

| Role | Can |
| --- | --- |
| Administrator | everything, on every source; manage users, groups, rules and settings |
| User | what their permission rules allow |
| Read-only | what their rules allow, capped at `view` and `download`: never uploads, edits, deletes or shares, whatever a rule says (an API key of theirs cannot widen that either) |

## Home folders

A **home folder** confines a user to one folder of a source: they see it as
the top of that source and cannot reach anything outside it. When you
create a user with a home folder (or set one later), Filebrary creates the
folder if it is missing and gives the user every permission on it (a
read-only role still caps that). The default path is `/home/<username>`.
Users without a home folder on a source see the whole source, as far as
their rules allow.

## The Users page

Administrators find **Users** in the sidebar. From there they can:

* create an account (username, password or a generated one, role, home folder),
* change a role or disable an account (a disabled account cannot sign in and
  its sessions end),
* reset a password (the user's sessions end; they can change it in
  **Settings → Account**),
* set the home folder and tick the permissions on each source,
* set a storage quota, reset two-factor sign-in, or delete the account.

Every user has **Settings** for their own password, two-factor sign-in, API
keys, theme and default view mode.

## Logins and sessions

- Passwords are hashed with argon2id and must be at least 8 characters.
- A login sets an `HttpOnly`, `SameSite=Lax` session cookie, `Secure` when
  `public_url` is `https://` (or `secure_cookies = true`).
- With **Remember me** ticked, the cookie survives closing the browser and
  the session ends after `session_ttl_hours` without activity. Without it,
  the cookie is dropped when the browser closes and the session ends after
  at most 12 hours without activity.
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

A user can also be confined to one folder of a source (a **scope**, or
home folder); the paths they see start there.

Example: the group `staff` gets `view, download` on `/`, `upload` on
`/drop` and nothing on `/hr`. Staff can browse and download everything
except `/hr`, which they cannot see. In `/drop` they can upload but not
download.

## Admin API

| Endpoint | Purpose |
| --- | --- |
| `GET/POST /api/users`, `GET/PUT/DELETE /api/users/{id}` | Manage users. `POST` takes `username`, `password`, `role` and optionally `home: {"source": "files", "path": "/home/bob"}`; `PUT` takes `username`, `role`, `disabled`, `password`. `role` is `admin`, `user` or `read_only` (`is_admin` is still accepted). |
| `PUT /api/users/{id}/home` | Give a user a home folder: `{"source": "files", "path": "/home/bob"}` (path optional). Creates the folder, confines the user to it and grants them every permission there. |
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
