-- Collections (#25) and the community feed (#10).
--
-- Collections are per user. `kind = 'manual'` keeps an explicit list in
-- `collection_games`; `kind = 'smart'` stores a rule tree in `rules_json` and
-- evaluates it at read time. Public collections are readable by every
-- authenticated account; private ones only by their owner.
--
-- Activity rows are the public feed. `game_id`/`collection_id` use
-- `ON DELETE SET NULL` so the feed keeps the human-readable metadata it
-- captured even after the target is deleted.

CREATE TABLE IF NOT EXISTS collections (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    description TEXT,
    is_public   INTEGER NOT NULL DEFAULT 0,
    kind        TEXT NOT NULL DEFAULT 'manual' CHECK (kind IN ('manual', 'smart')),
    rules_json  TEXT NOT NULL DEFAULT '{}',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_collections_user_name
    ON collections(user_id, name COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS idx_collections_user ON collections(user_id, id DESC);
CREATE INDEX IF NOT EXISTS idx_collections_public ON collections(is_public, updated_at DESC);

CREATE TABLE IF NOT EXISTS collection_games (
    collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    game_id       INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    added_at      TEXT NOT NULL,
    PRIMARY KEY (collection_id, game_id)
);

CREATE INDEX IF NOT EXISTS idx_collection_games_game ON collection_games(game_id);

-- Opt-in public profile. `display_name` and `bio` are only exposed through the
-- community endpoints, never through `/api/users` or `/api/auth/me`.
ALTER TABLE users ADD COLUMN public_profile INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN display_name TEXT;
ALTER TABLE users ADD COLUMN bio TEXT;

CREATE TABLE IF NOT EXISTS activities (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind          TEXT NOT NULL
                  CHECK (kind IN ('game_added', 'game_installed', 'game_played',
                                  'game_completed', 'collection_created')),
    game_id       INTEGER REFERENCES games(id) ON DELETE SET NULL,
    collection_id INTEGER REFERENCES collections(id) ON DELETE SET NULL,
    metadata      TEXT NOT NULL DEFAULT '{}',
    created_at    TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_activities_feed ON activities(id DESC);
CREATE INDEX IF NOT EXISTS idx_activities_user ON activities(user_id, id DESC);
