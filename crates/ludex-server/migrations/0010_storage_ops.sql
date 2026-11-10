-- Object storage for game archives (#47).
--
-- `storage_backend` is `local` for rows served from a library root, and `s3`
-- for rows whose archive was uploaded to the configured S3-compatible bucket.
-- `object_key` is the key inside the bucket; `etag` is what the object store
-- returned on upload. Existing rows keep `local` so the disk path stays the
-- source of truth until a scan uploads them.
--
-- The local library file is never deleted: it remains the fallback when the
-- object is missing from the bucket or the backend is disabled.

ALTER TABLE game_versions ADD COLUMN storage_backend TEXT NOT NULL DEFAULT 'local';
ALTER TABLE game_versions ADD COLUMN object_key TEXT;
ALTER TABLE game_versions ADD COLUMN etag TEXT;

CREATE INDEX IF NOT EXISTS idx_game_versions_storage
    ON game_versions(storage_backend);
