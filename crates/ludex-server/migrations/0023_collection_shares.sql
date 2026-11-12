-- Shareable collections (#173): unlisted visibility, token links and imports.
--
-- `visibility` extends the old `is_public` boolean:
--   private  — owner only (default);
--   unlisted — owner plus anyone holding a live share token;
--   public   — every authenticated account, the community profile and the feed.
-- `is_public` is kept in sync for older clients and for the existing public
-- directory query; `visibility` is authoritative.
--
-- A collection share link is a bearer capability like `share_links` (#9): only
-- the SHA-256 of the token is stored and the plaintext URL is returned once at
-- creation. Revoking a link, deleting the collection or switching it to
-- `private` invalidates access immediately.
--
-- `copied_from_id` records where a collection was imported from, so importing
-- the same share twice into the same account merges missing games instead of
-- duplicating the shelf.

ALTER TABLE collections ADD COLUMN visibility TEXT NOT NULL DEFAULT 'private';
UPDATE collections
   SET visibility = CASE WHEN is_public = 1 THEN 'public' ELSE 'private' END;
-- Older builds wrote a feed row for every collection, including private ones.
-- Drop those entries: a private or unlisted shelf must never be discoverable
-- through the public feed (#173).
DELETE FROM activities
 WHERE kind = 'collection_created'
   AND collection_id IN (SELECT id FROM collections WHERE visibility <> 'public');
ALTER TABLE collections ADD COLUMN copied_from_id INTEGER
    REFERENCES collections(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_collections_visibility
    ON collections(visibility, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_collections_copied_from
    ON collections(copied_from_id);

CREATE TABLE IF NOT EXISTS collection_shares (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    collection_id    INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    token_hash       TEXT NOT NULL UNIQUE,
    label            TEXT,
    created_by       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at       TEXT NOT NULL,
    expires_at       TEXT,
    password_hash    TEXT,
    view_count       INTEGER NOT NULL DEFAULT 0,
    last_viewed_at   TEXT,
    revoked_at       TEXT
);

CREATE INDEX IF NOT EXISTS idx_collection_shares_collection
    ON collection_shares(collection_id, id DESC);
CREATE INDEX IF NOT EXISTS idx_collection_shares_token
    ON collection_shares(token_hash);
