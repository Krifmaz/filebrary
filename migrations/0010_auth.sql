-- PLACEHOLDER owned by the Auth work stream, which replaces this file.
-- Minimal users/sessions/grants so the scaffold's file routes can be tested.

CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    username      TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    password_hash TEXT    NOT NULL,
    is_admin      INTEGER NOT NULL DEFAULT 0,
    disabled      INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL
);

CREATE TABLE user_sources (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    source  TEXT    NOT NULL,
    perms   INTEGER NOT NULL,
    scope   TEXT    NOT NULL DEFAULT '/',
    PRIMARY KEY (user_id, source)
);

CREATE TABLE sessions (
    token_hash BLOB    PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    csrf_token TEXT    NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    ip         TEXT,
    user_agent TEXT
);
CREATE INDEX sessions_user ON sessions(user_id);
