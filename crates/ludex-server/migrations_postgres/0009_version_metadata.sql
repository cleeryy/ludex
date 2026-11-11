-- Version metadata for desktop update detection (#30), PostgreSQL edition.

ALTER TABLE game_versions ADD COLUMN archive_hash TEXT;
ALTER TABLE game_versions ADD COLUMN file_modified BIGINT;
ALTER TABLE game_versions ADD COLUMN updated_at TEXT;

UPDATE game_versions SET updated_at = indexed_at WHERE updated_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_game_versions_updated
    ON game_versions(game_id, updated_at DESC);
