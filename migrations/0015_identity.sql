-- Identity features: TOTP 2FA, API keys, quotas, external identities (OIDC/LDAP).
-- Owned by the filebrary-identity crate. Requires the users table from 0010.

CREATE TABLE user_totp (
    user_id    INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    secret_enc BLOB    NOT NULL,          -- secret encrypted with the server key
    nonce      BLOB    NOT NULL,
    enabled    INTEGER NOT NULL DEFAULT 0, -- 0 while enrollment is pending
    last_step  INTEGER,                   -- last accepted time step (replay guard)
    created_at INTEGER NOT NULL
);

CREATE TABLE totp_recovery_codes (
    user_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash BLOB    NOT NULL,           -- SHA-256 of the normalised code
    used_at   INTEGER,
    PRIMARY KEY (user_id, code_hash)
);

CREATE TABLE api_keys (
    id           TEXT    PRIMARY KEY,     -- public part of the key
    user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT    NOT NULL,
    secret_hash  BLOB    NOT NULL,        -- SHA-256 of the secret part
    scopes       TEXT    NOT NULL,        -- space-separated scope names
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER,
    last_used_at INTEGER
);
CREATE INDEX api_keys_user ON api_keys(user_id);

CREATE TABLE user_quotas (
    user_id     INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    limit_bytes INTEGER,                  -- NULL: unlimited
    used_bytes  INTEGER NOT NULL DEFAULT 0,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE external_identities (
    provider      TEXT    NOT NULL,       -- 'oidc' or 'ldap'
    issuer        TEXT    NOT NULL,       -- OIDC issuer URL or LDAP server URL
    subject       TEXT    NOT NULL,       -- OIDC sub or LDAP DN
    user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at    INTEGER NOT NULL,
    last_login_at INTEGER NOT NULL,
    PRIMARY KEY (provider, issuer, subject)
);
CREATE INDEX external_identities_user ON external_identities(user_id);

CREATE TABLE oidc_pending (
    state_hash    BLOB    PRIMARY KEY,    -- SHA-256 of the state parameter
    nonce         TEXT    NOT NULL,
    pkce_verifier TEXT    NOT NULL,
    return_to     TEXT,
    created_at    INTEGER NOT NULL
);
