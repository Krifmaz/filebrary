# Sharing

Anyone with the [**share** permission](../configuration/users.md#permissions) can create a link to a file or folder
they can download. The person receiving the link needs no account.

Share links are built from [`public_url`](../configuration/reference.md#public_url),
so set it when Filebrary is behind a proxy, or the links will point at the
wrong address.

## Options

| Option | Effect |
|---|---|
| **Password** | Visitors must enter it before seeing anything. It is stored hashed. |
| **Expiry date** | The link stops working after this date and time. |
| **Download limit** | The link stops working after this many downloads. |
| **Upload only (drop link)** | Visitors can upload into the folder but cannot see or download what is in it. Each file is limited to [`max_drop_upload_mib`](../configuration/reference.md#max_drop_upload_mib). |

A link never gives more access than its creator has. If the creator loses
access to the item, or their account is disabled or deleted, the link stops
working.

## Managing links

**Shares** in the sidebar lists your links, with their options and how many
times each was opened. You can change or delete a link at any time; deleting
it takes effect immediately. Administrators can see and remove every link.

## Collections

A collection groups files and folders from any source, so you can share
them with one link. Select items and choose **Add to collection**; the
**Collections** page lists your collections and creates a share link for a
whole collection, with the usual password, expiry and download limit. The
link only serves items its owner may still share. API: `/api/collections`.
