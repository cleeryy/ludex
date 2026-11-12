-- Preferred game version (#168), PostgreSQL edition.
--
-- Same column as `0021_game_version_preferences.sql`: the version downloads,
-- the update check and fresh installs should use. Selecting an older version
-- is a rollback; the newer archives stay available.

ALTER TABLE games ADD COLUMN default_version_id BIGINT REFERENCES game_versions(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_games_default_version ON games(default_version_id);
