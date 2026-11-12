-- Community phase 2 (#161): friendships, presence and in-app notifications.
--
-- `friendships.pair_key` is the canonical "low:high" account pair, so the
-- unique index also blocks a reversed duplicate request. A request is one row
-- in `pending`; accepting moves it to `accepted`. Declining or cancelling
-- deletes the row, which lets the pair try again later.
--
-- `presence` is opt-in (`users.presence_enabled`). Rows older than two minutes
-- are served as offline; the game reference is never exposed to non-friends.
--
-- `notifications` are per recipient and pruned on write: the newest 200 rows
-- per account are kept, older rows are deleted after 90 days.
--
-- `users.friends_activity` opts an account into the friends-only activity and
-- achievement feed. Public profiles are unaffected.

ALTER TABLE users ADD COLUMN presence_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN friends_activity INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS friendships (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    requester_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    addressee_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    pair_key     TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'pending'
                 CHECK (status IN ('pending', 'accepted')),
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    CHECK (requester_id <> addressee_id)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_friendships_pair ON friendships(pair_key);
CREATE INDEX IF NOT EXISTS idx_friendships_requester
    ON friendships(requester_id, status, id DESC);
CREATE INDEX IF NOT EXISTS idx_friendships_addressee
    ON friendships(addressee_id, status, id DESC);

CREATE TABLE IF NOT EXISTS user_blocks (
    blocker_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    blocked_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    PRIMARY KEY (blocker_id, blocked_id),
    CHECK (blocker_id <> blocked_id)
);

CREATE INDEX IF NOT EXISTS idx_user_blocks_blocked ON user_blocks(blocked_id);

CREATE TABLE IF NOT EXISTS presence (
    user_id    INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    state      TEXT NOT NULL CHECK (state IN ('online', 'in_game')),
    game_id    INTEGER REFERENCES games(id) ON DELETE SET NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_presence_updated ON presence(updated_at DESC);

CREATE TABLE IF NOT EXISTS notifications (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind           TEXT NOT NULL
                   CHECK (kind IN ('friend_request', 'friend_accepted',
                                   'friend_achievement', 'collection_share')),
    actor_id       INTEGER REFERENCES users(id) ON DELETE CASCADE,
    game_id        INTEGER REFERENCES games(id) ON DELETE SET NULL,
    achievement_id INTEGER REFERENCES achievements(id) ON DELETE SET NULL,
    payload        TEXT NOT NULL DEFAULT '{}',
    created_at     TEXT NOT NULL,
    read_at        TEXT
);

CREATE INDEX IF NOT EXISTS idx_notifications_user ON notifications(user_id, id DESC);
CREATE INDEX IF NOT EXISTS idx_notifications_unread
    ON notifications(user_id, read_at);

CREATE TABLE IF NOT EXISTS notification_prefs (
    user_id             INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    friend_requests     INTEGER NOT NULL DEFAULT 1,
    friend_accepted     INTEGER NOT NULL DEFAULT 1,
    friend_achievements INTEGER NOT NULL DEFAULT 0,
    collection_shares   INTEGER NOT NULL DEFAULT 1,
    updated_at          TEXT NOT NULL
);
