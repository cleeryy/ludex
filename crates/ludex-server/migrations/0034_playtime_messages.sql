-- Playtime outbox receipts (Wave 2 data safety).
--
-- The desktop queues finished sessions locally and reports them with a
-- client-generated `messageId` (the local session id). A retry after a crash
-- between "server applied it" and "desktop marked it synced" must not count
-- the same minutes twice, so every accepted report is recorded here and a
-- replay of the same (user, message) is a no-op.
--
-- Rows are per user and pruned by `received_at`: the table only exists to make
-- the last day of retries idempotent. `game_id` and `minutes` are kept so an
-- operator can see what a replay would have added.
--
-- Reports without a message id (older desktop builds, API clients) keep the
-- historical additive behaviour and are never recorded here.

CREATE TABLE IF NOT EXISTS playtime_messages (
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    message_id  TEXT NOT NULL,
    game_id     BIGINT NOT NULL,
    minutes     BIGINT NOT NULL,
    received_at TEXT NOT NULL,
    PRIMARY KEY (user_id, message_id)
);

CREATE INDEX IF NOT EXISTS idx_playtime_messages_received
    ON playtime_messages(received_at);
