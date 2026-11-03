-- Account foundations: the read-only role, "remember me" sessions and
-- per-user preferences. Times are Unix seconds (UTC).

-- Read-only users can view and download what their rules allow, nothing
-- more (an upper bound on every permission check, like an API key's).
-- Ignored for administrators.
ALTER TABLE users ADD COLUMN read_only INTEGER NOT NULL DEFAULT 0;

-- 1: "remember me" (a cookie that survives the browser closing, with the
-- full session lifetime); 0: a browser-session cookie with a short lifetime.
ALTER TABLE sessions ADD COLUMN persistent INTEGER NOT NULL DEFAULT 1;

-- Per-user preferences that are not the theme (see user_themes).
CREATE TABLE user_preferences (
    user_id    INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    -- How folders open: 'list' or 'grid'.
    view_mode  TEXT    NOT NULL DEFAULT 'list',
    updated_at INTEGER NOT NULL
);
