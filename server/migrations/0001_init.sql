-- Events that are (or were) broadcast live.
CREATE TABLE streams (
    id            TEXT PRIMARY KEY,
    title         TEXT NOT NULL,
    description   TEXT NOT NULL DEFAULT '',
    category      TEXT NOT NULL DEFAULT 'other',
    status        TEXT NOT NULL CHECK (status IN ('scheduled', 'live', 'ended')),
    scheduled_at  INTEGER,
    started_at    INTEGER,
    ended_at      INTEGER,
    created_at    INTEGER NOT NULL,
    poster_at     INTEGER
);
CREATE INDEX streams_status ON streams (status, scheduled_at);

-- Archive entries. One per broadcast; the files live under DATA_DIR/public/rec/<id>.
CREATE TABLE recordings (
    id            TEXT PRIMARY KEY,
    stream_id     TEXT REFERENCES streams (id) ON DELETE SET NULL,
    title         TEXT NOT NULL,
    description   TEXT NOT NULL DEFAULT '',
    category      TEXT NOT NULL DEFAULT 'other',
    status        TEXT NOT NULL CHECK (status IN ('recording', 'processing', 'ready', 'failed')),
    hidden        INTEGER NOT NULL DEFAULT 0,
    recorded_at   INTEGER NOT NULL,
    duration_ms   INTEGER,
    width         INTEGER,
    height        INTEGER,
    renditions    TEXT NOT NULL DEFAULT '[]',
    has_thumb     INTEGER NOT NULL DEFAULT 0,
    views         INTEGER NOT NULL DEFAULT 0,
    error         TEXT,
    created_at    INTEGER NOT NULL
);
CREATE INDEX recordings_list ON recordings (hidden, status, recorded_at DESC);

-- Post-processing queue (survives restarts).
CREATE TABLE jobs (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    recording_id  TEXT NOT NULL REFERENCES recordings (id) ON DELETE CASCADE,
    kind          TEXT NOT NULL CHECK (kind IN ('finalize', 'ladder')),
    status        TEXT NOT NULL CHECK (status IN ('pending', 'running', 'done', 'failed')),
    attempts      INTEGER NOT NULL DEFAULT 0,
    error         TEXT,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);
CREATE INDEX jobs_pending ON jobs (status, id);

CREATE TABLE admin_sessions (
    token_hash    TEXT PRIMARY KEY,
    created_at    INTEGER NOT NULL,
    expires_at    INTEGER NOT NULL
);
