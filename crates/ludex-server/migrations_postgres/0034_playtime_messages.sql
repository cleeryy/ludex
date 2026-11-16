-- Playtime outbox receipts (Wave 2 data safety), PostgreSQL edition.
--
-- Same table and columns as `0034_playtime_messages.sql`. `message_id` is the
-- desktop's local session id; the primary key makes a replayed report a no-op
-- instead of adding the same minutes again.

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
