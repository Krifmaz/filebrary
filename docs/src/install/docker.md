# Docker

Images are published to the GitHub Container Registry for `linux/amd64` and
`linux/arm64`.

| Tag | Contents | Size |
| --- | --- | --- |
| `ghcr.io/krifmaz/filebrary:latest` | Filebrary only, distroless runtime | under 60 MB |
| `ghcr.io/krifmaz/filebrary:<version>` | A specific release, e.g. `0.1.0` | under 60 MB |
| `ghcr.io/krifmaz/filebrary:latest-full` | Adds ffmpeg and LibreOffice for previews and conversions | larger |
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

## Building the image yourself

```sh
docker build -t filebrary .                    # small image
docker build --target full -t filebrary:full . # with ffmpeg and LibreOffice
```
