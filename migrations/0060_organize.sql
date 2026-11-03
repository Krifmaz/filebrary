-- Trash bin, tags, stars and the activity log (Phase 2, "organize" work stream).
-- Times are Unix seconds (UTC). `source` is a source name from the config and
-- `path` is the real path inside it (the user's scope already applied), in
-- VPath display form: "/a/b.txt".

-- Items moved to a source's trash folder ("/.filebrary-trash/<id>").
CREATE TABLE trash (
    id            TEXT    PRIMARY KEY,           -- random id, also the name in the trash folder
    source        TEXT    NOT NULL,
    original_path TEXT    NOT NULL,              -- where it was, for restore
    kind          TEXT    NOT NULL,              -- 'file' or 'dir'
    size          INTEGER NOT NULL DEFAULT 0,    -- bytes (files only)
    deleted_by    INTEGER REFERENCES users(id) ON DELETE SET NULL,
    deleted_at    INTEGER NOT NULL
);
CREATE INDEX trash_source ON trash(source, deleted_at);
CREATE INDEX trash_deleted_at ON trash(deleted_at);

-- Tag names are global: everyone who can view a file sees its tags.
CREATE TABLE tags (
    id         INTEGER PRIMARY KEY,
    name       TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    color      TEXT,                              -- "#rrggbb" or NULL
    created_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE file_tags (
    tag_id   INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    source   TEXT    NOT NULL,
    path     TEXT    NOT NULL,
    added_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    added_at INTEGER NOT NULL,
    PRIMARY KEY (tag_id, source, path)
);
CREATE INDEX file_tags_path ON file_tags(source, path);

-- Stars (pins) are personal.
CREATE TABLE stars (
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    source     TEXT    NOT NULL,
    path       TEXT    NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, source, path)
);
CREATE INDEX stars_path ON stars(source, path);

-- Who did what, and when. Rows outlive users (username is copied).
CREATE TABLE activity (
    id       INTEGER PRIMARY KEY,
    at       INTEGER NOT NULL,
    user_id  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    username TEXT,                                -- NULL for anonymous (share links)
    action   TEXT    NOT NULL,                    -- e.g. 'upload', 'trash', 'share.open'
    source   TEXT,
    path     TEXT,
    target   TEXT,                                -- destination of rename/move/copy
    details  TEXT,                                -- small JSON object, optional
    ip       TEXT
);
CREATE INDEX activity_at ON activity(at);
CREATE INDEX activity_path ON activity(source, path);
CREATE INDEX activity_user ON activity(user_id, at);
