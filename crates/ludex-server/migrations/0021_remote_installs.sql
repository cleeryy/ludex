-- Mobile companion: remote install requests (#167).
--
-- A phone queues an install for a game; a signed-in desktop claims the row,
-- starts the download locally and reports progress. One row per founder and
-- game keeps the request idempotent: tapping "Install" twice never queues two
-- downloads. The version label and size are snapshotted at request time so the
-- phone still renders a stable description if the version is re-indexed.
--
-- `device` is a free-form label supplied by the requester (e.g. "Pixel 8"); it
-- is never used for authorization. Every request still requires a valid
-- session and is audited.

CREATE TABLE IF NOT EXISTS remote_installs (
    id             TEXT PRIMARY KEY,
    user_id        BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    game_id        BIGINT NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    version_id     BIGINT REFERENCES game_versions(id) ON DELETE SET NULL,
    version_label  TEXT,
    size_bytes     BIGINT,
    device         TEXT,
    status         TEXT NOT NULL DEFAULT 'queued',
    progress       INTEGER NOT NULL DEFAULT 0,
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
