-- Save manager history (#295).
--
-- A save manager needs more than "the newest snapshot": the user must be able
-- to see where a save came from and roll back to a known-good copy.
--
-- - `pinned` marks a snapshot the user promoted to a restore point. Pinned
--   snapshots are never pruned automatically and survive every retention
--   pass, including the one that runs after an automatic upload.
-- - `save_restore_receipts` records that a device actually restored a
--   snapshot (server or local copy). It is the history shown next to the
--   restore points: which device, when, and which snapshot was written.
--
-- The newest snapshot of a game is always kept even when it is unpinned, so a
-- cleanup can never delete the last known-good copy.

ALTER TABLE save_snapshots ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS save_restore_receipts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    game_id     INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    snapshot_id INTEGER NOT NULL REFERENCES save_snapshots(id) ON DELETE CASCADE,
    device_id   TEXT,
    kind        TEXT NOT NULL DEFAULT 'server',
    restored_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_save_restore_receipts_user_game
    ON save_restore_receipts(user_id, game_id, restored_at DESC);

CREATE INDEX IF NOT EXISTS idx_save_snapshots_pinned
    ON save_snapshots(user_id, game_id, pinned);
