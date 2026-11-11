-- Automatic cloud save sync metadata (#108), PostgreSQL edition.

ALTER TABLE save_snapshots ADD COLUMN source TEXT NOT NULL DEFAULT 'manual';
ALTER TABLE save_snapshots ADD COLUMN device_id TEXT;

CREATE INDEX IF NOT EXISTS idx_save_snapshots_auto
    ON save_snapshots(user_id, game_id, source, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_save_snapshots_hash
    ON save_snapshots(user_id, game_id, sha256);
