-- Version metadata for desktop update detection (#30).
--
-- `archive_hash` is the cheap sampled fingerprint produced by
-- `ludex_core::fingerprint` (`sha256-sample:<hex>`), not a full content hash.
-- It is NULL until the scanner has computed it for the file.
--
-- `file_modified` is the archive mtime (Unix nanoseconds) at indexing time, so a
-- same-size replacement is detected and re-fingerprinted on the next scan.
--
-- `updated_at` changes whenever the version row is re-indexed (file replaced,
-- renamed or re-hashed). Existing rows are backfilled from `indexed_at`.

ALTER TABLE game_versions ADD COLUMN archive_hash TEXT;
ALTER TABLE game_versions ADD COLUMN file_modified INTEGER;
ALTER TABLE game_versions ADD COLUMN updated_at TEXT;

UPDATE game_versions SET updated_at = indexed_at WHERE updated_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_game_versions_updated
    ON game_versions(game_id, updated_at DESC);
