# Upgrading and backups

## What to back up

Everything Filebrary owns is in the [data directory](configuration/reference.md#data_dir)
(`/data` in Docker). Your files stay in their sources and are never moved
into it.

| File | Contents | Back up? |
|---|---|---|
| `filebrary.db` (+ `-wal`, `-shm`) | Users, groups, permissions, share links, sessions; tags, comments, collections, branding, activity log; later the search index | **Yes** |
| `secret.key` | Key that signs sessions and share tokens. If lost, everyone is signed out. Keep it secret. | **Yes** |
| Upload staging area | Chunks of unfinished uploads | No |
| Caches (thumbnails, previews, once available) | Rebuilt on demand | No |

Also back up your configuration file, if you use one.

## Taking a backup

While Filebrary runs, use SQLite's online backup so the copy is consistent
(copying `filebrary.db` alone while it is in use can give a broken copy,
because recent changes may still be in the `-wal` file):

```sh
sqlite3 /data/filebrary.db ".backup '/backup/filebrary.db'"
cp /data/secret.key /backup/
```

The Docker image has no `sqlite3`; run it from the host against the volume,
or stop the container first. Stopping Filebrary and copying the whole data
directory is always safe.

## Restoring

Stop Filebrary, put `filebrary.db` and `secret.key` back in the data
directory (delete any leftover `filebrary.db-wal` and `filebrary.db-shm`),
and start it again.

## Upgrading

Database migrations run automatically on start-up, and are not reversible:
**take a backup first**, so you can go back to the old version with the old
database if needed. Read the release notes for anything that needs action.

Docker:

```sh
docker pull ghcr.io/krifmaz/filebrary:latest
docker stop filebrary && docker rm filebrary
# then run the same `docker run` command as before
```

With Docker Compose: `docker compose pull && docker compose up -d`.

Pin a version tag such as `0.1` (minor updates) or `0.1.0` (exact) instead
of `latest` if you want to choose when to upgrade.

Binary: replace the binary and restart the service
(`sudo systemctl restart filebrary`).
