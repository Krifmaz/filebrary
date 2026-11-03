-- Core tables owned by the files/storage work stream. Times are Unix seconds (UTC).
--
-- Migration numbering (one directory for the whole workspace, so sqlx keeps a
-- single history):
--   0001-0009 core (uploads, ...)
--   0010-0019 auth (users, groups, sessions, permissions)
--   0020-0029 shares
--   0030+     later phases

-- Resumable uploads in progress. Data is staged in <data_dir>/uploads/<id>.
CREATE TABLE uploads (
    id         TEXT    PRIMARY KEY,
    user_id    INTEGER NOT NULL,
    source     TEXT    NOT NULL,
    path       TEXT    NOT NULL,
    size       INTEGER NOT NULL,
    received   INTEGER NOT NULL DEFAULT 0,
    overwrite  INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX uploads_updated ON uploads(updated_at);
