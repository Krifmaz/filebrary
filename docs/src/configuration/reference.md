# Settings reference

This page lists every setting. How settings are loaded (file, environment,
defaults), the command line and the value formats are explained in the
[configuration overview](overview.md). A complete commented file is on the
[example configuration](example.md) page.

Each setting below is a top-level TOML key unless its name contains a dot
(a table) or `[]` (an array of tables).

> **For contributors:** every field of the `Config` struct must have a
> heading here, and every heading must match a field. `cargo test` checks
> this (`crates/filebrary/tests/docs_config.rs`). See
> [Contributing](../contributing.md#keeping-the-settings-reference-in-sync).

## Server

### `listen`

The address and port the web server listens on.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | socket address (`host:port`)            |
| Default     | `"127.0.0.1:8080"`                      |
| Environment | `FILEBRARY_LISTEN`                      |

The default only accepts connections from the same machine. Use
`"0.0.0.0:8080"` to listen on every IPv4 address, or `"[::]:8080"` for IPv6
(which on Linux usually accepts IPv4 too). The Docker image sets
`FILEBRARY_LISTEN=0.0.0.0:8080`.

```toml
listen = "0.0.0.0:8080"
```

### `data_dir`

The directory that holds Filebrary's own state: the SQLite database
(`filebrary.db`, with its `-wal` and `-shm` files), the staging area for
resumable uploads, and `secret.key`, the key that signs sessions and share
tokens. It is created if it does not exist.

|             |                         |
|-------------|-------------------------|
| Type        | path                    |
| Default     | `"./data"` (`/data` in the Docker image) |
| Environment | `FILEBRARY_DATA_DIR`    |

Back this directory up; see [Backups and upgrades](../operations.md). It must be on
a local file system (SQLite does not work reliably over NFS or SMB). Keep it
outside every source so users cannot browse it.

If no [`sources`](#sources) are configured, Filebrary serves
`<data_dir>/files` as a single source called `files`.

### `public_url`

The address users reach Filebrary at, for example
`"https://files.example.com"`. It is used to build absolute share links and to
decide whether session cookies are marked `Secure`.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | URL starting with `http://` or `https://`, optional |
| Default     | not set                                  |
| Environment | `FILEBRARY_PUBLIC_URL` (an empty value unsets it) |

Set it whenever Filebrary runs behind a reverse proxy or on a non-default
port. Serving under a sub-path is not supported yet; use a sub-domain.

### `log_format`

How log lines are written to standard output.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | `"text"` or `"json"`                    |
| Default     | `"text"`                                |
| Environment | `FILEBRARY_LOG_FORMAT`                  |

`json` writes one JSON object per line, for log collectors such as Loki,
Vector or Elasticsearch. The level filter is set separately with
`FILEBRARY_LOG`. See [Logs, health and metrics](../operations/monitoring.md).

## Sessions and sign-in

### `session_ttl_hours`

How long a login with **Remember me** stays valid without activity, in
hours. Using the session extends it: once less than half of this time is
left, the next request renews it to the full length. Logins without
Remember me last at most 12 hours (or this value, if smaller) and their
cookie is dropped when the browser closes.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer, at least `1`                   |
| Default     | `168` (7 days)                          |
| Environment | `FILEBRARY_SESSION_TTL_HOURS`           |

### `secure_cookies`

Forces the `Secure` flag on session and CSRF cookies on (`true`) or off
(`false`). A `Secure` cookie is only sent over HTTPS.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | boolean, optional                       |
| Default     | not set: on when [`public_url`](#public_url) starts with `https://`, off otherwise |
| Environment | `FILEBRARY_SECURE_COOKIES`              |

Only set this to `false` if you really serve Filebrary over plain HTTP.
Browsers will not sign you in over HTTP while it is `true`.

### `trust_proxy_headers`

Whether to take the client's address from the `X-Forwarded-For` header
instead of the TCP connection. The client address is used for login rate
limiting and the activity log.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | boolean                                 |
| Default     | `false`                                 |
| Environment | `FILEBRARY_TRUST_PROXY_HEADERS`         |

Turn it on only when Filebrary is reachable **only** through a reverse proxy
you control that sets this header. Otherwise anyone can forge their address
and get around rate limiting. See [Running behind a reverse proxy](reverse-proxy.md).

### `login_max_attempts`

Failed logins allowed for one username, or from one client address, within
[`login_window_secs`](#login_window_secs). Further attempts are refused with
`429 Too Many Requests` until the window has passed.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer, at least `1`                   |
| Default     | `5`                                     |
| Environment | none (file only)                        |

### `login_window_secs`

The length of the login rate-limiting window, in seconds.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer                                 |
| Default     | `300` (5 minutes)                       |
| Environment | none (file only)                        |

### `admin_username`

Name of the administrator created on first start, when the database has no
users yet. Ignored once any user exists. See
[Users and permissions](users.md#the-first-administrator).

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string                                  |
| Default     | `"admin"`                               |
| Environment | `FILEBRARY_ADMIN_USERNAME`              |

### `admin_password`

An optional bootstrap password for that first administrator, for
unattended installs. If unset there is no default password: the web UI shows
a setup page where you choose the name and password, confirmed with a
one-time setup token printed to the log (see
[First steps](../install/first-steps.md#1-create-the-administrator)).
Prefer the environment variable (or a secret manager) to writing a password
into the file, and change it after the first sign-in.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string, optional                        |
| Default     | not set: first-run setup in the web UI  |
| Environment | `FILEBRARY_ADMIN_PASSWORD` (an empty value unsets it) |

## Uploads

### `max_chunk_mib`

The largest single chunk of a resumable upload, in MiB. The web UI splits
files into chunks no bigger than this, so files of any size can be uploaded.
Lower it if a reverse proxy in front of Filebrary limits request bodies (for
example nginx's `client_max_body_size`).

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer, `1` to `4096`                  |
| Default     | `64`                                    |
| Environment | none (file only)                        |

### `max_drop_upload_mib`

The largest file a visitor may send through a public upload-only
("drop") share link, in MiB. Uploads by signed-in users are limited by their
quota instead.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer                                 |
| Default     | `1024` (1 GiB)                          |
| Environment | none (file only)                        |

### `upload_expiry_hours`

Hours after which an unfinished resumable upload is discarded and its
staged chunks deleted from [`data_dir`](#data_dir).

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer                                 |
| Default     | `24`                                    |
| Environment | none (file only)                        |

## Trash and activity log

### `trash_retention_days`

Days a deleted item stays in the trash before it is purged automatically.
`0` turns the trash off, so deletes are permanent. Purging runs hourly. See
[Trash, tags, stars and activity](../organize.md).

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer, `0` to `36500`                 |
| Default     | `30`                                    |
| Environment | `FILEBRARY_TRASH_RETENTION_DAYS`        |

### `activity_retention_days`

Days activity-log events are kept. `0` keeps them forever.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer, `0` to `36500`                 |
| Default     | `365`                                   |
| Environment | `FILEBRARY_ACTIVITY_RETENTION_DAYS`     |

## WebDAV

### `webdav`

Serve WebDAV (class 2, with locking) at `/dav/`, for Windows Explorer,
macOS Finder, rclone and other clients. It uses the same accounts,
permissions, scopes and login rate limit as the web UI. See
[WebDAV](../guide/webdav.md).

|             |                                         |
|-------------|-----------------------------------------|
| Type        | boolean                                 |
| Default     | `true`                                  |
| Environment | `FILEBRARY_WEBDAV`                      |

## Two-factor login and quotas

### `totp_issuer`

The name authenticator apps show next to Filebrary's two-factor codes. See [Two-factor authentication](../guide/authentication.md#two-factor-authentication-totp).

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | `"Filebrary"` |

### `default_quota`

The storage limit for users who have no limit of their own. Administrators set per-user limits in the admin pages or with `PUT /api/users/{id}/quota`; a user set to unlimited stays unlimited.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | size such as `"500M"`, `"10G"` or `"1.5T"` (binary units), optional |
| Default     | not set (unlimited) |
| Environment | `FILEBRARY_DEFAULT_QUOTA` (an empty value unsets it) |

Uploads that would go over a user's limit fail with `507 quota_exceeded`. See [Storage quotas](../guide/authentication.md#storage-quotas).

### `quota_rescan_hours`

Hours between measurements of each limited user's real usage: the size of everything in their home folder (their scope on each source). This corrects the running total for deletes, abandoned uploads and changes made outside Filebrary.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | whole number of hours |
| Default     | `6` |
| Environment | `FILEBRARY_QUOTA_RESCAN_HOURS` |

`0` turns the rescan off; usage is then only counted on upload.

## Single sign-on (OIDC)

The `[oidc]` table turns on OpenID Connect sign-in. Leave it out to turn SSO off. It needs [`public_url`](#public_url), because the provider redirects back to `<public_url>/api/auth/oidc/callback`. See [Single sign-on](../guide/authentication.md#single-sign-on-oidc).

```toml
[oidc]
issuer_url = "https://auth.example.com"
client_id = "filebrary"
client_secret_file = "/run/secrets/oidc_client_secret"
admin_group = "filebrary-admins"
```

### `oidc.issuer_url`

The provider's issuer URL. Discovery is read from `/.well-known/openid-configuration` below it.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | URL; `https://` except for `localhost` |
| Default     | required |

### `oidc.client_id`

The client id registered at the provider.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | required |

### `oidc.client_secret`

The client secret. Prefer `client_secret_file` or the environment variable so the secret is not in the file.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string, optional |
| Default     | not set (public client) |
| Environment | `FILEBRARY_OIDC_CLIENT_SECRET` (overrides both this and `client_secret_file`) |

### `oidc.client_secret_file`

A file holding the client secret, such as a Docker secret. Trailing newlines are ignored.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | path, optional |
| Default     | not set |

### `oidc.scopes`

Scopes requested besides `openid`.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | array of strings |
| Default     | `["profile", "email", "groups"]` |

### `oidc.username_claim`

The claim used as the local username on first sign-in. Falls back to a verified `email`, then `sub`.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | `"preferred_username"` |

### `oidc.groups_claim`

The claim listing the user's groups. A user joins each local group with the same name and leaves local groups the claim no longer lists.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | `"groups"` |

### `oidc.admin_group`

Members of this provider group are administrators; others are not. Checked at every sign-in. Unset: administrator rights are managed in Filebrary.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string, optional |
| Default     | not set |

### `oidc.allowed_groups`

Only members of at least one of these groups may sign in.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | array of strings |
| Default     | `[]` (everyone the provider accepts) |

### `oidc.auto_create`

Create a local user on first sign-in. When off, an administrator creates the account first (with the `oidc` source and the same username), and the first sign-in links to it.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | boolean |
| Default     | `true` |

### `oidc.button_label`

Text of the sign-in button on the login page.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | `"Sign in with SSO"` |

### `oidc.disable_password_login`

Hide and refuse password login for everyone except administrators, who keep it as a way in when the provider is down.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | boolean |
| Default     | `false` |

## LDAP

The `[ldap]` table lets users who are not local password accounts sign in with their directory password. Leave it out to turn LDAP off. See [LDAP](../guide/authentication.md#ldap).

```toml
[ldap]
url = "ldaps://ldap.example.com"
bind_dn = "cn=filebrary,ou=services,dc=example,dc=com"
bind_password_file = "/run/secrets/ldap_password"
base_dn = "ou=people,dc=example,dc=com"
```

### `ldap.url`

The directory server.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | `ldaps://host:636` or `ldap://host:389` |
| Default     | required |

### `ldap.starttls`

Upgrade an `ldap://` connection with StartTLS. Use this or `ldaps://` so passwords are not sent in clear text.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | boolean |
| Default     | `false` |

### `ldap.bind_dn`

The service account used to search for users.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | DN, optional |
| Default     | not set (anonymous search) |

### `ldap.bind_password`

The service account's password. Prefer `bind_password_file` or the environment variable.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string, optional |
| Default     | not set |
| Environment | `FILEBRARY_LDAP_BIND_PASSWORD` (overrides both this and `bind_password_file`) |

### `ldap.bind_password_file`

A file holding the service account's password. Trailing newlines are ignored.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | path, optional |
| Default     | not set |

### `ldap.base_dn`

Where users are searched for (subtree).

|             |                                         |
|-------------|-----------------------------------------|
| Type        | DN |
| Default     | required |

### `ldap.user_filter`

The search filter. `{username}` is replaced by the login name, escaped so it cannot change the filter. Exactly one entry must match. For Active Directory use `(&(objectClass=user)(sAMAccountName={username}))`.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | LDAP filter containing `{username}` |
| Default     | `"(&(objectClass=person)(uid={username}))"` |

### `ldap.username_attribute`

The attribute holding the login name stored in Filebrary (`sAMAccountName` for Active Directory).

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | `"uid"` |

### `ldap.email_attribute`

The attribute holding the e-mail address.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | `"mail"` |

### `ldap.display_name_attribute`

The attribute holding the display name.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | `"cn"` |

### `ldap.group_attribute`

The attribute listing the user's group DNs. Each group's `cn` is matched to local group names, as for SSO.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | `"memberOf"` |

### `ldap.required_group`

Only members of this group may sign in.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | group DN, optional |
| Default     | not set |

### `ldap.admin_group`

Members of this group are administrators; others are not. Checked at every sign-in.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | group DN, optional |
| Default     | not set |

### `ldap.timeout_secs`

Network timeout for each LDAP operation, in seconds.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | whole number |
| Default     | `10` |

## Search

These settings live in a `[search]` table. See [Search](../guide/search.md).

```toml
[search]
enabled = true
watch = true
```

### `search.enabled`

Build and maintain the file index. When off, no indexing runs in the
background and `GET /api/search` answers with an error.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | boolean                                 |
| Default     | `true`                                  |
| Environment | `FILEBRARY_SEARCH_ENABLED`              |

### `search.watch`

Watch local sources for changes (inotify on Linux, FSEvents on macOS,
ReadDirectoryChangesW on Windows), so changes made outside Filebrary are
searchable within about a second. On Linux each folder uses one inotify
watch; raise `fs.inotify.max_user_watches` for very large trees. If the
watcher cannot start, the source falls back to periodic rescans and the
error is shown in the search status.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | boolean                                 |
| Default     | `true`                                  |
| Environment | `FILEBRARY_SEARCH_WATCH`                |

### `search.rescan_minutes`

Minutes between full rescans of sources that are not watched: network
sources, sources with `search.watch` off, and sources whose watcher could
not start.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer, at least `1`                   |
| Default     | `60`                                    |
| Environment | `FILEBRARY_SEARCH_RESCAN_MINUTES`       |

### `search.watched_rescan_hours`

Hours between full rescans of watched sources, a safety net for missed
events. `0` turns them off.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer                                 |
| Default     | `24`                                    |

### `search.exclude_sources`

Names of sources that are never indexed (and so never appear in search).

|             |                                         |
|-------------|-----------------------------------------|
| Type        | list of source names                    |
| Default     | `[]`                                    |

### `search.batch_size`

Rows written per database transaction while scanning. Larger batches scan a
little faster and use a little more memory.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer, `1` to `10000`                 |
| Default     | `500`                                   |

## Previews

Settings for thumbnails and previews, in the `[previews]` table. See
[Previews and conversions](../guide/previews.md).

### `previews`

The `[previews]` table.

```toml
[previews]
cache_mib = 4096
thumb_size = 320
```

### `previews.enabled`

Turns thumbnails, previews, media info and transcoded playback on or off.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | boolean                                 |
| Default     | `true`                                  |
| Environment | `FILEBRARY_PREVIEWS`                    |

### `previews.cache_mib`

Size limit of the preview cache in `<data_dir>/cache/previews`, in MiB. Every
10 minutes the least recently used previews are deleted until the cache fits.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer (MiB)                           |
| Default     | `2048`                                  |
| Environment | `FILEBRARY_PREVIEW_CACHE_MIB`           |

### `previews.thumb_size`

Longest side of thumbnails, in pixels.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer, `32` to `1024`                 |
| Default     | `256`                                   |
| Environment | (none)                                  |

### `previews.large_size`

Longest side of large previews (RAW, HEIC, TIFF, video frames), in pixels.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer, `256` to `8192`                |
| Default     | `1920`                                  |
| Environment | (none)                                  |

### `previews.max_parallel`

How many previews are generated at the same time. `0` means one per CPU, at
most 8. LibreOffice conversions always run one at a time.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer                                 |
| Default     | `0`                                     |
| Environment | (none)                                  |

### `previews.max_remote_mib`

Largest file from a remote source (S3, SFTP, SMB) that is copied to the
server to make a preview, in MiB. Larger files get no preview.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer (MiB)                           |
| Default     | `1024`                                  |
| Environment | (none)                                  |

## Sources

### `sources`

The storage sources to serve, as an array of tables. Each source appears as
a top-level location in the web UI and has its own permissions. See
[Storage sources](sources.md).

|             |                                         |
|-------------|-----------------------------------------|
| Type        | array of tables (`[[sources]]`)          |
| Default     | empty: serve `<data_dir>/files` as `files` (`files=/srv` in the Docker image) |
| Environment | `FILEBRARY_SOURCES`, as `name=path` pairs separated by `;` (or `,`), local sources only |

```toml
[[sources]]
name = "files"
path = "/srv/files"

[[sources]]
name = "media"
path = "/mnt/pool/media"
```

The same with an environment variable:

```sh
FILEBRARY_SOURCES="files=/srv/files;media=/mnt/pool/media"
```

A non-empty `FILEBRARY_SOURCES` replaces every source from the file. An
empty value counts as unset, so `FILEBRARY_SOURCES=` lets the file's
`[[sources]]` apply even in the Docker image, which sets the variable.

### `sources[].name`

The source's identifier. It appears in URLs, permissions and the API, so
changing it later breaks existing share links and permission entries for
that source.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string: 1 to 32 characters from `a-z`, `0-9`, `-` and `_` |
| Default     | required                                |

Names must be unique.

### `sources[].kind`

Which storage backend the source uses.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | `"local"`, `"s3"`, `"sftp"` or `"smb"`  |
| Default     | `"local"`                               |

`local` serves a directory on the host (set [`path`](#sourcespath)),
including FUSE file systems such as mergerfs pools. The other kinds take
their settings from the sub-table of the same name:
[`sources[].s3`](#sourcess3), [`sources[].sftp`](#sourcessftp) or
[`sources[].smb`](#sourcessmb). A source must have exactly the table that
matches its kind.

### `sources[].path`

For `local` sources, the root directory (leave it unset for other kinds). Users can never reach anything
outside it: paths are normalised and checked, and symbolic links that point
outside the root are refused.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | path                                    |
| Default     | required                                |

The directory is created at start-up if it does not exist. It must be
readable (and writable, for uploads) by the user Filebrary runs as. In Docker
this is the path *inside* the container.

### `sources[].content_search`

Whether the text inside this source's documents (plain text and code, PDF,
Word, PowerPoint, Excel/ODS, OpenDocument) is indexed for
[content search](../guide/search.md#content-search). Turn it off for sources
that hold mostly media, or that sit on slow network storage, to save disk
space and CPU time. Switching it off drops the source's content index at the
next start; switching it back on rebuilds it in the background.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | boolean                                 |
| Default     | `true`                                  |

Sources set up through `FILEBRARY_SOURCES` use the default. Content search
also needs [`fulltext.enabled`](#fulltextenabled) and the file-name index for
the source.

```toml
[[sources]]
name = "media"
path = "/mnt/pool/media"
content_search = false
```

### `sources[].s3`

Settings for an S3-compatible source (`kind = "s3"`): AWS S3, MinIO, Garage,
SeaweedFS, Ceph RGW, Cloudflare R2, Backblaze B2, Wasabi. See
[S3-compatible object storage](sources.md#s3-compatible-object-storage).

```toml
[[sources]]
name = "photos"
kind = "s3"

[sources.s3]
bucket = "family-photos"
endpoint = "https://minio.lan:9000"
access_key_id = "AKIA..."
secret_access_key_file = "/run/secrets/s3_secret"
```

### `sources[].s3.bucket`

The bucket name.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | required |

### `sources[].s3.prefix`

A key prefix inside the bucket that acts as the source root, so one bucket can hold several sources. `photos/2024` serves the objects under `photos/2024/`.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | empty: the whole bucket |

### `sources[].s3.endpoint`

The endpoint URL of a non-AWS service, such as `http://minio:9000`. Plain `http://` is accepted only when written here; use it only on a trusted network.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | URL |
| Default     | AWS, from the region |

### `sources[].s3.region`

The region. Most non-AWS services ignore it.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | `"us-east-1"` |

### `sources[].s3.access_key_id`

The access key ID. When unset, credentials come from the standard `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` variables, web identity or instance metadata.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | unset |

### `sources[].s3.secret_access_key`

The secret access key. Prefer [`secret_access_key_file`](#sourcess3secret_access_key_file).

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | unset |

### `sources[].s3.secret_access_key_file`

A file holding the secret access key, such as a Docker or systemd secret. Wins over `secret_access_key`. Surrounding whitespace is ignored.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | path |
| Default     | unset |

### `sources[].s3.session_token`

A session token, for temporary credentials.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | unset |

### `sources[].s3.virtual_hosted_style`

Address the bucket as `bucket.host` instead of `host/bucket`.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | boolean |
| Default     | `false` when `endpoint` is set (MinIO and most others want path style), `true` on AWS |

### `sources[].sftp`

Settings for an SFTP source (`kind = "sftp"`): a directory on any SSH server.
One of [`password`](#sourcessftppassword),
[`password_file`](#sourcessftppassword_file) or
[`private_key`](#sourcessftpprivate_key) is required, and the server's host
key must be pinned with [`host_key_fingerprint`](#sourcessftphost_key_fingerprint)
or [`known_hosts`](#sourcessftpknown_hosts). See [SFTP](sources.md#sftp).

```toml
[[sources]]
name = "nas"
kind = "sftp"

[sources.sftp]
host = "nas.lan"
username = "filebrary"
private_key = "/run/secrets/nas_ed25519"
root = "/volume1/shared"
host_key_fingerprint = "SHA256:7cLm9wR4..."
```

### `sources[].sftp.host`

The server's host name or address.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | required |

### `sources[].sftp.port`

The SSH port.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer |
| Default     | `22` |

### `sources[].sftp.username`

The account to log in as.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | required |

### `sources[].sftp.password`

The account's password. Prefer [`password_file`](#sourcessftppassword_file) or a key.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | unset |

### `sources[].sftp.password_file`

A file holding the password. Wins over `password`. A trailing newline is ignored.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | path |
| Default     | unset |

### `sources[].sftp.private_key`

A private key file in OpenSSH or PKCS#8 format (Ed25519, ECDSA or RSA). Used instead of a password when set.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | path |
| Default     | unset |

### `sources[].sftp.private_key_passphrase`

The passphrase of an encrypted private key.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | unset |

### `sources[].sftp.root`

The directory on the server that acts as the source root. Relative paths start at the account's home directory. Symbolic links that resolve outside it are hidden and refused.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | empty: the home directory |

### `sources[].sftp.host_key_fingerprint`

The SHA-256 fingerprint of the server's host key, as `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` prints it (`SHA256:...`). When the key does not match, the connection is refused and the error names the fingerprint the server presented; check it against the server before pinning it.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | unset |

### `sources[].sftp.known_hosts`

A `known_hosts` file to check the host key against, used when `host_key_fingerprint` is unset.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | path |
| Default     | unset |

### `sources[].sftp.insecure_accept_any_host_key`

Accept any host key. This allows man-in-the-middle attacks; use it only for tests.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | boolean |
| Default     | `false` |

### `sources[].smb`

Settings for an SMB source (`kind = "smb"`): a folder on a Windows, Samba or
NAS share, over SMB 2 or 3 with NTLMv2. See
[SMB](sources.md#smb-windows-shares-samba-nas).

```toml
[[sources]]
name = "office"
kind = "smb"

[sources.smb]
host = "fileserver.lan"
share = "Projects"
username = "filebrary"
password_file = "/run/secrets/smb"
```

### `sources[].smb.host`

The server's host name or address (IPv6 addresses work bare or in brackets).

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | required |

### `sources[].smb.port`

The TCP port.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer |
| Default     | `445` |

### `sources[].smb.share`

The share name (one name, no slashes).

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | required |

### `sources[].smb.root`

A folder inside the share that acts as the source root. `/` and `\\` both separate folders.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | empty: the share root |

### `sources[].smb.username`

The account to log in as. Leave it empty for guest access.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | empty: guest |

### `sources[].smb.password`

The account's password. Prefer [`password_file`](#sourcessmbpassword_file).

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | unset |

### `sources[].smb.password_file`

A file holding the password. Wins over `password`. A trailing newline is ignored.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | path |
| Default     | unset |

### `sources[].smb.domain`

The Windows domain or workgroup.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | empty |

## Content search

The `[fulltext]` table tunes the full-text index, which is stored in
`<data_dir>/fulltext`. Text extraction is pure Rust and runs inside the
server; no external program is started.

### `fulltext.enabled`

Master switch for content search. When off, no content index is opened and
no memory or disk is used for it, whatever the per-source
[`content_search`](#sourcescontent_search) settings say. `content=` searches
then answer `400`.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | boolean                                 |
| Default     | `true`                                  |
| Environment | `FILEBRARY_FULLTEXT`                    |

### `fulltext.max_file_mib`

Files larger than this many MiB are not read for content (their names are
still indexed). One file is read into memory at a time, so this bounds peak
memory while indexing.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer (MiB)                           |
| Default     | `32`                                    |

### `fulltext.max_chars`

At most this many characters of text are indexed per file; the rest of a
very long document is not searchable. Bounds index size and memory.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer                                 |
| Default     | `1000000`                               |

### `fulltext.writer_memory_mib`

Size of the index writer's in-memory buffer, in MiB (at least 15). It is
allocated only while files are being indexed and freed after each commit.
Larger values index big batches a little faster.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer (MiB)                           |
| Default     | `24`                                    |

### `fulltext.commit_every_docs`

New and changed documents become searchable after this many changes, or
[`commit_interval_secs`](#fulltextcommit_interval_secs) after the first
pending change, whichever comes first.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer                                 |
| Default     | `500`                                   |

### `fulltext.commit_interval_secs`

The longest a change waits before it becomes searchable, in seconds.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer (seconds)                       |
| Default     | `5`                                     |

Measured peak memory while indexing is listed under
[Logs, health and metrics](../operations/monitoring.md).
## FTP server

The optional FTP and FTPS server. See [FTP and FTPS](../guide/ftp.md).

### `ftp.enabled`

Runs the FTP server. Off by default.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | boolean |
| Default     | `false` |
| Environment | `FILEBRARY_FTP_ENABLED` |

### `ftp.listen`

Address and port of the FTP control connection.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | socket address |
| Default     | `"0.0.0.0:2121"` |
| Environment | `FILEBRARY_FTP_LISTEN` |

### `ftp.passive_ports`

First and last port of the passive data port range. Publish the whole range when running in Docker.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | array of two port numbers |
| Default     | `[50000, 50100]` |
| Environment | `FILEBRARY_FTP_PASSIVE_PORTS` |

In the environment variable, write the range as `low-high`, for example `50000-50100`.

### `ftp.passive_host`

Address announced to clients in `PASV` replies. Set it to the public address when the server is behind NAT or in Docker.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | IP address |
| Default     | not set (the address the client connected to) |
| Environment | `FILEBRARY_FTP_PASSIVE_HOST` |

### `ftp.tls`

Turns on explicit FTPS (`AUTH TLS`). Without it, passwords travel unencrypted and a warning is logged.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | table |
| Default     | not set |

### `ftp.tls.cert`

PEM file with the certificate chain.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | path |
| Default     | required when `[ftp.tls]` is set |

### `ftp.tls.key`

PEM file with the private key.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | path |
| Default     | required when `[ftp.tls]` is set |

### `ftp.tls.mode`

`required` refuses logins and transfers that do not use TLS; `optional` allows plain FTP too.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | `"required"` or `"optional"` |
| Default     | `"required"` |

### `ftp.max_login_attempts`

Failed logins from one address before it is blocked. FTP logins also count against [`login_max_attempts`](#login_max_attempts).

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer |
| Default     | `5` |

### `ftp.login_block_secs`

How long a blocked address stays blocked, in seconds.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer |
| Default     | `300` |

### `ftp.idle_timeout_secs`

Idle time before an FTP session is closed, in seconds.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer |
| Default     | `600` |

### `ftp.greeting`

One-line greeting sent when a client connects.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | `"Filebrary FTP server ready"` |

## Notifications

Where events are sent. See [Notifications](../guide/notifications.md).

### `notifications.instance`

Server name shown in every notification.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | `"Filebrary"` |

### `notifications.quota_warning_percent`

Storage quota percentage that sends `quota_near_full`, once, when a user's usage crosses it.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer, 1 to 100 |
| Default     | `90` |

### `notifications.retry.attempts`

Delivery attempts per notification, including the first.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer, 1 to 10 |
| Default     | `4` |

### `notifications.retry.initial_backoff_ms`

Wait before the first retry, in milliseconds; doubled after each attempt. Only network errors and `5xx`/`429` answers are retried.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer |
| Default     | `1000` |

### `notifications.retry.timeout_secs`

Timeout of one delivery attempt, in seconds.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | integer |
| Default     | `10` |

### `notifications.webhooks`

Webhook targets: each event is sent as an HTTP `POST` with a JSON body. `FILEBRARY_WEBHOOK_URL` (with optional `FILEBRARY_WEBHOOK_SECRET`) adds one named `env-webhook`.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | array of tables |
| Default     | none |

### `notifications.webhooks[].name`

Unique name across all notification targets, used in logs, metrics and the test endpoint.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | required |

### `notifications.webhooks[].url`

Where to send the `POST`.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | `http` or `https` URL |
| Default     | required |

### `notifications.webhooks[].secret`

Shared secret. When set, requests carry `X-Filebrary-Timestamp` and `X-Filebrary-Signature: sha256=<hex HMAC-SHA256 of "timestamp.body">`.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | not set |

### `notifications.webhooks[].headers`

Extra request headers, for example `Authorization`.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | table of strings |
| Default     | none |

### `notifications.webhooks[].events`

Events to send: `upload`, `share_opened`, `quota_near_full`, `job_failed`. Empty means every event.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | array of strings |
| Default     | `[]` (every event) |

### `notifications.ntfy`

[ntfy](https://ntfy.sh) targets. `FILEBRARY_NTFY_TOPIC` (with optional `FILEBRARY_NTFY_SERVER` and `FILEBRARY_NTFY_TOKEN`) adds one named `env-ntfy`.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | array of tables |
| Default     | none |

### `notifications.ntfy[].name`

Unique name across all notification targets.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | required |

### `notifications.ntfy[].server`

The ntfy server.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | `http` or `https` URL |
| Default     | `"https://ntfy.sh"` |

### `notifications.ntfy[].topic`

The topic to publish to.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string of letters, digits, `-` and `_` |
| Default     | required |

### `notifications.ntfy[].token`

Access token, sent as a bearer token.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | not set |

### `notifications.ntfy[].username`

User name for basic authentication, instead of a token.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | not set |

### `notifications.ntfy[].password`

Password for basic authentication.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | not set |

### `notifications.ntfy[].click`

URL opened when the notification is tapped.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | URL |
| Default     | not set |

### `notifications.ntfy[].events`

Events to send. Empty means every event.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | array of strings |
| Default     | `[]` (every event) |

### `notifications.email`

SMTP email targets.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | array of tables |
| Default     | none |

### `notifications.email[].name`

Unique name across all notification targets.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | required |

### `notifications.email[].smtp_host`

SMTP server host name.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | required |

### `notifications.email[].smtp_port`

SMTP port.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | port number |
| Default     | `465` for `tls`, `587` for `starttls`, `25` for `none` |

### `notifications.email[].security`

`tls` (TLS from the start), `starttls` (must upgrade) or `none` (only for a relay on the same host or network).

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | `"starttls"` |

### `notifications.email[].username`

SMTP user name, if the server needs authentication.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | not set |

### `notifications.email[].password`

SMTP password.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string |
| Default     | not set |

### `notifications.email[].from`

Sender address, for example `Filebrary <files@example.com>`.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | email address |
| Default     | required |

### `notifications.email[].to`

Recipients.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | array of email addresses |
| Default     | required |

### `notifications.email[].events`

Events to send. Empty means every event.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | array of strings |
| Default     | `[]` (every event) |

## Metrics

The Prometheus endpoint. See [Logs, health and metrics](../operations/monitoring.md#metrics).

### `metrics.enabled`

Serves `/metrics`. When off, it answers `404`.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | boolean |
| Default     | `true` |
| Environment | `FILEBRARY_METRICS_ENABLED` |

### `metrics.token`

Bearer token scrapers must send (`Authorization: Bearer <token>`). Without it, `/metrics` is open to anyone who can reach the server.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string, at least 16 characters |
| Default     | not set |
| Environment | `FILEBRARY_METRICS_TOKEN` |
