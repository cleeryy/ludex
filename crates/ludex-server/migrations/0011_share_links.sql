-- Share links for a game or a single version (#9).
--
-- A share link is a bearer capability: only the SHA-256 of the token is
-- stored, the plaintext link is returned to the creator exactly once.
-- `version_id IS NULL` means "latest available version at access time", so a
-- link keeps working after a newer version is indexed. A link pinned to a
-- version is deleted with that version row rather than silently widening to
-- "latest". `password_hash` is Argon2id (nullable), `max_downloads` NULL means
-- unlimited. Links are revoked softly (`revoked_at`) so the audit trail keeps
-- the row.

CREATE TABLE IF NOT EXISTS share_links (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    token_hash       TEXT NOT NULL UNIQUE,
    game_id          INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    version_id       INTEGER REFERENCES game_versions(id) ON DELETE CASCADE,
    label            TEXT,
    created_by       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at       TEXT NOT NULL,
    expires_at       TEXT,
    password_hash    TEXT,
    max_downloads    INTEGER,
    download_count   INTEGER NOT NULL DEFAULT 0,
    revoked_at       TEXT,
    last_accessed_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_share_links_game_id ON share_links(game_id);
CREATE INDEX IF NOT EXISTS idx_share_links_token_hash ON share_links(token_hash);
