# Reverse proxy

Filebrary works behind any reverse proxy. Set `FILEBRARY_PUBLIC_URL` to the
public URL so share links and cookies are correct, set
`FILEBRARY_TRUST_PROXY_HEADERS=true` so login rate limiting sees real client
addresses, and allow large request bodies for uploads
(uploads are chunked, so a limit of a few hundred megabytes is plenty).

## Caddy

```
files.example.com {
    reverse_proxy localhost:8080
}
```

## nginx

```nginx
server {
    listen 443 ssl;
    server_name files.example.com;

    client_max_body_size 512m;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_request_buffering off;
    }
}
```

## Traefik (Docker labels)

```yaml
services:
  filebrary:
    image: ghcr.io/krifmaz/filebrary:latest
    environment:
      FILEBRARY_PUBLIC_URL: https://files.example.com
      FILEBRARY_TRUST_PROXY_HEADERS: "true"
    labels:
      - traefik.enable=true
      - traefik.http.routers.filebrary.rule=Host(`files.example.com`)
      - traefik.http.routers.filebrary.entrypoints=websecure
      - traefik.http.routers.filebrary.tls.certresolver=letsencrypt
      - traefik.http.services.filebrary.loadbalancer.server.port=8080
```

## Checklist

* **Body size:** the proxy must accept request bodies at least as large as
  [`max_chunk_mib`](reference.md#max_chunk_mib) (64 MiB by default). With
  nginx, also turn off request buffering so uploads stream through.
* **Timeouts:** large downloads and zip downloads can run for a long time.
  Raise the proxy's read/send timeouts (nginx `proxy_read_timeout`,
  `proxy_send_timeout`) if they are cut off.
* **`public_url`:** must be the address users type, with `https://`.
* **`trust_proxy_headers`:** turn on only if the proxy is the *only* way to
  reach Filebrary. Bind Filebrary to `127.0.0.1` or a private Docker network
  so nobody can bypass the proxy and forge `X-Forwarded-For`.
* **Sub-path:** serving Filebrary under a path (e.g.
  `https://example.com/files/`) is not supported yet; use a sub-domain.
