-- Full archive content hash (#216).
--
-- `archive_hash` stays the cheap sampled fingerprint used for update
-- detection. `content_sha256` is the SHA-256 of every byte of the archive,
-- computed by a background job (never in the request path) and published as
-- `contentSha256` plus the `X-Ludex-Archive-Sha256` download header so clients
-- can verify a transfer before extraction.
--
-- Nullable on purpose: legacy rows stay downloadable while the backfill job
-- hashes them.

ALTER TABLE game_versions ADD COLUMN content_sha256 TEXT;

CREATE INDEX IF NOT EXISTS idx_game_versions_content_sha256
    ON game_versions (content_sha256)
    WHERE content_sha256 IS NULL;
