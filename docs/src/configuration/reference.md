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

How long a login stays valid without activity, in hours. Using the session
extends it: once less than half of this time is left, the next request
renews it to the full length.

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

Password for that first administrator. If unset, a random password is
generated and printed once to the log. Prefer the environment variable (or a
secret manager) to writing a password into the file, and change it after the
first sign-in.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | string, optional                        |
| Default     | not set: random, logged once            |
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
