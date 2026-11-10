-- Server integrations: API keys (#40), audit log (#42) and webhooks (#41).
--
-- Timestamps are RFC 3339 UTC strings, like every other table.
--
-- API keys: only the SHA-256 of a 32-byte random token is stored. `prefix`
-- is a short, non-secret fragment used to look the row up without a full
-- table scan; it is safe to display and log. Revocation keeps the row so the
-- audit trail keeps referring to a real key id.

CREATE TABLE IF NOT EXISTS api_keys (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    prefix       TEXT NOT NULL,
    secret_hash  TEXT NOT NULL,
    scopes       TEXT NOT NULL DEFAULT '["read"]',
    created_at   TEXT NOT NULL,
    last_used_at TEXT,
    revoked_at   TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_api_keys_prefix ON api_keys(prefix);
CREATE INDEX IF NOT EXISTS idx_api_keys_user ON api_keys(user_id, id DESC);

-- Audit log. `actor_id` is a user id when `actor_kind = 'user'` and an API
-- key id when `actor_kind = 'api_key'`; system events leave it NULL. There is
-- no foreign key on purpose: rows must survive account/key deletion.
CREATE TABLE IF NOT EXISTS audit_logs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    actor_id    INTEGER,
    actor_kind  TEXT NOT NULL DEFAULT 'system'
                CHECK (actor_kind IN ('user', 'api_key', 'system')),
    action      TEXT NOT NULL,
    target_type TEXT,
    target_id   TEXT,
    metadata    TEXT NOT NULL DEFAULT '{}',
    ip          TEXT,
    created_at  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_audit_logs_created ON audit_logs(created_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_audit_logs_actor ON audit_logs(actor_id, id DESC);
CREATE INDEX IF NOT EXISTS idx_audit_logs_action ON audit_logs(action, id DESC);

-- Webhooks. `events` is a JSON array of subscribed event names; `secret` is
-- the HMAC-SHA256 key.

CREATE TABLE IF NOT EXISTS webhooks (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    url        TEXT NOT NULL,
    secret     TEXT NOT NULL,
    events     TEXT NOT NULL,
    enabled    INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL
);

-- Deliveries keep the exact payload that was signed so a retry is
-- byte-for-byte identical. `next_attempt_at` is NULL once delivered or after
-- the fifth failed attempt.

CREATE TABLE IF NOT EXISTS webhook_deliveries (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    webhook_id      INTEGER NOT NULL REFERENCES webhooks(id) ON DELETE CASCADE,
    event           TEXT NOT NULL,
    payload         TEXT NOT NULL,
    status_code     INTEGER,
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT,
    delivered_at    TEXT,
    last_error      TEXT
);

CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_due
    ON webhook_deliveries(next_attempt_at)
    WHERE delivered_at IS NULL AND next_attempt_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_webhook
    ON webhook_deliveries(webhook_id, id DESC);
