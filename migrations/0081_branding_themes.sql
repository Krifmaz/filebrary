-- Branding (server-wide, set by administrators) and per-user theme choice.
-- Owned by the comments/collections/themes work stream.

-- A single row (id = 1) holding the branding JSON and the optional logo.
CREATE TABLE branding (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    data       TEXT    NOT NULL DEFAULT '{}',
    logo       BLOB,
    logo_mime  TEXT,
    updated_at INTEGER NOT NULL
);

-- theme: 'light' | 'dark' | 'system' | 'custom'; colours: JSON object of
-- CSS variable name -> '#rrggbb', used when theme = 'custom'.
CREATE TABLE user_themes (
    user_id    INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    theme      TEXT    NOT NULL,
    colours    TEXT    NOT NULL DEFAULT '{}',
    updated_at INTEGER NOT NULL
);
