-- Per-user cloud save snapshots (issue #11).
--
-- Archives are opaque to the server: only their size and SHA-256 are recorded
-- so a client can verify a download. Files live under
-- `{data_dir}/saves/{user_id}/{game_id}/{snapshot_id}-{sanitized_name}`.
--
-- `version_id` is optional metadata for a future "save tied to a build" flow;
-- deleting a version keeps the snapshot.

CREATE TABLE IF NOT EXISTS save_snapshots (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    game_id    INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    version_id INTEGER REFERENCES game_versions(id) ON DELETE SET NULL,
    label      TEXT NOT NULL DEFAULT '',
    file_name  TEXT NOT NULL,
    size       INTEGER NOT NULL DEFAULT 0,
    sha256     TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_save_snapshots_user_game
    ON save_snapshots(user_id, game_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_save_snapshots_game
    ON save_snapshots(game_id);
