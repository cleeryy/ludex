-- Ludex v0.1 initial schema.
--
-- Timestamps are stored as RFC 3339 UTC strings so that they sort
-- lexicographically and remain readable in a SQLite browser.
-- `games.group_key` is an internal scanner identity (normalised title +
-- release-year bucket); it is never exposed through the HTTP API.

CREATE TABLE IF NOT EXISTS users (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    username      TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    email         TEXT,
    role          TEXT NOT NULL DEFAULT 'user',
    activated     INTEGER NOT NULL DEFAULT 1,
    avatar_path   TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS games (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    title                  TEXT NOT NULL,
    sort_title             TEXT NOT NULL,
    group_key              TEXT,
    description            TEXT,
    release_date           TEXT,
    cover_path             TEXT,
    background_path        TEXT,
    screenshots_json       TEXT NOT NULL DEFAULT '[]',
    trailers_json          TEXT NOT NULL DEFAULT '[]',
    genres_json            TEXT NOT NULL DEFAULT '[]',
    developers_json        TEXT NOT NULL DEFAULT '[]',
    publishers_json        TEXT NOT NULL DEFAULT '[]',
    tags_json              TEXT NOT NULL DEFAULT '[]',
    rating                 REAL,
    age_rating             INTEGER,
    early_access           INTEGER NOT NULL DEFAULT 0,
    provider_slug          TEXT,
    provider_id            TEXT,
    provider_url           TEXT,
    launch_executable      TEXT,
    launch_parameters      TEXT,
    installer_executable   TEXT,
    installer_parameters   TEXT,
    uninstaller_executable TEXT,
    uninstaller_parameters TEXT,
    created_at             TEXT NOT NULL,
    updated_at             TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_games_group_key ON games(group_key);
CREATE INDEX IF NOT EXISTS idx_games_sort_title ON games(sort_title);
CREATE INDEX IF NOT EXISTS idx_games_title ON games(title);

CREATE TABLE IF NOT EXISTS game_versions (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    game_id      INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    file_path    TEXT NOT NULL UNIQUE,
    file_name    TEXT NOT NULL,
    size         INTEGER NOT NULL DEFAULT 0,
    version      TEXT,
    release_date TEXT,
    early_access INTEGER NOT NULL DEFAULT 0,
    missing      INTEGER NOT NULL DEFAULT 0,
    indexed_at   TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_game_versions_game_id ON game_versions(game_id);
CREATE INDEX IF NOT EXISTS idx_game_versions_missing ON game_versions(missing);

CREATE TABLE IF NOT EXISTS progress (
    user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    game_id        INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    state          TEXT NOT NULL DEFAULT 'unplayed',
    minutes_played INTEGER NOT NULL DEFAULT 0,
    last_played_at TEXT,
    PRIMARY KEY (user_id, game_id)
);

CREATE INDEX IF NOT EXISTS idx_progress_game_id ON progress(game_id);

CREATE TABLE IF NOT EXISTS bookmarks (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    game_id INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, game_id)
);

CREATE TABLE IF NOT EXISTS play_sessions (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    game_id    INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    minutes    INTEGER NOT NULL,
    started_at TEXT NOT NULL,
    ended_at   TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_play_sessions_user_game ON play_sessions(user_id, game_id);

CREATE TABLE IF NOT EXISTS news (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    title      TEXT NOT NULL,
    body       TEXT NOT NULL,
    author_id  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
