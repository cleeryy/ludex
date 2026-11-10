-- Achievements framework, local providers and unlock state (#117, #118).
--
-- `achievements` holds the definitions. `game_id IS NULL` is reserved for
-- global achievements; the two partial unique indexes keep `key` unique in
-- each namespace (SQLite treats every NULL as distinct, so a plain
-- UNIQUE(game_id, key) would allow duplicates for global rows).
--
-- Unlocks are per account and immutable once written: the timestamp is the
-- first unlock, later provider rescans never move it. `progress` stays for
-- providers that report partial completion and is clamped to 0..=1 by the
-- server.
--
-- `achievement_sources` remembers where a game was scanned so the scheduled
-- worker can rescan without a client call. `achievement_scans` is the audit
-- trail of past runs.

CREATE TABLE IF NOT EXISTS achievements (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    game_id     INTEGER REFERENCES games(id) ON DELETE CASCADE,
    key         TEXT NOT NULL,
    name        TEXT NOT NULL,
    description TEXT,
    icon_path   TEXT,
    points      INTEGER NOT NULL DEFAULT 0,
    hidden      INTEGER NOT NULL DEFAULT 0,
    provider    TEXT NOT NULL DEFAULT 'manual',
    source_json TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_achievements_game_key
    ON achievements(game_id, key) WHERE game_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_achievements_global_key
    ON achievements(key) WHERE game_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_achievements_game_id ON achievements(game_id);

CREATE TABLE IF NOT EXISTS user_achievements (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    achievement_id INTEGER NOT NULL REFERENCES achievements(id) ON DELETE CASCADE,
    unlocked_at    TEXT NOT NULL,
    progress       REAL NOT NULL DEFAULT 1.0,
    source         TEXT NOT NULL DEFAULT 'manual',
    raw_json       TEXT,
    created_at     TEXT NOT NULL,
    UNIQUE (user_id, achievement_id)
);

CREATE INDEX IF NOT EXISTS idx_user_achievements_user
    ON user_achievements(user_id, unlocked_at DESC);
CREATE INDEX IF NOT EXISTS idx_user_achievements_achievement
    ON user_achievements(achievement_id);

CREATE TABLE IF NOT EXISTS achievement_sources (
    game_id      INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    install_path TEXT,
    app_id       TEXT,
    providers    TEXT,
    updated_at   TEXT NOT NULL,
    PRIMARY KEY (game_id, user_id)
);

CREATE TABLE IF NOT EXISTS achievement_scans (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    game_id    INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    user_id    INTEGER REFERENCES users(id) ON DELETE CASCADE,
    provider   TEXT NOT NULL,
    status     TEXT NOT NULL,
    found      INTEGER NOT NULL DEFAULT 0,
    unlocked   INTEGER NOT NULL DEFAULT 0,
    error      TEXT,
    scanned_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_achievement_scans_game
    ON achievement_scans(game_id, id DESC);
