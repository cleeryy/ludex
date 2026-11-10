-- GameVault -> Ludex JSON export (#6)
--
-- Produces the single JSON document consumed by:
--
--   ludex-server import-gamevault --dump gamevault.json --dry-run
--   ludex-server import-gamevault --dump gamevault.json
--
-- Usage (read-only against GameVault; the dump is the only output):
--
--   psql "$GAMEVAULT_DATABASE_URL" -At -f scripts/gamevault-export.sql > gamevault.json
--
-- The importer never reads GameVault's PostgreSQL database directly: this
-- script is the only schema-aware piece and the dump is reviewable (and easy
-- to keep, encrypt or delete) before anything is written to Ludex.
--
-- SCHEMA ASSUMPTIONS — check these before running on your GameVault version
-- (`\dt` and `\d "Game"` in psql). GameVault is a Prisma application, so table
-- and column names are the Prisma model/field names in double quotes. Adjust
-- the identifiers below if your release renamed them:
--
--   "User"      (id, username, email, role, activated)
--   "Game"      (id, title, description, releaseDate, cover, background,
--                genres, developers, publishers, tags, rating, earlyAccess,
--                metadataProvider, metadataProviderId, metadataProviderUrl)
--   "GameFile"  (gameId, filePath, size)          -- one row per archive/version
--   "Progress"  (userId, gameId, state, minutesPlayed, lastPlayedAt)
--   "Bookmark"  (userId, gameId)
--   "NewsPost"  (title, body, authorId, createdAt, updatedAt)
--
-- GameVault ids are UUIDs while the Ludex import contract uses dense integer
-- join keys. Each section maps its UUIDs with row_number() and the joins stay
-- inside this query, so the dump is self-consistent.
--
-- What is intentionally NOT exported: password hashes (they cannot be reused),
-- sessions, API keys, GameVault+ settings, downloaded media files. Only the
-- artwork paths are exported and the importer warns that refresh replaces them.

WITH
user_ids AS (
    SELECT row_number() OVER (ORDER BY u.id)::bigint AS id, u.id AS source_id
    FROM "User" u
),
user_rows AS (
    SELECT
        user_ids.id,
        u.username,
        NULLIF(u.email, '') AS email,
        CASE lower(u.role::text)
            WHEN 'admin' THEN 'admin'
            WHEN 'editor' THEN 'editor'
            ELSE 'user'
        END AS role,
        COALESCE(u.activated, true) AS activated
    FROM "User" u
    JOIN user_ids ON user_ids.source_id = u.id
),
users_json AS (
    SELECT COALESCE(json_agg(json_build_object(
        'id', id,
        'username', username,
        'email', email,
        'role', role,
        'activated', activated
    ) ORDER BY id), '[]'::json) AS rows FROM user_rows
),
game_ids AS (
    SELECT row_number() OVER (ORDER BY g.id)::bigint AS id, g.id AS source_id
    FROM "Game" g
),
game_rows AS (
    SELECT
        game_ids.id,
        g.title,
        g.description,
        g."releaseDate"::text AS release_date,
        g.cover,
        g.background,
        COALESCE(to_json(g.genres), '[]'::json) AS genres,
        COALESCE(to_json(g.developers), '[]'::json) AS developers,
        COALESCE(to_json(g.publishers), '[]'::json) AS publishers,
        COALESCE(to_json(g.tags), '[]'::json) AS tags,
        g.rating,
        g."earlyAccess" AS early_access,
        g."metadataProvider" AS provider_slug,
        g."metadataProviderId" AS provider_id,
        g."metadataProviderUrl" AS provider_url
    FROM "Game" g
    JOIN game_ids ON game_ids.source_id = g.id
),
files_json AS (
    SELECT
        game_ids.id AS game_id,
        COALESCE(json_agg(json_build_object(
            'filePath', replace(f."filePath", '\', '/'),
            'size', f.size
        ) ORDER BY f."filePath"), '[]'::json) AS versions
    FROM "GameFile" f
    JOIN game_ids ON game_ids.source_id = f."gameId"
    GROUP BY game_ids.id
),
games_json AS (
    SELECT COALESCE(json_agg(json_build_object(
        'id', game_rows.id,
        'title', game_rows.title,
        'description', game_rows.description,
        'releaseDate', game_rows.release_date,
        'cover', game_rows.cover,
        'background', game_rows.background,
        'genres', game_rows.genres,
        'developers', game_rows.developers,
        'publishers', game_rows.publishers,
        'tags', game_rows.tags,
        'rating', game_rows.rating,
        'earlyAccess', game_rows.early_access,
        'providerSlug', game_rows.provider_slug,
        'providerId', game_rows.provider_id,
        'providerUrl', game_rows.provider_url,
        'versions', COALESCE(files_json.versions, '[]'::json)
    ) ORDER BY game_rows.id), '[]'::json) AS rows
    FROM game_rows
    LEFT JOIN files_json ON files_json.game_id = game_rows.id
),
progress_json AS (
    SELECT COALESCE(json_agg(json_build_object(
        'userId', user_ids.id,
        'gameId', game_ids.id,
        'state', p.state::text,
        'minutesPlayed', p."minutesPlayed",
        'lastPlayedAt', p."lastPlayedAt"::text
    ) ORDER BY user_ids.id, game_ids.id), '[]'::json) AS rows
    FROM "Progress" p
    JOIN user_ids ON user_ids.source_id = p."userId"
    JOIN game_ids ON game_ids.source_id = p."gameId"
),
bookmarks_json AS (
    SELECT COALESCE(json_agg(json_build_object(
        'userId', user_ids.id,
        'gameId', game_ids.id
    ) ORDER BY user_ids.id, game_ids.id), '[]'::json) AS rows
    FROM "Bookmark" b
    JOIN user_ids ON user_ids.source_id = b."userId"
    JOIN game_ids ON game_ids.source_id = b."gameId"
),
news_json AS (
    SELECT COALESCE(json_agg(json_build_object(
        'title', n.title,
        'body', n.body,
        'authorId', user_ids.id,
        'createdAt', n."createdAt"::text,
        'updatedAt', n."updatedAt"::text
    ) ORDER BY n."createdAt"), '[]'::json) AS rows
    FROM "NewsPost" n
    LEFT JOIN user_ids ON user_ids.source_id = n."authorId"
)
SELECT json_build_object(
    'format', 'gamevault-export',
    'version', 1,
    'users', users_json.rows,
    'games', games_json.rows,
    'progress', progress_json.rows,
    'bookmarks', bookmarks_json.rows,
    'news', news_json.rows
)::text
FROM users_json, games_json, progress_json, bookmarks_json, news_json;
