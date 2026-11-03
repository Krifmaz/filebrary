# Upgrading and backups

## Backups

Everything Filebrary owns is in the data directory (`/data` in Docker):
the SQLite database, the configuration file and caches. Your files stay in
their sources and are never moved into the data directory.

To back up while running, use SQLite's online backup so the copy is
consistent:

```sh
sqlite3 /data/filebrary.db ".backup '/backup/filebrary.db'"
```

Or stop Filebrary and copy the whole data directory.

## Upgrading

Docker:

```sh
docker pull ghcr.io/krifmaz/filebrary:latest
docker stop filebrary && docker rm filebrary
# then run the same `docker run` command as before
```

Binary: replace the binary and restart the service. Database migrations run
automatically on start. Take a backup first.
