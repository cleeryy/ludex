-- Libraries on removable and network mounts (#290), PostgreSQL edition.
--
-- Same columns as `0035_library_mounts.sql`. `min_free_bytes` defaults to the
-- 5 GiB floor; existing rows keep indexed content when their mount comes back
-- through the stored identity fingerprint.

ALTER TABLE library_roots ADD COLUMN IF NOT EXISTS offline INTEGER NOT NULL DEFAULT 0;
ALTER TABLE library_roots ADD COLUMN IF NOT EXISTS min_free_bytes BIGINT NOT NULL DEFAULT 5368709120;
ALTER TABLE library_roots ADD COLUMN IF NOT EXISTS mount_device_id TEXT;
ALTER TABLE library_roots ADD COLUMN IF NOT EXISTS mount_marker TEXT;
