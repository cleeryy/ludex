-- Full archive content hash (#216), PostgreSQL edition.
--
-- Same semantics as `0031_version_content_sha256.sql`: `archive_hash` remains
-- the sampled fingerprint, `content_sha256` is the full SHA-256 computed by a
-- background job and is nullable while the backfill catches up.

ALTER TABLE game_versions ADD COLUMN IF NOT EXISTS content_sha256 TEXT;

CREATE INDEX IF NOT EXISTS idx_game_versions_content_sha256
    ON game_versions (content_sha256)
    WHERE content_sha256 IS NULL;
