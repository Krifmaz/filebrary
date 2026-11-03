-- File comments (Phase 4). Owned by the comments/collections/themes work stream.
--
-- A comment is attached to a real path inside a source (not the path a scoped
-- user sees), so users with different scopes see the same thread. When a file
-- is moved or deleted the files code calls `comments::on_rename` /
-- `comments::on_delete` to keep threads attached or clean them up.

CREATE TABLE comments (
    id         INTEGER PRIMARY KEY,
    source     TEXT    NOT NULL,
    path       TEXT    NOT NULL,
    user_id    INTEGER REFERENCES users(id) ON DELETE SET NULL,
    body       TEXT    NOT NULL,
    created_at INTEGER NOT NULL,
    edited_at  INTEGER
);
CREATE INDEX comments_target ON comments(source, path, created_at);
