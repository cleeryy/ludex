//! Shared SQL column lists and row to DTO mapping.

use ludex_core::models::GameSummary;

use crate::db;
use crate::models::{GameDetailRow, GameListRow};

pub(crate) const LIST_COLUMNS: &str = "g.id, g.title, g.sort_title, g.release_date, g.cover_path, \
g.background_path, g.rating, g.genres_json, g.early_access, g.alt_titles_json, g.max_players, \
g.provider_slug, g.kind, g.platform, g.emulator_id, \
COALESCE(p.state, 'unplayed') AS state, COALESCE(p.minutes_played, 0) AS minutes_played, \
CAST(CASE WHEN b.user_id IS NULL THEN 0 ELSE 1 END AS BIGINT) AS bookmarked, p.last_played_at, \
(SELECT COUNT(*) FROM game_versions v WHERE v.game_id = g.id AND v.missing = 0 AND v.root_unavailable = 0) AS version_count, \
(SELECT COUNT(*) FROM game_versions v WHERE v.game_id = g.id AND v.missing = 0 AND v.root_unavailable = 0) AS available_version_count, \
(SELECT COUNT(*) FROM game_versions v WHERE v.game_id = g.id AND (v.missing = 1 OR v.root_unavailable = 1)) AS missing_version_count, \
(SELECT CAST(COALESCE(SUM(v.size), 0) AS BIGINT) FROM game_versions v WHERE v.game_id = g.id AND v.missing = 0 AND v.root_unavailable = 0) AS total_size";

pub(crate) const DETAIL_COLUMNS: &str = "g.id, g.title, g.sort_title, g.description, g.release_date, \
g.cover_path, g.background_path, g.screenshots_json, g.trailers_json, g.genres_json, \
g.developers_json, g.publishers_json, g.tags_json, g.rating, g.age_rating, g.early_access, \
g.alt_titles_json, g.max_players, \
g.provider_slug, g.provider_id, g.provider_url, g.launch_executable, g.launch_parameters, \
g.installer_executable, g.installer_parameters, g.uninstaller_executable, \
g.uninstaller_parameters, g.kind, g.platform, g.emulator_id, g.default_version_id, \
COALESCE(p.state, 'unplayed') AS state, \
COALESCE(p.minutes_played, 0) AS minutes_played, \
CAST(CASE WHEN b.user_id IS NULL THEN 0 ELSE 1 END AS BIGINT) AS bookmarked, p.last_played_at, \
(SELECT COUNT(*) FROM game_versions v WHERE v.game_id = g.id AND v.missing = 0 AND v.root_unavailable = 0) AS version_count, \
(SELECT COUNT(*) FROM game_versions v WHERE v.game_id = g.id AND v.missing = 0 AND v.root_unavailable = 0) AS available_version_count, \
(SELECT COUNT(*) FROM game_versions v WHERE v.game_id = g.id AND (v.missing = 1 OR v.root_unavailable = 1)) AS missing_version_count, \
(SELECT CAST(COALESCE(SUM(v.size), 0) AS BIGINT) FROM game_versions v WHERE v.game_id = g.id AND v.missing = 0 AND v.root_unavailable = 0) AS total_size";

pub(crate) const FROM_JOINS: &str = " FROM games g \
LEFT JOIN progress p ON p.game_id = g.id AND p.user_id = ? \
LEFT JOIN bookmarks b ON b.game_id = g.id AND b.user_id = ?";

/// Availability label shared by list rows and the detail payload (#210).
///
/// A game with zero rows at all is reported missing: the library hides it until
/// an archive exists again, but its metadata is preserved.
pub(crate) fn version_status(available: i64, missing: i64) -> &'static str {
    if available > 0 {
        if missing > 0 {
            "partially_missing"
        } else {
            "available"
        }
    } else {
        "missing"
    }
}

pub(crate) fn summary_from_list(row: GameListRow) -> GameSummary {
    GameSummary {
        id: row.id,
        title: row.title,
        sort_title: row.sort_title,
        release_date: row.release_date.as_deref().and_then(db::parse_time),
        cover_url: row.cover_path.as_deref().map(crate::media::media_url),
        background_url: row.background_path.as_deref().map(crate::media::media_url),
        rating: row.rating,
        genres: db::parse_json_list(&row.genres_json),
        early_access: row.early_access != 0,
        version_count: u32::try_from(row.version_count).unwrap_or(0),
        available_version_count: u32::try_from(row.available_version_count).unwrap_or(0),
        missing_version_count: u32::try_from(row.missing_version_count).unwrap_or(0),
        status: version_status(row.available_version_count, row.missing_version_count).to_string(),
        total_size: row.total_size,
        state: row.state.parse().unwrap_or_default(),
        minutes_played: row.minutes_played,
        bookmarked: row.bookmarked != 0,
        last_played_at: row.last_played_at.as_deref().and_then(db::parse_time),
        max_players: row.max_players.and_then(|value| u32::try_from(value).ok()),
        matched_field: None,
        matched_title: None,
        match_score: None,
        kind: row.kind,
        platform: row.platform,
        emulator_id: row.emulator_id,
    }
}

pub(crate) fn summary_from_detail(row: &GameDetailRow) -> GameSummary {
    GameSummary {
        id: row.id,
        title: row.title.clone(),
        sort_title: row.sort_title.clone(),
        release_date: row.release_date.as_deref().and_then(db::parse_time),
        cover_url: row.cover_path.as_deref().map(crate::media::media_url),
        background_url: row.background_path.as_deref().map(crate::media::media_url),
        rating: row.rating,
        genres: db::parse_json_list(&row.genres_json),
        early_access: row.early_access != 0,
        version_count: u32::try_from(row.version_count).unwrap_or(0),
        available_version_count: u32::try_from(row.available_version_count).unwrap_or(0),
        missing_version_count: u32::try_from(row.missing_version_count).unwrap_or(0),
        status: version_status(row.available_version_count, row.missing_version_count).to_string(),
        total_size: row.total_size,
        state: row.state.parse().unwrap_or_default(),
        minutes_played: row.minutes_played,
        bookmarked: row.bookmarked != 0,
        last_played_at: row.last_played_at.as_deref().and_then(db::parse_time),
        max_players: row.max_players.and_then(|value| u32::try_from(value).ok()),
        matched_field: None,
        matched_title: None,
        match_score: None,
        kind: row.kind.clone(),
        platform: row.platform.clone(),
        emulator_id: row.emulator_id,
    }
}
