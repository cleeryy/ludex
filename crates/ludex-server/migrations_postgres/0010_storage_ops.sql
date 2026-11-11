-- Object storage for game archives (#47), PostgreSQL edition.

ALTER TABLE game_versions ADD COLUMN storage_backend TEXT NOT NULL DEFAULT 'local';
ALTER TABLE game_versions ADD COLUMN object_key TEXT;
ALTER TABLE game_versions ADD COLUMN etag TEXT;

CREATE INDEX IF NOT EXISTS idx_game_versions_storage
    ON game_versions(storage_backend);
