-- Auth schema: users, groups, sessions, per-folder permission rules and
-- per-user source scopes. Owned by the auth work stream. Times are Unix
-- seconds (UTC).

CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    username      TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    -- argon2id PHC string. NULL for accounts that cannot use a password
    -- (for example OIDC or LDAP users).
    password_hash TEXT,
    -- Where the account authenticates: 'local' (password), 'oidc', 'ldap'...
    auth_source   TEXT    NOT NULL DEFAULT 'local',
    is_admin      INTEGER NOT NULL DEFAULT 0,
    disabled      INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);

CREATE TABLE groups (
    id         INTEGER PRIMARY KEY,
    name       TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    created_at INTEGER NOT NULL
);

CREATE TABLE group_members (
    group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id  INTEGER NOT NULL REFERENCES users(id)  ON DELETE CASCADE,
    PRIMARY KEY (group_id, user_id)
);
CREATE INDEX group_members_user ON group_members(user_id);

-- Only the SHA-256 of the cookie token is stored.
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
CREATE INDEX sessions_expires ON sessions(expires_at);

-- A rule grants a permission set to exactly one user or one group on a
-- folder of a source and everything below it. The most specific folder wins
-- (see crates/filebrary/src/auth/permissions.rs). `path` is the real path in
-- the source, normalised: '/' or '/a/b'.
CREATE TABLE permission_rules (
    id         INTEGER PRIMARY KEY,
    source     TEXT    NOT NULL,
    path       TEXT    NOT NULL,
    user_id    INTEGER REFERENCES users(id)  ON DELETE CASCADE,
    group_id   INTEGER REFERENCES groups(id) ON DELETE CASCADE,
    perms      INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    CHECK ((user_id IS NULL) <> (group_id IS NULL))
);
CREATE UNIQUE INDEX permission_rules_user
    ON permission_rules(source, path, user_id) WHERE user_id IS NOT NULL;
CREATE UNIQUE INDEX permission_rules_group
    ON permission_rules(source, path, group_id) WHERE group_id IS NOT NULL;
CREATE INDEX permission_rules_source ON permission_rules(source);

-- Confines a user to one folder of a source (a "home" folder). Paths the
-- user sends are relative to it. No row means the source root.
CREATE TABLE user_scopes (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    source  TEXT    NOT NULL,
    scope   TEXT    NOT NULL,
    PRIMARY KEY (user_id, source)
);
