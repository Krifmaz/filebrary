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

Failed logins allowed from one client address within
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
| Type        | `"local"`                               |
| Default     | `"local"`                               |

`local` serves a directory on the host, including FUSE file systems such as
mergerfs pools. S3-compatible, SFTP and SMB backends are planned; see
[Feature status](../status.md).

### `sources[].path`

For `local` sources, the root directory. Users can never reach anything
outside it: paths are normalised and checked, and symbolic links that point
outside the root are refused.

|             |                                         |
|-------------|-----------------------------------------|
| Type        | path                                    |
| Default     | required                                |

The directory is created at start-up if it does not exist. It must be
readable (and writable, for uploads) by the user Filebrary runs as. In Docker
this is the path *inside* the container.
