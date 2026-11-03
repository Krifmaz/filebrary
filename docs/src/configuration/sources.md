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

Remote sources take their settings from a sub-table named after their kind
(`[sources.s3]`, `[sources.sftp]` or `[sources.smb]`) and leave `path` unset;
every key is in the [settings reference](reference.md#sourcess3). They are
checked at start-up but connect on first use, so a server that is
down does not stop Filebrary or the other sources from starting; requests to
it fail until it is back, and the connection is re-made automatically.

Every secret can be given inline or, better, read from a file (`*_file`
settings), such as a Docker or systemd secret. All three clients are written
in Rust and built into the binary: no `ssh`, `smbclient` or other program is
run, and nothing goes through a shell.

### S3-compatible object storage

AWS S3 and anything that speaks its API: MinIO, Garage, SeaweedFS, Ceph RGW,
Cloudflare R2, Backblaze B2, Wasabi.

```toml
[[sources]]
name = "photos"
kind = "s3"

[sources.s3]
bucket = "family-photos"
prefix = "library"                  # optional: serve only this part of the bucket
endpoint = "https://minio.lan:9000" # omit for AWS
region = "us-east-1"
access_key_id = "AKIA..."
secret_access_key_file = "/run/secrets/s3_secret"
```


How files map onto objects:

- `/a/b.txt` is the object `<prefix>/a/b.txt`, with the name unencoded, so
  objects uploaded by other tools appear under their real names.
- Folders are key prefixes. An empty folder made in Filebrary is kept alive by
  a zero-byte `.filebrary-folder` object inside it, which is hidden. Folder
  objects made by other tools (keys ending in `/`) work too.
- S3 has no rename, so moving or renaming a folder copies every object on the
  server and then deletes the originals. A large folder takes a while, and an
  interruption leaves both copies (nothing is lost).
- Large uploads use multipart upload in 8 MiB parts, so memory use stays flat.

The bucket policy needs `s3:ListBucket`, `s3:GetObject`, `s3:PutObject` and
`s3:DeleteObject` on the prefix (and the multipart actions, which `PutObject`
covers on AWS).

### SFTP

A directory on any SSH server.

```toml
[[sources]]
name = "nas"
kind = "sftp"

[sources.sftp]
host = "nas.lan"
port = 22
username = "filebrary"
private_key = "/run/secrets/nas_ed25519"
root = "/volume1/shared"
host_key_fingerprint = "SHA256:7cLm9wR4..."
```


One of `password`, `password_file` or `private_key` is required, and the host
key must be pinned with `host_key_fingerprint` or `known_hosts`. To get the
fingerprint, run `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` on the
server, or start Filebrary with a wrong value: the error names the fingerprint
the server presented, which you should compare with the server's before
pasting it in.

Symbolic links on the server follow the same rules as local sources: links
that resolve outside `root` are hidden and refused, and deleting a link
removes the link, never its target. Writes go to a hidden temporary file that
is renamed into place. SFTP cannot rename over a file, so overwriting removes
the old file just before the rename.

### SMB (Windows shares, Samba, NAS)

A folder on an SMB 2 or 3 share.

```toml
[[sources]]
name = "office"
kind = "smb"

[sources.smb]
host = "fileserver.lan"
share = "Projects"
root = "Shared/2024"     # optional folder inside the share
username = "filebrary"
password_file = "/run/secrets/smb"
domain = "CORP"          # optional
```


Authentication is NTLMv2. Signing and encryption follow what the server asks
for (SMB 3 encrypts when the share requires it). SMB 1 is not supported.

Junctions and symbolic links that the server reports (reparse points) are
hidden, cannot be passed through, and are deleted as links. Samba resolves
Unix symlinks on its side instead; its default `wide links = no` refuses the
ones that leave the share. If a share holds links that point elsewhere in the
same share and you serve only part of it with `root`, set
`follow symlinks = no` on that share.

Writes go to a hidden temporary file that is renamed over the target in one
step. Copies within a source use server-side copy when the server supports it.

### Testing the backends

`scripts/test-backends.sh` starts MinIO, an OpenSSH SFTP server and Samba
(the last two in Docker), runs the backend test suite against all of them,
and stops them again. CI runs the same script.
