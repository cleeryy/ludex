-- Preferred game version (#168).
--
-- `games.default_version_id` pins the version used by downloads, the
-- `/versions/latest` update check and "Play" on a fresh install. Editors set it
-- from the game page: picking an older version is a rollback that keeps the
-- newer archives available. `ON DELETE SET NULL` means deleting the pinned
-- archive falls back to the newest available one instead of breaking the game.

ALTER TABLE games ADD COLUMN default_version_id INTEGER REFERENCES game_versions(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_games_default_version ON games(default_version_id);
