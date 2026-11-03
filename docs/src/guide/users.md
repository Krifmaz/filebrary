# Users, groups and permissions

Filebrary has its own user accounts, stored in the SQLite database in the
[data directory](../configuration/reference.md#data_dir). Accounts can also
come from [single sign-on or LDAP](authentication.md).

## Administrators and users

* **Administrators** manage users, groups, sources and settings, and have
  every permission on every source.
* **Users** see only the sources and folders they have been given access
  to. A source they cannot view does not appear for them at all.

An account can be disabled without deleting it; a disabled user cannot sign
in, and their existing sessions stop working.

## Permissions

Permissions are granted per source, and can be narrowed or widened for a
folder inside it. A grant on a folder applies to everything below it.

| Permission | Allows |
|---|---|
| **view** | List folders and see file names, sizes and dates. |
| **download** | Open and download files, including zip downloads of a selection. |
| **upload** | Create files and folders. |
| **edit** | Rename, move and change existing files (including in the text editor). |
| **delete** | Delete files and folders (to the [trash](../organize.md#trash-bin)). |
| **share** | Create [share links](sharing.md) for what the user can see. |

A user can only share what they can download, and a share link never grants
more than its creator has.

## Groups

Put users in groups and grant permissions to the group instead of to each
user. A user's permissions are the union of their own grants and those of
every group they belong to.

## Storage quotas

> **Planned for Phase 3.** See [Feature status](../status.md).

Each user can have a storage quota. Uploads that would exceed it are
refused, and a notification can be sent when a user is close to it.
