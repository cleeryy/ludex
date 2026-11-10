-- Automatic cloud save sync metadata (#108).
--
-- `source` distinguishes manual snapshots (the user pressed Upload) from
-- automatic ones created after a game exits. Retention only ever prunes
-- `auto` rows, so a manual snapshot is never deleted by the scheduler.
-- `device_id` identifies the machine that produced the archive; it is opaque
-- and never used for authorisation.

ALTER TABLE save_snapshots ADD COLUMN source TEXT NOT NULL DEFAULT 'manual';
ALTER TABLE save_snapshots ADD COLUMN device_id TEXT;

CREATE INDEX IF NOT EXISTS idx_save_snapshots_auto
    ON save_snapshots(user_id, game_id, source, created_at DESC);

-- Auto uploads are idempotent by content hash; this index keeps the lookup
-- cheap without making manual re-uploads of identical bytes illegal.
CREATE INDEX IF NOT EXISTS idx_save_snapshots_hash
    ON save_snapshots(user_id, game_id, sha256);
