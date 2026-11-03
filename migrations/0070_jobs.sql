-- Background jobs (conversions, archives, custom jobs, and jobs other
-- subsystems submit). Owned by the jobs work stream. Times are Unix seconds.
--
-- status: queued | running | succeeded | failed | cancelled
CREATE TABLE jobs (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    kind             TEXT    NOT NULL,
    title            TEXT    NOT NULL,
    -- NULL for system jobs (visible to administrators only).
    user_id          INTEGER REFERENCES users(id) ON DELETE CASCADE,
    params           TEXT    NOT NULL,
    status           TEXT    NOT NULL DEFAULT 'queued',
    progress         REAL    NOT NULL DEFAULT 0,
    message          TEXT,
    error            TEXT,
    result           TEXT,
    attempts         INTEGER NOT NULL DEFAULT 0,
    cancel_requested INTEGER NOT NULL DEFAULT 0,
    created_at       INTEGER NOT NULL,
    started_at       INTEGER,
    finished_at      INTEGER,
    updated_at       INTEGER NOT NULL
);
CREATE INDEX jobs_status ON jobs(status, id);
CREATE INDEX jobs_user ON jobs(user_id, id);
CREATE INDEX jobs_finished ON jobs(finished_at);
