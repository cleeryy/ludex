-- Library search and advanced filters, PostgreSQL edition.

ALTER TABLE games ADD COLUMN alt_titles_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE games ADD COLUMN max_players BIGINT;

CREATE INDEX IF NOT EXISTS idx_games_release_date ON games(release_date);
CREATE INDEX IF NOT EXISTS idx_games_rating ON games(rating);
CREATE INDEX IF NOT EXISTS idx_games_created_at ON games(created_at);
CREATE INDEX IF NOT EXISTS idx_games_provider_slug ON games(provider_slug);
CREATE INDEX IF NOT EXISTS idx_games_max_players ON games(max_players);
CREATE INDEX IF NOT EXISTS idx_progress_user_last_played ON progress(user_id, last_played_at);
