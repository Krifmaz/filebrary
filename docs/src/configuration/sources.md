# Storage sources

A source is a storage location Filebrary serves. Each source has its own
name, root and permissions, and you can configure several at once. Each one
appears as a top-level entry in the sidebar.

```toml
[[sources]]
name = "files"
path = "/srv/files"

[[sources]]
name = "media"
kind = "local"
path = "/mnt/pool/media"
```

Or, as an environment variable (local sources only):

```sh
FILEBRARY_SOURCES="files=/srv/files;media=/mnt/pool/media"
```

The keys are described in the [settings reference](reference.md#sources).
With no sources configured, Filebrary serves `<data_dir>/files` as `files`.

## Choosing names

The name (`a-z`, `0-9`, `-`, `_`, up to 32 characters) appears in URLs,
share links and permission entries. Pick it once: renaming a source later
breaks its share links and permissions.

## Local folders

Any folder on the host, including FUSE file systems such as **mergerfs**
pools, **rclone mount** and **sshfs**. The folder is created on start-up if
it does not exist.

Filebrary needs read permission to browse and write permission to upload,
rename and delete. In Docker, mount the folder into the container and point
the source at the path *inside* the container:

```sh
docker run -v /mnt/pool/media:/media \
  -e FILEBRARY_SOURCES="files=/srv;media=/media" ...
```

For FUSE mounts used from Docker, mount them on the host with
`allow_other` so the container's user can see them.

## Path safety

Users can never reach anything outside a source's root:

* URL encoding is decoded exactly once, then every path is normalised;
* `..` components are rejected outright (never "resolved"), as are
  backslashes, NUL bytes and over-long names, and on Windows drive
  prefixes, alternate data streams and reserved names such as `CON`;
* a doubly encoded `%2e%2e` is treated as a literal file name, not as `..`;
* the parent of every path is resolved on disk and checked to be inside the
  root, and symbolic links that point outside the root are refused (links
  inside the root work);
* no file operation goes through a shell.

## S3, SFTP and SMB

> **Planned for Phase 3.** See [Feature status](../status.md).

S3-compatible object storage (AWS S3, MinIO, Backblaze B2, Cloudflare R2,
Wasabi, Garage), SFTP servers and SMB / Windows shares will be available as
source kinds, each with the same permissions as local folders. Their keys
will be added to the [settings reference](reference.md).
