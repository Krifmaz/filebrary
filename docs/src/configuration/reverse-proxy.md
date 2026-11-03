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
