# Notifications

Upload, share opened, quota near full and job failed events can be sent to
webhooks, [ntfy](https://ntfy.sh) and email. Targets are set in the config
file only; administrators can list them and send a test from
`GET /api/admin/notifications` and `POST /api/admin/notifications/{name}/test`.

```toml
[notifications]
instance = "Home NAS"          # shown in every notification
quota_warning_percent = 90

[[notifications.webhooks]]
name = "automation"
url = "https://hooks.example.com/filebrary"
secret = "shared secret"       # adds X-Filebrary-Timestamp and X-Filebrary-Signature
events = ["upload", "job_failed"]  # empty or missing: every event

[[notifications.ntfy]]
name = "phone"
server = "https://ntfy.sh"
topic = "my-filebrary"
token = "tk_..."

[[notifications.email]]
name = "admin-mail"
smtp_host = "smtp.example.com"
security = "starttls"          # tls | starttls | none
username = "files@example.com"
password = "..."
from = "Filebrary <files@example.com>"
to = ["admin@example.com"]
events = ["quota_near_full", "job_failed"]
```

Webhook bodies are JSON: `{"at": "...", "instance": "...", "type": "upload",
"user": "...", "source": "...", "path": "...", "size": 123}`. With a
`secret`, check `X-Filebrary-Signature == "sha256=" + hex(HMAC-SHA256(secret,
timestamp + "." + body))` and reject old `X-Filebrary-Timestamp` values.
Failed deliveries are retried with backoff (`[notifications.retry]`:
`attempts`, `initial_backoff_ms`, `timeout_secs`) and counted in
`filebrary_notifications_failed_total`. Quick setup by environment:
`FILEBRARY_WEBHOOK_URL`, `FILEBRARY_WEBHOOK_SECRET`, `FILEBRARY_NTFY_TOPIC`,
`FILEBRARY_NTFY_SERVER`, `FILEBRARY_NTFY_TOKEN`.

## Events

| Event | When | Fields |
| --- | --- | --- |
| `upload` | A file finished uploading (web, FTP, drop link; drop-link uploads name the link owner as `user`) | `user`, `source`, `path`, `size` |
| `share_opened` | Someone opened a share link (its top level, after the password if it has one; browsing deeper is not reported again) | `share_id`, `owner`, `source`, `path`, `remote_addr` |
| `quota_near_full` | A user's usage crossed `quota_warning_percent` | `user`, `used_bytes`, `quota_bytes`, `threshold_percent` |
| `job_failed` | A background job failed after its last retry | `job_id`, `kind`, `user`, `error` |
| `test` | An administrator pressed "send test" | `message` |

Every body also carries `at` (RFC 3339) and `instance`.

## Settings

| TOML key | Default | Description |
| --- | --- | --- |
| `notifications.instance` | `Filebrary` | Server name shown in every notification. |
| `notifications.quota_warning_percent` | `90` | Quota percentage that triggers `quota_near_full` (once, when crossed). |
| `notifications.retry.attempts` | `4` | Attempts per notification, including the first (1-10). |
| `notifications.retry.initial_backoff_ms` | `1000` | Wait before the first retry; doubled each time. |
| `notifications.retry.timeout_secs` | `10` | Timeout of one attempt. |
| `notifications.webhooks[]` | none | `name`, `url`, optional `secret`, `headers`, `events`. |
| `notifications.ntfy[]` | none | `name`, `server` (`https://ntfy.sh`), `topic`, optional `token` or `username`+`password`, `click`, `events`. |
| `notifications.email[]` | none | `name`, `smtp_host`, `smtp_port` (465/587/25 by `security`), `security`, optional `username`+`password`, `from`, `to`, `events`. |

Target names must be unique across all kinds. Secrets are never logged or
returned by the API. TLS uses rustls with the ring provider; no OpenSSL is
needed.
