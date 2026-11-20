-- Save conflict policy for multi-device play (#296).
--
-- The desktop used to treat a newer remote snapshot as a silent overwrite: the
-- last machine to sync won. This migration stores everything the policy chain
-- needs to decide otherwise:
--
-- * `client_mtime` is the newest save file mtime reported by the device (in
--   device time) and `clock_offset_secs` is the server's estimate of that
--   device's clock error (device clock minus server clock) at upload time. A
--   side whose offset exceeds the skew limit can never win on mtime alone.
-- * `save_seq` is the game's own save counter when the client knows it; it
--   vetoes a size/mtime lineage that contradicts it.
-- * `conflict_of` marks a snapshot kept aside as the losing side of a
--   conflict. Such a row is a restore point, never "the latest save".
-- * `save_devices` keeps the last known clock offset per device, so a device
--   that is consistently ahead is visible before its next upload.
-- * `save_conflicts` is the open/closed conflict list surfaced by the API.
--   `status = 'open'` means the user has not chosen yet; nothing is deleted by
--   any transition.
-- * `save_sync_policies` is the per-game "always prefer this device" override.

ALTER TABLE save_snapshots ADD COLUMN client_mtime TEXT;
ALTER TABLE save_snapshots ADD COLUMN clock_offset_secs INTEGER NOT NULL DEFAULT 0;
ALTER TABLE save_snapshots ADD COLUMN save_seq INTEGER;
ALTER TABLE save_snapshots ADD COLUMN conflict_of INTEGER REFERENCES save_snapshots(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_save_snapshots_latest
    ON save_snapshots(user_id, game_id, conflict_of, created_at DESC);

CREATE TABLE IF NOT EXISTS save_devices (
    user_id           INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id         TEXT    NOT NULL,
    label             TEXT,
    clock_offset_secs INTEGER NOT NULL DEFAULT 0,
    first_seen_at     TEXT    NOT NULL,
    last_seen_at      TEXT    NOT NULL,
    PRIMARY KEY (user_id, device_id)
);

CREATE TABLE IF NOT EXISTS save_conflicts (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id              INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    game_id              INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    existing_snapshot_id INTEGER NOT NULL REFERENCES save_snapshots(id) ON DELETE CASCADE,
    candidate_snapshot_id INTEGER NOT NULL REFERENCES save_snapshots(id) ON DELETE CASCADE,
    reason               TEXT    NOT NULL,
    status               TEXT    NOT NULL DEFAULT 'open',
    detected_at          TEXT    NOT NULL,
    resolved_at          TEXT,
    strategy             TEXT,
    UNIQUE (existing_snapshot_id, candidate_snapshot_id)
);

CREATE INDEX IF NOT EXISTS idx_save_conflicts_open
    ON save_conflicts(user_id, game_id, status, detected_at DESC);

CREATE TABLE IF NOT EXISTS save_sync_policies (
    user_id          INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    game_id          INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    prefer_device_id TEXT,
    updated_at       TEXT    NOT NULL,
    PRIMARY KEY (user_id, game_id)
);
