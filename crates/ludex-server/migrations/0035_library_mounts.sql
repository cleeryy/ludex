-- Libraries on removable and network mounts (#290).
--
-- A root can live on an NFS/SMB export or a removable disk. The columns below
-- let the scanner tell three states apart instead of treating every absent
-- path as deleted content:
--
-- - `offline` is 1 when the configured mount is absent (the drive is
--   unplugged, the share is not mounted); rows under the root are flagged
--   `root_unavailable`, never `missing`.
-- - `mount_device_id` / `mount_marker` fingerprint the mount that last served
--   the root. When the root comes back with the same identity the scanner
--   reuses the stored index instead of walking the tree again.
-- - `min_free_bytes` is the per-library free-space floor. Installs and uploads
--   into this root leave at least this much free; the default is 5 GiB.

ALTER TABLE library_roots ADD COLUMN offline INTEGER NOT NULL DEFAULT 0;
ALTER TABLE library_roots ADD COLUMN min_free_bytes BIGINT NOT NULL DEFAULT 5368709120;
ALTER TABLE library_roots ADD COLUMN mount_device_id TEXT;
ALTER TABLE library_roots ADD COLUMN mount_marker TEXT;
