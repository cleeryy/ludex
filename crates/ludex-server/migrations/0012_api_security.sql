-- API security (#17/#19) and resumable uploads (#21).
--
-- `auth_sessions` records every JWT issued by a login (browser or OIDC). The
-- `jti` claim ties the token to its row so a session can be listed and
-- revoked without keeping the token itself. `tokens_valid_after` on `users`
-- invalidates every older token at once (password change, admin revoke-all).
--
-- Share links live in their own migration (`0011_share_links.sql`).
--
-- `uploads` tracks resumable archive uploads. Chunks live on disk under
-- `{data}/uploads/{id}`; the row is a rebuildable index and is deleted with
-- the chunk directory.

CREATE TABLE IF NOT EXISTS auth_sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    jti TEXT NOT NULL UNIQUE,
    user_agent TEXT,
    ip TEXT,
    created_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    revoked_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_auth_sessions_user
    ON auth_sessions(user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_auth_sessions_expires
    ON auth_sessions(expires_at);

CREATE TABLE IF NOT EXISTS token_revocations (
    jti TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    reason TEXT,
    revoked_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_token_revocations_user
    ON token_revocations(user_id);

ALTER TABLE users ADD COLUMN tokens_valid_after TEXT;

CREATE TABLE IF NOT EXISTS uploads (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    file_name TEXT NOT NULL,
    size INTEGER NOT NULL,
    sha256 TEXT,
    chunk_size INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'open',
    game_id INTEGER,
    version_id INTEGER,
    error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_uploads_user
    ON uploads(user_id, created_at DESC);
