-- Core gaming additions (#51, #52, #28).
--
-- * `controller_profiles`: one controller profile per account and scope
--   (`game_id = 0` is the account-wide profile, any other id is a per-game
--   override). The profile document itself is validated in
--   `ludex_core::gamepad` before it is stored.
-- * `emulators` + `emulator_defaults`: user-configured launch commands
--   (`%ROM%` template), and the default emulator per platform. No emulator
--   binary is shipped with Ludex.
-- * ROM fields on `games` and `library_roots`: a root can be a game library or
--   a ROM library; ROM games carry a platform and an optional emulator.
-- Metadata provider plugins (#28) live in `0018_metadata_providers.sql`;
-- this migration only covers controller profiles and ROM/emulator support.

CREATE TABLE IF NOT EXISTS emulators (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL COLLATE NOCASE UNIQUE,
    command TEXT NOT NULL,
    platforms_json TEXT NOT NULL DEFAULT '[]',
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS emulator_defaults (
    platform TEXT PRIMARY KEY,
    emulator_id INTEGER NOT NULL REFERENCES emulators(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS controller_profiles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- 0 is the account-wide profile; anything else is a per-game override.
    game_id INTEGER NOT NULL DEFAULT 0,
    name TEXT NOT NULL,
    profile_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (user_id, game_id)
);

CREATE INDEX IF NOT EXISTS idx_controller_profiles_user
    ON controller_profiles(user_id);

ALTER TABLE library_roots ADD COLUMN kind TEXT NOT NULL DEFAULT 'game';
ALTER TABLE games ADD COLUMN kind TEXT NOT NULL DEFAULT 'game';
ALTER TABLE games ADD COLUMN platform TEXT;
ALTER TABLE games ADD COLUMN emulator_id INTEGER REFERENCES emulators(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_games_kind_platform ON games(kind, platform);
