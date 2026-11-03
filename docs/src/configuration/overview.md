# Configuration overview

Filebrary reads its settings, in order of precedence (highest first), from:

1. Environment variables prefixed with `FILEBRARY_`.
2. A TOML configuration file, given with `--config <file>` or
   `FILEBRARY_CONFIG`.
3. Built-in defaults.

```toml
listen = "0.0.0.0:8080"
data_dir = "/data"
public_url = "https://files.example.com"

[[sources]]
name = "files"
path = "/srv"
```

## Core settings

| TOML key | Environment | Default | Description |
| --- | --- | --- | --- |
| `listen` | `FILEBRARY_LISTEN` | `127.0.0.1:8080` (`0.0.0.0:8080` in Docker) | Address and port to listen on. |
| `data_dir` | `FILEBRARY_DATA_DIR` | `./data` (`/data` in Docker) | Database, upload staging area and secret key. |
| `sources` | `FILEBRARY_SOURCES` | `files=<data_dir>/files` (`files=/srv` in Docker) | Storage sources. The variable takes `name=path` pairs separated by `;`. |
| `public_url` | `FILEBRARY_PUBLIC_URL` | none | The URL users reach the server at. Used in share links and for cookie security. |
| `secure_cookies` | `FILEBRARY_SECURE_COOKIES` | on when `public_url` is `https://` | Force the `Secure` cookie flag on or off. |
| `trust_proxy_headers` | `FILEBRARY_TRUST_PROXY_HEADERS` | `false` | Trust `X-Forwarded-For`. Enable only behind a proxy you control. |
| `session_ttl_hours` | `FILEBRARY_SESSION_TTL_HOURS` | `168` | How long a login lasts without activity. |
| `login_max_attempts` | (none) | `5` | Failed logins allowed per username and per client address within `login_window_secs`. |
| `login_window_secs` | (none) | `300` | Window for login rate limiting, in seconds. |
| `admin_username` | `FILEBRARY_ADMIN_USERNAME` | `admin` | Name of the administrator created on first start, when there are no users. |
| `admin_password` | `FILEBRARY_ADMIN_PASSWORD` | generated | Password for that administrator. If unset, a random one is printed once to the log. Prefer the variable over the file. |
| `log_format` | `FILEBRARY_LOG_FORMAT` | `text` | `text`, or `json` for structured logs. |
| `trash_retention_days` | `FILEBRARY_TRASH_RETENTION_DAYS` | `30` | Days a deleted item stays in the trash before it is purged. `0` turns the trash off: deletes are permanent. See [Trash, tags, stars and activity](../organize.md). |
| `activity_retention_days` | `FILEBRARY_ACTIVITY_RETENTION_DAYS` | `365` | Days activity-log events are kept. `0` keeps them forever. |
| (none) | `FILEBRARY_LOG` | `info` | Log filter, e.g. `debug` or `info,sqlx=warn`. |

Settings for later features are documented here as they land.

## Health check

`GET /healthz` returns 200 when the server is up. `filebrary healthcheck`
probes it on the configured `listen` address and exits 0 when healthy, which
is what the Docker image's `HEALTHCHECK` runs.
