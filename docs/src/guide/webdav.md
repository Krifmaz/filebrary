# WebDAV

> **Planned for Phase 2.** See [Feature status](../status.md).

Filebrary includes a WebDAV server (class 2, with locking), so you can mount
your sources as a network drive. It uses the same accounts and permissions
as the web UI.

Tested clients:

* **Windows Explorer**: *This PC → Map network drive*, address
  `https://files.example.com/dav/`. Windows only allows Basic
  authentication over HTTPS.
* **macOS Finder**: *Go → Connect to Server*, address
  `https://files.example.com/dav/`.
* **rclone**: `rclone config`, type `webdav`, vendor `other`.
* Linux file managers (GNOME Files, Dolphin): `davs://files.example.com/dav/`.

The exact path and any settings will be listed here and in the
[settings reference](../configuration/reference.md) when WebDAV ships.
