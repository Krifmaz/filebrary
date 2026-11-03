# Storage sources

A source is a storage location Filebrary serves. Each source has its own
name, root and permissions, and you can configure several at once.

## Local folders

Any folder on the host, including FUSE and mergerfs pools.

```toml
[[sources]]
name = "files"
kind = "local"
path = "/srv"
```

Every path a user requests is normalised and checked against the source
root. Symbolic links that point outside the root are refused.

S3-compatible, SFTP and SMB sources arrive in a later release.
