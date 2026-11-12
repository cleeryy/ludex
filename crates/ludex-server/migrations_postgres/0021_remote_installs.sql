-- Mobile companion: remote install requests (#167), PostgreSQL edition.
--
-- Same table as `0021_remote_installs.sql`. One row per founder and game keeps
-- the request idempotent; the version label and size are snapshotted.

CREATE TABLE IF NOT EXISTS remote_installs (
    id             TEXT PRIMARY KEY,
    user_id        BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    game_id        BIGINT NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    version_id     BIGINT REFERENCES game_versions(id) ON DELETE SET NULL,
    version_label  TEXT,
    size_bytes     BIGINT,
    device         TEXT,
    status         TEXT NOT NULL DEFAULT 'queued',
    progress       BIGINT NOT NULL DEFAULT 0,
    message        TEXT,
    requested_at   TEXT NOT NULL,
    updated_at     TEXT NOT NULL,
    claimed_at     TEXT,
    completed_at   TEXT,
    UNIQUE (user_id, game_id)
);

CREATE INDEX IF NOT EXISTS idx_remote_installs_user
    ON remote_installs(user_id, requested_at DESC);
CREATE INDEX IF NOT EXISTS idx_remote_installs_status
    ON remote_installs(status, requested_at);
