# Docker

Images are published to the GitHub Container Registry for `linux/amd64` and
`linux/arm64`.

| Tag | Contents | Size |
| --- | --- | --- |
| `ghcr.io/krifmaz/filebrary:latest` | Filebrary only, distroless runtime | under 60 MB |
| `ghcr.io/krifmaz/filebrary:<version>` | A specific release, e.g. `0.1.0` | under 60 MB |
| `ghcr.io/krifmaz/filebrary:latest-full` | Adds ffmpeg, LibreOffice, poppler and libheif for previews and conversions | larger |
| `ghcr.io/krifmaz/filebrary:<version>-full` | A specific release with conversion tools | larger |

## Quick start

```sh
docker run -d --name filebrary \
  -p 8080:8080 \
  -v filebrary-data:/data \
  -v /path/to/your/files:/srv \
  ghcr.io/krifmaz/filebrary:latest
```

Open <http://localhost:8080>. Instructions for the first admin account are
printed to the container log on first start:

```sh
docker logs filebrary
```

The image sets `FILEBRARY_LISTEN=0.0.0.0:8080`, `FILEBRARY_DATA_DIR=/data`
and `FILEBRARY_SOURCES=files=/srv`. Override any of them with `-e`, for
example `-e FILEBRARY_SOURCES="files=/srv;media=/mnt/media"` together with a
second `-v` mount. See the [configuration overview](../configuration/overview.md).

## Volumes

| Path | Purpose |
| --- | --- |
| `/data` | Database, configuration and caches. Back this up. |
| `/srv` | The default local storage source. Mount your files here. |

The container runs as the unprivileged user `65532:65532`. Make sure that
user can read (and, for uploads, write) the folders you mount, or run with
`--user <uid>:<gid>` matching the owner of your files.

## Docker Compose

```yaml
services:
  filebrary:
    image: ghcr.io/krifmaz/filebrary:latest
    container_name: filebrary
    restart: unless-stopped
    ports:
      - "8080:8080"
    volumes:
      - ./data:/data
      - /path/to/your/files:/srv
    environment:
      FILEBRARY_LOG: info
      FILEBRARY_PUBLIC_URL: https://files.example.com
```

## Health check

The image declares a Docker `HEALTHCHECK` that runs `filebrary healthcheck`,
which calls the `/healthz` endpoint on the local server. `docker ps` shows the
status, and orchestrators can use `GET /healthz` directly.

## Troubleshooting

### `unable to open database file` or `cannot write to /data`

The container cannot write to its data folder. This almost always means the
host folder mounted at `/data` belongs to another user. Docker creates a
missing bind-mount folder as `root`, while Filebrary runs as `65532:65532` (or
the `user:` you set). On start-up Filebrary checks the folder and prints the
path, the user it runs as, the folder's owner and the command to fix it:

```text
Error: cannot write to /data (Permission denied (os error 13)). Filebrary runs as uid 1000, gid 1000 and needs to write to its data directory /data. It is owned by 0:0.
Fix: make the folder writable by that user, for example

    sudo chown -R 1000:1000 <folder>
```

Fix it on the host, using the folder from your `volumes:` entry and the ids
from the message (or from the `user:` line), then restart:

```sh
sudo mkdir -p /opt/filebrary
sudo chown -R 1000:1000 /opt/filebrary
docker restart filebrary
docker logs --tail 50 filebrary
```

Creating and owning the folder before the first `docker compose up` avoids the
problem altogether.

### `unable to find group` when the stack starts

Docker could not resolve the group in a `user:` value. Use numbers on both
sides (`user: 1000:1000`), and remember that Portainer stacks do not read a
`.env` file: a `${PGID}` that is not set under the stack's environment
variables becomes empty.

## Building the image yourself

```sh
docker build -t filebrary .                    # small image
docker build --target full -t filebrary:full . # with ffmpeg and LibreOffice
```
