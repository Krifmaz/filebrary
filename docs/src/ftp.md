# FTP and FTPS

An FTP server is built in and **off by default**. It uses the same users,
passwords and permissions as the web UI: the root lists one folder per
source the user can see, and each user stays inside their folder.

```toml
[ftp]
enabled = true
listen = "0.0.0.0:2121"
passive_ports = [50000, 50100]   # publish these too in Docker
passive_host = "203.0.113.7"     # address clients connect back to (NAT/Docker)

[ftp.tls]                         # explicit FTPS (AUTH TLS)
cert = "/data/tls/cert.pem"
key = "/data/tls/key.pem"
mode = "required"                 # or "optional"
```

Without `[ftp.tls]` passwords travel in clear text, and a warning is logged.
Failed logins block the client address after `max_login_attempts` (5) for
`login_block_secs` (300). Environment: `FILEBRARY_FTP_ENABLED`,
`FILEBRARY_FTP_LISTEN`, `FILEBRARY_FTP_PASSIVE_PORTS` (`50000-50100`),
`FILEBRARY_FTP_PASSIVE_HOST`. Permission changes apply from the next login.

## Settings

| TOML key | Environment | Default | Description |
| --- | --- | --- | --- |
| `ftp.enabled` | `FILEBRARY_FTP_ENABLED` | `false` | Run the FTP server. |
| `ftp.listen` | `FILEBRARY_FTP_LISTEN` | `0.0.0.0:2121` | Control connection address. |
| `ftp.passive_ports` | `FILEBRARY_FTP_PASSIVE_PORTS` | `[50000, 50100]` | Passive data port range (env: `low-high`). |
| `ftp.passive_host` | `FILEBRARY_FTP_PASSIVE_HOST` | the connected address | Address announced in `PASV` replies. |
| `ftp.tls.cert`, `ftp.tls.key` | none | none | PEM certificate chain and key for explicit FTPS. |
| `ftp.tls.mode` | none | `required` | `required` or `optional`. |
| `ftp.max_login_attempts` | none | `5` | Failed logins from one address before it is blocked. |
| `ftp.login_block_secs` | none | `300` | How long a blocked address stays blocked. |
| `ftp.idle_timeout_secs` | none | `600` | Idle session timeout. |
| `ftp.greeting` | none | `Filebrary FTP server ready` | One-line greeting. |

## Commands and permissions

| Command | Permission |
| --- | --- |
| `LIST`, `NLST`, `MLSD`, `SIZE`, `MDTM`, `CWD` | view |
| `RETR` (with `REST` to resume) | download |
| `STOR` of a new file, `MKD` | upload |
| `STOR` over an existing file, resumed `STOR`, `RNFR`/`RNTO` | edit |
| `DELE`, `RMD` (empty folders only) | delete |

Deletes go to the trash bin like web deletes (permanent when the trash is
off), renames keep tags and stars, and every change is written to the
activity log with `"via": "ftp"`. The trash folder is never listed.
Moving between sources and changing the top-level folders is refused.
`SITE MD5` is disabled. Paths are normalised on the virtual tree first, so
`..` never leaves a source, and symlinks that point outside the user's folder
are refused by the storage layer.

## Docker

```sh
docker run -p 8080:8080 -p 2121:2121 -p 50000-50100:50000-50100 \
  -e FILEBRARY_FTP_ENABLED=true -e FILEBRARY_FTP_PASSIVE_HOST=203.0.113.7 \
  -v /srv:/srv -v filebrary-data:/data ghcr.io/krifmaz/filebrary:latest
```
