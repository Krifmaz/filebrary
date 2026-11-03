-- Share links: public links to files and folders (and, later, collections).
CREATE TABLE IF NOT EXISTS shares (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Random URL-safe token used in public links. Unique and unguessable.
    token          TEXT    NOT NULL UNIQUE,
    -- User who created the share. The share stops working if they lose the rights it grants.
    owner_id       INTEGER NOT NULL,
    -- Source id the shared paths live in.
    source         TEXT    NOT NULL,
    -- JSON array of absolute source paths ("/a/b"). One entry for a file or folder share.
    paths          TEXT    NOT NULL,
    -- argon2id PHC string, or NULL when the share has no password.
    password_hash  TEXT,
    -- Unix seconds after which the share is dead, or NULL for no expiry.
    expires_at     INTEGER,
    -- Maximum number of downloads, or NULL for unlimited.
    max_downloads  INTEGER,
    downloads      INTEGER NOT NULL DEFAULT 0,
    -- 1 for upload-only drop links (visitors can upload but not list or download).
    upload_only    INTEGER NOT NULL DEFAULT 0,
    created_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS shares_owner ON shares(owner_id);

-- Grants issued after a correct share password; stored as SHA-256 of the secret.
CREATE TABLE IF NOT EXISTS share_access (
    access_hash TEXT    PRIMARY KEY,
    share_id    INTEGER NOT NULL REFERENCES shares(id) ON DELETE CASCADE,
    expires_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS share_access_share ON share_access(share_id);
