//! Game library routes: browsing, metadata editing, progress, downloads.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use bytes::Bytes;
use chrono::Duration;
use futures_util::Stream;
use ludex_core::archive::{
    archive_tar_size, detect_format, format_from_extension, zip_entry_info, zip_stream_manifest,
    ArchiveError, ArchiveFormat, StreamManifest,
};
use ludex_core::models::{
    GameDetail, GameSummary, GameVersion, LatestVersion, LaunchInfo, Paginated, ProgressState,
    ProgressUpdate, ProviderRef, Stats, Trailer,
};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::api_keys::Scope;
use crate::auth::{authenticate_token, bearer_token, issue_download_ticket, AuthUser};
use crate::config::StorageBackend;
use crate::db;
use crate::download_limits::{parse_job_id, DownloadAdmission, LimitRejection, JOB_ID_HEADER};
use crate::error::ApiError;
use crate::extract::{ApiPath, ApiQuery, JsonBody};
use crate::media::content_disposition;
use crate::models::{
    BookmarkRequest, BookmarkResponse, DeleteVersionResponse, DownloadQuery, GameDetailRow,
    GameListRow, GameQuery, GameVersionInfo, GameVersionsResponse, InstalledReportResponse,
    LocalPathResponse, PlaytimeRequest, ProgressRequest, RefreshResponse, SetDefaultVersionRequest,
    StreamQuery, TicketRequest, TicketResponse, UpdateGameRequest, VersionRow,
};
use crate::roots;
use crate::sql::{bind_query, bind_query_as, bind_query_scalar, Bind};
use crate::state::AppState;
use crate::storage::{RequestRange, StorageError};
use crate::stream::{archive_tar_body, entry_body};

pub(crate) const LIST_COLUMNS: &str = "g.id, g.title, g.sort_title, g.release_date, g.cover_path, \
g.background_path, g.rating, g.genres_json, g.early_access, g.alt_titles_json, g.max_players, \
g.provider_slug, g.kind, g.platform, g.emulator_id, \
COALESCE(p.state, 'unplayed') AS state, COALESCE(p.minutes_played, 0) AS minutes_played, \
CAST(CASE WHEN b.user_id IS NULL THEN 0 ELSE 1 END AS BIGINT) AS bookmarked, p.last_played_at, \
(SELECT COUNT(*) FROM game_versions v WHERE v.game_id = g.id AND v.missing = 0 AND v.root_unavailable = 0) AS version_count, \
(SELECT COUNT(*) FROM game_versions v WHERE v.game_id = g.id AND v.missing = 0 AND v.root_unavailable = 0) AS available_version_count, \
(SELECT COUNT(*) FROM game_versions v WHERE v.game_id = g.id AND (v.missing = 1 OR v.root_unavailable = 1)) AS missing_version_count, \
(SELECT CAST(COALESCE(SUM(v.size), 0) AS BIGINT) FROM game_versions v WHERE v.game_id = g.id AND v.missing = 0 AND v.root_unavailable = 0) AS total_size";

const DETAIL_COLUMNS: &str = "g.id, g.title, g.sort_title, g.description, g.release_date, \
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

/// Game routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/games", get(list))
        .route("/games/random", get(random))
        .route(
            "/games/{id}",
            get(detail).patch(update_game).delete(delete_game),
        )
        .route("/games/{id}/refresh-metadata", post(refresh_metadata))
        .route("/games/{id}/download-ticket", post(download_ticket))
        .route("/games/{id}/download", get(download))
        .route("/games/{id}/versions", get(list_versions))
        .route("/games/{id}/versions/default", put(set_default_version))
        .route("/games/{id}/versions/{version_id}", delete(delete_version))
        .route("/games/{id}/versions/latest", get(latest_version))
        .route(
            "/games/{id}/versions/{version_id}/stream",
            get(stream_version),
        )
        .route(
            "/games/{id}/versions/{version_id}/stream/manifest",
            get(stream_manifest),
        )
        .route(
            "/games/{id}/versions/{version_id}/stream/entries/{index}",
            get(stream_entry),
        )
        .route(
            "/games/{id}/versions/{version_id}/local-path",
            get(local_path),
        )
        .route("/games/{id}/progress", put(set_progress))
        .route("/games/{id}/playtime", post(report_playtime))
        .route("/games/{id}/bookmark", put(set_bookmark))
        .route("/games/{id}/installed", post(report_installed))
        .route("/stats", get(stats))
}

struct Filters {
    clauses: Vec<String>,
    binds: Vec<Bind>,
    sort: &'static str,
    direction: &'static str,
}

impl Filters {
    fn from_query(query: &GameQuery, postgres: bool) -> Result<Self, ApiError> {
        let mut clauses = Vec::new();
        let mut binds = Vec::new();

        if let Some(genre) = query
            .genre
            .as_deref()
            .map(str::trim)
            .filter(|g| !g.is_empty())
        {
            clauses.push(if postgres {
                "EXISTS (SELECT 1 FROM jsonb_array_elements_text(g.genres_json::jsonb) AS genre \
                 WHERE genre = ?)"
                    .to_string()
            } else {
                "EXISTS (SELECT 1 FROM json_each(g.genres_json) WHERE value = ?)".to_string()
            });
            binds.push(Bind::Text(genre.to_string()));
        }
        if let Some(state) = query.state {
            clauses.push("COALESCE(p.state, 'unplayed') = ?".to_string());
            binds.push(Bind::Text(state.as_str().to_string()));
        }
        if let Some(bookmarked) = query.bookmarked {
            clauses.push(if bookmarked {
                "b.user_id IS NOT NULL".to_string()
            } else {
                "b.user_id IS NULL".to_string()
            });
        }
        if let Some(early_access) = query.early_access {
            clauses.push("g.early_access = ?".to_string());
            binds.push(Bind::Int(i64::from(early_access)));
        }
        if let Some(year) = query.year_from {
            clauses.push("g.release_date >= ?".to_string());
            binds.push(Bind::Text(format!("{year:04}-01-01T00:00:00Z")));
        }
        if let Some(year) = query.year_to {
            clauses.push("g.release_date <= ?".to_string());
            binds.push(Bind::Text(format!("{year:04}-12-31T23:59:59Z")));
        }
        if let Some(players) = query.players.filter(|value| *value > 0) {
            clauses.push("g.max_players >= ?".to_string());
            binds.push(Bind::Int(players));
        }
        if let Some(minutes) = query.playtime_min.filter(|value| *value > 0) {
            clauses.push("COALESCE(p.minutes_played, 0) >= ?".to_string());
            binds.push(Bind::Int(minutes));
        }
        if let Some(minutes) = query.playtime_max.filter(|value| *value >= 0) {
            clauses.push("COALESCE(p.minutes_played, 0) <= ?".to_string());
            binds.push(Bind::Int(minutes));
        }
        if let Some(rating) = query.min_rating {
            clauses.push("g.rating >= ?".to_string());
            binds.push(Bind::Float(rating));
        }
        if let Some(provider) = query
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            clauses.push("g.provider_slug = ?".to_string());
            binds.push(Bind::Text(provider.to_string()));
        }
        if let Some(platform) = query
            .platform
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            clauses.push("g.platform = ?".to_string());
            binds.push(Bind::Text(platform.to_string()));
        }
        if let Some(kind) = query
            .kind
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if kind != "game" && kind != "rom" {
                return Err(ApiError::bad_request(
                    "invalid_kind",
                    "kind must be `game` or `rom`",
                ));
            }
            clauses.push("g.kind = ?".to_string());
            binds.push(Bind::Text(kind.to_string()));
        }

        let sort = match query.sort.as_deref().unwrap_or("title") {
            "title" => "g.sort_title",
            "releaseDate" => "g.release_date",
            "rating" => "g.rating",
            "addedAt" => "g.created_at",
            "lastPlayed" => "p.last_played_at",
            "playtime" => "COALESCE(p.minutes_played, 0)",
            other => {
                return Err(ApiError::bad_request(
                    "invalid_sort",
                    format!("Unknown sort column: {other}"),
                ));
            }
        };
        let direction = match query.order.as_deref().unwrap_or("asc") {
            "asc" => "ASC",
            "desc" => "DESC",
            other => {
                return Err(ApiError::bad_request(
                    "invalid_order",
                    format!("Unknown sort order: {other}"),
                ));
            }
        };

        Ok(Self {
            clauses,
            binds,
            sort,
            direction,
        })
    }

    /// Availability filter (#210).
    ///
    /// The default hides games whose archives are all missing; the row stays in
    /// the database and a rescan re-lists it as soon as the file is back. A
    /// deep link to `/game/{id}` still resolves: this only filters lists.
    fn with_availability(mut self, status: Option<&str>) -> Result<Self, ApiError> {
        let available = "(SELECT COUNT(*) FROM game_versions v WHERE v.game_id = g.id \
                         AND v.missing = 0 AND v.root_unavailable = 0)";
        let missing = "(SELECT COUNT(*) FROM game_versions v WHERE v.game_id = g.id \
                       AND (v.missing = 1 OR v.root_unavailable = 1))";
        match status.map(str::trim).unwrap_or("available") {
            "" | "available" => self.clauses.push(format!("{available} > 0")),
            "missing" => self.clauses.push(format!("{available} = 0")),
            "partial" => {
                self.clauses.push(format!("{available} > 0"));
                self.clauses.push(format!("{missing} > 0"));
            }
            "all" => {}
            other => {
                return Err(ApiError::bad_request(
                    "invalid_status",
                    format!("Unknown status filter: {other} (available, missing, partial, all)"),
                ));
            }
        }
        Ok(self)
    }

    /// Hides games above the account's age limit (#39). Unrated games stay
    /// visible: an unknown rating is not evidence of adult content.
    fn with_age_limit(mut self, limit: Option<u8>) -> Self {
        if let Some(limit) = limit {
            self.clauses
                .push("(g.age_rating IS NULL OR g.age_rating <= ?)".to_string());
            self.binds.push(Bind::Int(i64::from(limit)));
        }
        self
    }

    fn where_clause(&self) -> String {
        if self.clauses.is_empty() {
            " WHERE 1=1".to_string()
        } else {
            format!(" WHERE {}", self.clauses.join(" AND "))
        }
    }

    fn order_clause(&self) -> String {
        format!(
            " ORDER BY ({} IS NULL) ASC, {} {}, g.id ASC",
            self.sort, self.sort, self.direction
        )
    }
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<GameQuery>,
) -> Result<Json<Paginated<GameSummary>>, ApiError> {
    let filters = Filters::from_query(&query, state.pool.is_postgres())?
        .with_availability(query.status.as_deref())?
        .with_age_limit(user.age_limit);
    let page = query.page.unwrap_or(1).max(1);
    let limit = query.limit.unwrap_or(48).clamp(1, 200);
    let offset = (page - 1).saturating_mul(limit);

    if let Some(search) = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        let sql = format!(
            "SELECT {LIST_COLUMNS}{FROM_JOINS}{} LIMIT 5000",
            filters.where_clause()
        );
        let mut binds = vec![Bind::Int(user.id), Bind::Int(user.id)];
        binds.extend(filters.binds.iter().cloned());
        let rows = bind_query_as(db::query_as::<GameListRow>(&sql), &binds)
            .fetch_all(&state.pool)
            .await
            .map_err(ApiError::from)?;

        let mut ranked: Vec<(GameListRow, crate::routes::search::TitleMatch)> = rows
            .into_iter()
            .filter_map(|row| {
                let alt_titles = db::parse_json_list(&row.alt_titles_json);
                let found = crate::routes::search::match_game(
                    search,
                    &row.title,
                    &row.sort_title,
                    &alt_titles,
                )?;
                Some((row, found))
            })
            .collect();
        ranked.sort_by(|a, b| {
            b.1.score
                .partial_cmp(&a.1.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.sort_title.cmp(&b.0.sort_title))
        });
        let total = i64::try_from(ranked.len()).unwrap_or(i64::MAX);
        let items = ranked
            .into_iter()
            .skip(usize::try_from(offset).unwrap_or(usize::MAX))
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .map(|(row, found)| {
                let mut summary = summary_from_list(row);
                summary.matched_field = Some(found.field.to_string());
                summary.matched_title = Some(found.text);
                summary.match_score = Some((found.score * 10.0).round() / 10.0);
                summary
            })
            .collect();
        return Ok(Json(Paginated {
            items,
            total,
            page,
            limit,
        }));
    }

    let count_sql = format!("SELECT COUNT(*){FROM_JOINS}{}", filters.where_clause());
    let mut count_binds = vec![Bind::Int(user.id), Bind::Int(user.id)];
    count_binds.extend(filters.binds.iter().cloned());
    let total = bind_query_scalar(db::query_scalar::<i64>(&count_sql), &count_binds)
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)?;

    let sql = format!(
        "SELECT {LIST_COLUMNS}{FROM_JOINS}{}{} LIMIT ? OFFSET ?",
        filters.where_clause(),
        filters.order_clause()
    );
    let mut binds = vec![Bind::Int(user.id), Bind::Int(user.id)];
    binds.extend(filters.binds.iter().cloned());
    binds.push(Bind::Int(limit));
    binds.push(Bind::Int(offset));

    let rows = bind_query_as(db::query_as::<GameListRow>(&sql), &binds)
        .fetch_all(&state.pool)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(Paginated {
        items: rows.into_iter().map(summary_from_list).collect(),
        total,
        page,
        limit,
    }))
}

async fn random(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<GameQuery>,
) -> Result<Json<GameSummary>, ApiError> {
    let filters = Filters::from_query(&query, state.pool.is_postgres())?
        .with_availability(query.status.as_deref())?
        .with_age_limit(user.age_limit);
    let mut binds = vec![Bind::Int(user.id), Bind::Int(user.id)];
    binds.extend(filters.binds.iter().cloned());

    if let Some(search) = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        let sql = format!(
            "SELECT {LIST_COLUMNS}{FROM_JOINS}{} LIMIT 5000",
            filters.where_clause()
        );
        let rows = bind_query_as(db::query_as::<GameListRow>(&sql), &binds)
            .fetch_all(&state.pool)
            .await
            .map_err(ApiError::from)?;
        let mut matches: Vec<GameListRow> = rows
            .into_iter()
            .filter(|row| {
                let alt_titles = db::parse_json_list(&row.alt_titles_json);
                crate::routes::search::match_game(search, &row.title, &row.sort_title, &alt_titles)
                    .is_some()
            })
            .collect();
        if matches.is_empty() {
            return Err(ApiError::not_found(
                "no_game",
                "No game matches these filters",
            ));
        }
        let index = rand::Rng::gen_range(&mut rand::thread_rng(), 0..matches.len());
        return Ok(Json(summary_from_list(matches.swap_remove(index))));
    }

    let sql = format!(
        "SELECT {LIST_COLUMNS}{FROM_JOINS}{} ORDER BY RANDOM() LIMIT 1",
        filters.where_clause()
    );
    let row = bind_query_as(db::query_as::<GameListRow>(&sql), &binds)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("no_game", "No game matches these filters"))?;
    Ok(Json(summary_from_list(row)))
}

async fn detail(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<GameDetail>, ApiError> {
    let game = fetch_detail(&state, user.id, id).await?;
    if age_blocked(user.age_limit, game.age_rating) {
        deny_age_restricted(&state, &user, id);
        return Err(age_not_found());
    }
    Ok(Json(game))
}

pub(crate) async fn fetch_detail(
    state: &AppState,
    user_id: i64,
    id: i64,
) -> Result<GameDetail, ApiError> {
    let sql = format!("SELECT {DETAIL_COLUMNS}{FROM_JOINS} WHERE g.id = ?");
    let row = bind_query_as(
        db::query_as::<GameDetailRow>(&sql),
        &[Bind::Int(user_id), Bind::Int(user_id), Bind::Int(id)],
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("game_not_found", "Game not found"))?;

    // Every row is loaded so the page can explain a vanished archive (#210).
    // Only available ones are offered for download.
    let versions = db::query_as::<VersionRow>(
        "SELECT id, root_id, file_path, file_name, size, version, release_date,
                early_access, missing, root_unavailable, archive_hash, content_sha256,
                indexed_at, updated_at, storage_backend, object_key, etag
         FROM game_versions WHERE game_id = ?
         ORDER BY COALESCE(release_date, indexed_at) DESC, id DESC",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let mut available_versions = Vec::new();
    let mut missing_versions = Vec::new();
    for version in versions {
        let (archive_format, stream_supported) = version_transport(&version.file_name);
        let row = GameVersion {
            id: version.id,
            file_name: version.file_name,
            size: version.size,
            version: version.version,
            release_date: version.release_date.as_deref().and_then(db::parse_time),
            early_access: version.early_access != 0,
            archive_hash: version.archive_hash,
            content_sha256: version.content_sha256,
            archive_format,
            stream_supported,
            updated_at: version.updated_at.as_deref().and_then(db::parse_time),
            missing: version.missing != 0,
            root_unavailable: version.root_unavailable != 0,
            download_url: format!("/api/games/{id}/download?versionId={}", version.id),
        };
        if row.missing || row.root_unavailable {
            missing_versions.push(row);
        } else {
            available_versions.push(row);
        }
    }

    let screenshots = db::parse_json_list(&row.screenshots_json)
        .iter()
        .map(|name| crate::media::media_url(name))
        .collect();
    let trailers = serde_json::from_str::<Vec<Trailer>>(&row.trailers_json).unwrap_or_default();
    let provider = match (&row.provider_slug, &row.provider_id) {
        (Some(slug), Some(provider_id)) if !slug.is_empty() && !provider_id.is_empty() => {
            Some(ProviderRef {
                slug: slug.clone(),
                id: provider_id.clone(),
                url: row.provider_url.clone(),
            })
        }
        _ => None,
    };
    let launch = LaunchInfo {
        executable: row.launch_executable.clone(),
        parameters: row.launch_parameters.clone(),
        installer_executable: row.installer_executable.clone(),
        installer_parameters: row.installer_parameters.clone(),
        uninstaller_executable: row.uninstaller_executable.clone(),
        uninstaller_parameters: row.uninstaller_parameters.clone(),
    };

    Ok(GameDetail {
        summary: summary_from_detail(&row),
        description: row.description.clone(),
        screenshots,
        trailers: trailers.clone(),
        videos: trailers,
        developers: db::parse_json_list(&row.developers_json),
        publishers: db::parse_json_list(&row.publishers_json),
        tags: db::parse_json_list(&row.tags_json),
        age_rating: row.age_rating.and_then(|value| u8::try_from(value).ok()),
        provider,
        alt_titles: db::parse_json_list(&row.alt_titles_json),
        versions: available_versions,
        missing_versions,
        default_version_id: row.default_version_id,
        launch,
    })
}

/// Version clients should update to: the game's pinned version (#168), else the
/// newest available one. The payload carries the metadata desktop clients need
/// to detect updates (version label, archive fingerprint, size, last update).
async fn latest_version(
    State(state): State<AppState>,
    _user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<LatestVersion>, ApiError> {
    ensure_game(&state, id).await?;

    let version_id = preferred_version_id(&state, id).await.map_err(|error| {
        if error.code == "version_not_found" {
            ApiError::not_found("no_versions", "This game has no available version")
        } else {
            error
        }
    })?;
    let row = fetch_version(&state, id, version_id).await?;

    Ok(Json(LatestVersion {
        game_id: id,
        version_id: row.id,
        version_label: row.version,
        file_name: row.file_name,
        archive_hash: row.archive_hash,
        content_sha256: row.content_sha256,
        archive_size: row.size,
        updated_at: row.updated_at.as_deref().and_then(db::parse_time),
        early_access: row.early_access != 0,
        download_url: format!("/api/games/{id}/download?versionId={}", row.id),
    }))
}

/// Every version of a game with its archive state, for the editor version
/// manager (#168). Missing rows are listed so an editor can see what happened
/// to an archive instead of watching it disappear.
async fn list_versions(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<GameVersionsResponse>, ApiError> {
    user.require_editor()?;
    ensure_game(&state, id).await?;
    Ok(Json(fetch_versions(&state, id).await?))
}

/// Pins the version used by downloads, fresh installs and the update check.
/// Picking an older version is a rollback; newer archives stay available.
async fn set_default_version(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    JsonBody(request): JsonBody<SetDefaultVersionRequest>,
) -> Result<Json<GameVersionsResponse>, ApiError> {
    user.require_editor()?;
    ensure_game(&state, id).await?;

    let version = fetch_version(&state, id, request.version_id).await?;
    if version.missing != 0 {
        return Err(ApiError::conflict(
            "version_missing",
            "This archive is missing; restore it before making it the default version",
        ));
    }
    if version.root_unavailable != 0 {
        return Err(ApiError::conflict(
            "root_unavailable",
            "The library root holding this archive is offline",
        ));
    }

    db::query("UPDATE games SET default_version_id = ?, updated_at = ? WHERE id = ?")
        .bind(version.id)
        .bind(db::db_time(db::now()))
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;

    state.events.emit(
        crate::events::Event::new(
            crate::audit::actions::GAME_VERSION_DEFAULT_CHANGED,
            user.actor(),
        )
        .target("game", id.to_string())
        .metadata(serde_json::json!({
            "versionId": version.id,
            "version": version.version,
            "fileName": version.file_name,
        })),
    );

    Ok(Json(fetch_versions(&state, id).await?))
}

/// Deletes one archive (file + row) after an explicit confirmation (#168).
///
/// The game, its metadata, saves and audit history are kept; only the archive
/// goes away. The last available version is never deleted, nor one that is
/// missing, on an offline root, or currently being downloaded/streamed.
async fn delete_version(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath((game_id, version_id)): ApiPath<(i64, i64)>,
    ApiQuery(query): ApiQuery<DeleteVersionQuery>,
) -> Result<Json<DeleteVersionResponse>, ApiError> {
    user.require_editor()?;
    ensure_game(&state, game_id).await?;
    if !query.confirm.unwrap_or(false) {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "Pass confirm=true to delete this archive; the game itself is kept",
        ));
    }

    let version = fetch_version(&state, game_id, version_id).await?;
    let remaining = db::query_scalar::<i64>(
        "SELECT COUNT(*) FROM game_versions
         WHERE game_id = ? AND id != ? AND missing = 0 AND root_unavailable = 0",
    )
    .bind(game_id)
    .bind(version_id)
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;
    ensure_version_deletable(&state, &version, remaining)?;

    // Remove the archive bytes first: a row without a file would be re-marked
    // missing by the next scan, while a file without a row is re-indexed.
    if is_s3_version(&version) {
        if let Some(key) = version.object_key.as_deref() {
            state
                .storage
                .delete(key)
                .await
                .map_err(|error| map_storage_error(error, "delete"))?;
        }
    }
    if version.root_unavailable == 0 {
        if let Some(path) = roots::resolve_version_path(
            &state.pool,
            &state.config,
            version.root_id,
            &version.file_path,
        )
        .await
        {
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    tracing::error!(%error, version = version.id, "could not delete the archive");
                    return Err(ApiError::internal());
                }
            }
        }
    }

    let default_changed = game_default_version_id(&state, game_id).await? == Some(version.id);
    let replacement = if default_changed {
        newest_other_version(&state, game_id, version.id).await?
    } else {
        None
    };

    let mut transaction = state.pool.begin().await.map_err(ApiError::from)?;
    db::query("DELETE FROM game_versions WHERE id = ? AND game_id = ?")
        .bind(version.id)
        .bind(game_id)
        .execute(&mut transaction)
        .await
        .map_err(ApiError::from)?;
    if default_changed {
        db::query("UPDATE games SET default_version_id = ?, updated_at = ? WHERE id = ?")
            .bind(replacement)
            .bind(db::db_time(db::now()))
            .bind(game_id)
            .execute(&mut transaction)
            .await
            .map_err(ApiError::from)?;
    }
    transaction.commit().await.map_err(ApiError::from)?;

    state.events.emit(
        crate::events::Event::new(crate::audit::actions::GAME_VERSION_DELETED, user.actor())
            .target("game", game_id.to_string())
            .metadata(serde_json::json!({
                "versionId": version.id,
                "version": version.version,
                "fileName": version.file_name,
                "size": version.size,
                "storageBackend": version.storage_backend,
            })),
    );

    Ok(Json(DeleteVersionResponse {
        version_id: version.id,
        freed_bytes: version.size,
        default_version_id: game_default_version_id(&state, game_id).await?,
    }))
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteVersionQuery {
    #[serde(default)]
    confirm: Option<bool>,
}

/// Guards shared by the delete route and its unit tests.
fn ensure_version_deletable(
    state: &AppState,
    version: &VersionRow,
    remaining: i64,
) -> Result<(), ApiError> {
    if version.missing != 0 {
        return Err(ApiError::conflict(
            "version_missing",
            "This archive is already missing; the row is kept for history",
        ));
    }
    if version.root_unavailable != 0 {
        return Err(ApiError::conflict(
            "root_unavailable",
            "The library root holding this archive is offline; restore it before deleting",
        ));
    }
    if state.transfers.is_active(version.id) {
        return Err(ApiError::conflict(
            "version_in_use",
            "A download or stream is using this archive; stop it first",
        ));
    }
    if remaining <= 0 {
        return Err(ApiError::conflict(
            "last_version",
            "This is the last available version; Ludex never deletes it",
        ));
    }
    Ok(())
}

/// Which half of a game `DELETE /api/games/{id}` removes (#209, #210).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeleteMode {
    /// Delete the database rows only; archives stay on disk and a scan can
    /// re-index them. Editor operation, audited.
    Library,
    /// Delete the archive files below the resolved library root, then the
    /// database rows. Administrator operation, audited.
    Archives,
}

impl DeleteMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::Archives => "archives",
        }
    }

    pub(crate) fn parse(raw: Option<&str>) -> Result<Self, ApiError> {
        match raw.map(str::trim) {
            Some("library") => Ok(Self::Library),
            Some("archives") => Ok(Self::Archives),
            Some(other) => Err(ApiError::bad_request(
                "invalid_mode",
                format!("Unknown delete mode: {other} (library or archives)"),
            )),
            None => Err(ApiError::bad_request(
                "mode_required",
                "Pass mode=library (keep the files) or mode=archives (delete them)",
            )),
        }
    }
}

/// `DELETE /api/games/{id}` query.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeleteGameQuery {
    #[serde(default)]
    pub(crate) mode: Option<String>,
    /// Legacy spelling of `mode=library` (#209).
    #[serde(default)]
    pub(crate) keep_files: Option<bool>,
    #[serde(default)]
    pub(crate) confirm: Option<bool>,
}

impl DeleteGameQuery {
    pub(crate) fn mode(&self) -> Result<DeleteMode, ApiError> {
        if self.mode.is_none() {
            if let Some(keep) = self.keep_files {
                return Ok(if keep {
                    DeleteMode::Library
                } else {
                    DeleteMode::Archives
                });
            }
        }
        DeleteMode::parse(self.mode.as_deref())
    }
}

/// Result of removing one game from the library.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeleteGameResponse {
    /// Game row that was removed.
    pub game_id: i64,
    /// `library` (rows only) or `archives` (files first).
    pub mode: String,
    /// Versions that existed on the row.
    pub versions: i64,
    /// Versions already marked missing or on an offline root.
    pub missing_versions: i64,
    /// Archive files actually unlinked by this call.
    pub deleted_files: i64,
    /// Bytes reclaimed by the unlinks.
    pub freed_bytes: i64,
    /// Archive files intentionally left on disk.
    pub kept_files: i64,
}

async fn delete_game(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    ApiQuery(query): ApiQuery<DeleteGameQuery>,
) -> Result<Json<DeleteGameResponse>, ApiError> {
    let mode = query.mode()?;
    match mode {
        DeleteMode::Archives => user.require_admin_access()?,
        DeleteMode::Library => user.require_editor()?,
    }
    if !query.confirm.unwrap_or(false) {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "Pass confirm=true to remove this game; the response says what was deleted and what was kept",
        ));
    }
    Ok(Json(remove_game(&state, &user, id, mode).await?))
}

/// Shared by the single-game route and the admin bulk endpoint.
///
/// Files are removed before rows: a row without a file would be re-marked
/// missing by the next scan, while an orphan file is harmless and re-indexable.
pub(crate) async fn remove_game(
    state: &AppState,
    user: &AuthUser,
    game_id: i64,
    mode: DeleteMode,
) -> Result<DeleteGameResponse, ApiError> {
    let Some((title, _default_version)) = db::query_as::<(String, Option<i64>)>(
        "SELECT title, default_version_id FROM games WHERE id = ?",
    )
    .bind(game_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    else {
        return Err(ApiError::not_found("game_not_found", "Game not found"));
    };

    let versions = db::query_as::<VersionRow>(
        "SELECT id, root_id, file_path, file_name, size, version, release_date,
                early_access, missing, root_unavailable, archive_hash, content_sha256,
                indexed_at, updated_at, storage_backend, object_key, etag
         FROM game_versions WHERE game_id = ? ORDER BY id",
    )
    .bind(game_id)
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let mut deleted_files = 0_i64;
    let mut freed_bytes = 0_i64;
    let missing_versions = versions
        .iter()
        .filter(|version| version.missing != 0 || version.root_unavailable != 0)
        .count() as i64;

    if mode == DeleteMode::Archives {
        // An offline root cannot be verified: deleting rows would leave files
        // behind with no way to find them again.
        if versions.iter().any(|version| version.root_unavailable != 0) {
            return Err(ApiError::conflict(
                "root_unavailable",
                "A library root holding this game is offline; restore it before deleting the archives",
            ));
        }
        if versions
            .iter()
            .any(|version| state.transfers.is_active(version.id))
        {
            return Err(ApiError::conflict(
                "version_in_use",
                "A download or stream is using an archive of this game; stop it first",
            ));
        }
        for version in &versions {
            if is_s3_version(version) {
                if let Some(key) = version.object_key.as_deref() {
                    match state.storage.delete(key).await {
                        Ok(()) => {
                            deleted_files += 1;
                            freed_bytes += version.size;
                        }
                        Err(StorageError::NotFound) => {}
                        Err(error) => return Err(map_storage_error(error, "delete")),
                    }
                }
                continue;
            }
            if delete_archive_file(state, version).await? {
                deleted_files += 1;
                freed_bytes += version.size;
            }
        }
    }

    delete_game_rows(state, game_id).await?;

    state.events.emit(
        crate::events::Event::new(crate::audit::actions::GAME_DELETED, user.actor())
            .target("game", game_id.to_string())
            .metadata(serde_json::json!({
                "title": title,
                "mode": mode.as_str(),
                "versions": versions.len(),
                "deletedFiles": deleted_files,
                "freedBytes": freed_bytes,
                "missingVersions": missing_versions,
                "files": versions
                    .iter()
                    .filter(|version| version.missing == 0 && version.root_unavailable == 0)
                    .map(|version| version.file_path.as_str())
                    .collect::<Vec<_>>(),
            })),
    );

    Ok(DeleteGameResponse {
        game_id,
        mode: mode.as_str().to_string(),
        versions: versions.len() as i64,
        missing_versions,
        deleted_files,
        freed_bytes,
        kept_files: versions.len() as i64 - deleted_files,
    })
}

/// Unlinks one archive below its library root.
///
/// `Ok(false)` means the file was already gone (idempotent). A symlink, a path
/// that escapes the root, or a canonical target outside it is refused with
/// `unsafe_path` and nothing is deleted.
async fn delete_archive_file(state: &AppState, version: &VersionRow) -> Result<bool, ApiError> {
    let base = roots::base_dir(&state.pool, &state.config, version.root_id)
        .await
        .map_err(|_| ApiError::internal())?;
    if !base.is_dir() {
        return Err(ApiError::conflict(
            "root_unavailable",
            "The library root holding this archive is offline; restore it before deleting",
        ));
    }
    let Some(path) = crate::scanner::resolve_relative(&base, &version.file_path) else {
        return Err(ApiError::conflict(
            "unsafe_path",
            "The archive path escapes the library root; refusing to delete it",
        ));
    };
    match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(ApiError::conflict(
                "unsafe_path",
                "The archive path is a symlink; refusing to follow it",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            tracing::error!(%error, version = version.id, "could not stat the archive");
            return Err(ApiError::internal());
        }
    }
    let root = tokio::fs::canonicalize(&base)
        .await
        .map_err(|_| ApiError::internal())?;
    let file = tokio::fs::canonicalize(&path)
        .await
        .map_err(|_| ApiError::internal())?;
    if !file.starts_with(&root) {
        return Err(ApiError::conflict(
            "unsafe_path",
            "The archive resolves outside the library root; refusing to delete it",
        ));
    }
    match tokio::fs::remove_file(&file).await {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => {
            tracing::error!(%error, version = version.id, "could not delete the archive");
            Err(ApiError::internal())
        }
    }
}

/// Removes the game and every row that belongs to it.
///
/// Cascade: versions, progress, bookmarks, play sessions, collection entries,
/// share links, achievements and their unlocks, achievement sources/scans,
/// cloud-save snapshots and remote-install requests. Kept: the activity feed
/// and notifications (their `game_id` becomes null), audit logs, health
/// findings, uploads and provider fetches (historical reports).
pub(crate) async fn delete_game_rows(state: &AppState, game_id: i64) -> Result<(), ApiError> {
    let mut transaction = state.pool.begin().await.map_err(ApiError::from)?;
    for sql in [
        "DELETE FROM game_versions WHERE game_id = ?",
        "DELETE FROM achievements WHERE game_id = ?",
        "DELETE FROM achievement_sources WHERE game_id = ?",
        "DELETE FROM achievement_scans WHERE game_id = ?",
        "DELETE FROM collection_games WHERE game_id = ?",
        "DELETE FROM progress WHERE game_id = ?",
        "DELETE FROM bookmarks WHERE game_id = ?",
        "DELETE FROM play_sessions WHERE game_id = ?",
        "DELETE FROM save_snapshots WHERE game_id = ?",
        "DELETE FROM share_links WHERE game_id = ?",
        "DELETE FROM remote_installs WHERE game_id = ?",
        "DELETE FROM games WHERE id = ?",
    ] {
        db::query(sql)
            .bind(game_id)
            .execute(&mut transaction)
            .await
            .map_err(ApiError::from)?;
    }
    transaction.commit().await.map_err(ApiError::from)?;
    Ok(())
}

/// Transport hints for a version, derived from its file name only (#226).
///
/// `stream_supported` mirrors the server-side stream gate: the per-entry
/// manifest exists for ZIP only. A 7z/RAR/unknown archive must be downloaded
/// whole and extracted; the payload still carries the format so clients can
/// say so before the first byte.
fn version_transport(file_name: &str) -> (Option<String>, bool) {
    let format = format_from_extension(Path::new(file_name));
    (
        format.extension().map(str::to_string),
        format == ArchiveFormat::Zip,
    )
}

/// All versions (including missing rows), newest first, with the pinned id.
async fn fetch_versions(state: &AppState, game_id: i64) -> Result<GameVersionsResponse, ApiError> {
    let rows = db::query_as::<VersionRow>(
        "SELECT id, root_id, file_path, file_name, size, version, release_date,
                early_access, missing, root_unavailable, archive_hash, content_sha256,
                indexed_at, updated_at, storage_backend, object_key, etag
         FROM game_versions WHERE game_id = ?
         ORDER BY COALESCE(release_date, indexed_at) DESC, id DESC",
    )
    .bind(game_id)
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;

    Ok(GameVersionsResponse {
        game_id,
        default_version_id: game_default_version_id(state, game_id).await?,
        versions: rows
            .into_iter()
            .map(|row| {
                let (archive_format, stream_supported) = version_transport(&row.file_name);
                GameVersionInfo {
                    id: row.id,
                    file_name: row.file_name,
                    size: row.size,
                    version: row.version,
                    release_date: row.release_date.as_deref().and_then(db::parse_time),
                    early_access: row.early_access != 0,
                    archive_hash: row.archive_hash,
                    content_sha256: row.content_sha256,
                    archive_format,
                    stream_supported,
                    indexed_at: row.indexed_at.as_deref().and_then(db::parse_time),
                    updated_at: row.updated_at.as_deref().and_then(db::parse_time),
                    missing: row.missing != 0,
                    root_unavailable: row.root_unavailable != 0,
                    download_url: format!("/api/games/{game_id}/download?versionId={}", row.id),
                }
            })
            .collect(),
    })
}

/// Pinned version id from `games.default_version_id`, if any.
async fn game_default_version_id(state: &AppState, game_id: i64) -> Result<Option<i64>, ApiError> {
    Ok(
        db::query_scalar::<Option<i64>>("SELECT default_version_id FROM games WHERE id = ?")
            .bind(game_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(ApiError::from)?
            .flatten(),
    )
}

async fn update_game(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    JsonBody(request): JsonBody<UpdateGameRequest>,
) -> Result<Json<GameDetail>, ApiError> {
    user.require_editor()?;
    ensure_game(&state, id).await?;

    let mut sets = Vec::new();
    let mut binds = Vec::new();
    let mut manual_fields: Vec<&str> = Vec::new();

    if let Some(title) = request.title {
        let title = title.trim();
        if title.is_empty() {
            return Err(ApiError::bad_request(
                "invalid_title",
                "Title cannot be empty",
            ));
        }
        sets.push("title = ?");
        binds.push(Bind::Text(title.to_string()));
        manual_fields.push("title");
        if request.sort_title.is_none() {
            let derived = ludex_core::naming::parse_filename(&format!("{title}.zip")).sort_title;
            sets.push("sort_title = ?");
            binds.push(Bind::Text(if derived.trim().is_empty() {
                title.to_string()
            } else {
                derived
            }));
        }
    }
    if let Some(sort_title) = request.sort_title {
        let sort_title = sort_title.trim();
        if sort_title.is_empty() {
            return Err(ApiError::bad_request(
                "invalid_sort_title",
                "Sort title cannot be empty",
            ));
        }
        sets.push("sort_title = ?");
        binds.push(Bind::Text(sort_title.to_string()));
    }
    if let Some(description) = request.description {
        manual_fields.push("description");
        sets.push("description = ?");
        binds.push(Bind::OptText(
            description
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        ));
    }
    if let Some(release_date) = request.release_date {
        manual_fields.push("releaseDate");
        let parsed = match release_date {
            Some(value) => Some(
                db::parse_date(&value)
                    .ok_or_else(|| ApiError::bad_request("invalid_date", "Invalid release date"))?,
            ),
            None => None,
        };
        sets.push("release_date = ?");
        binds.push(Bind::OptText(parsed.map(db::db_time)));
    }
    if let Some(genres) = request.genres {
        manual_fields.push("genres");
        sets.push("genres_json = ?");
        binds.push(Bind::Text(db::dump_json_list(&clean_labels(genres))));
    }
    if let Some(developers) = request.developers {
        manual_fields.push("developers");
        sets.push("developers_json = ?");
        binds.push(Bind::Text(db::dump_json_list(&clean_labels(developers))));
    }
    if let Some(publishers) = request.publishers {
        manual_fields.push("publishers");
        sets.push("publishers_json = ?");
        binds.push(Bind::Text(db::dump_json_list(&clean_labels(publishers))));
    }
    if let Some(tags) = request.tags {
        sets.push("tags_json = ?");
        binds.push(Bind::Text(db::dump_json_list(&clean_labels(tags))));
    }
    if let Some(rating) = request.rating {
        manual_fields.push("rating");
        sets.push("rating = ?");
        binds.push(Bind::OptFloat(rating.map(|value| value.clamp(0.0, 100.0))));
    }
    if let Some(age_rating) = request.age_rating {
        manual_fields.push("ageRating");
        sets.push("age_rating = ?");
        binds.push(Bind::OptInt(age_rating.map(i64::from)));
    }
    if let Some(early_access) = request.early_access {
        sets.push("early_access = ?");
        binds.push(Bind::Int(i64::from(early_access)));
    }
    if let Some(alt_titles) = request.alt_titles {
        manual_fields.push("altTitles");
        sets.push("alt_titles_json = ?");
        binds.push(Bind::Text(db::dump_json_list(&clean_labels(alt_titles))));
    }
    if let Some(players) = request.players {
        manual_fields.push("players");
        sets.push("max_players = ?");
        binds.push(Bind::OptInt(players.map(i64::from)));
    }
    if let Some(launch) = request.launch {
        sets.push("launch_executable = ?");
        binds.push(Bind::OptText(launch.executable));
        sets.push("launch_parameters = ?");
        binds.push(Bind::OptText(launch.parameters));
        sets.push("installer_executable = ?");
        binds.push(Bind::OptText(launch.installer_executable));
        sets.push("installer_parameters = ?");
        binds.push(Bind::OptText(launch.installer_parameters));
        sets.push("uninstaller_executable = ?");
        binds.push(Bind::OptText(launch.uninstaller_executable));
        sets.push("uninstaller_parameters = ?");
        binds.push(Bind::OptText(launch.uninstaller_parameters));
    }

    if !sets.is_empty() {
        sets.push("updated_at = ?");
        binds.push(Bind::Text(db::db_time(db::now())));
        binds.push(Bind::Int(id));
        let sql = format!("UPDATE games SET {} WHERE id = ?", sets.join(", "));
        bind_query(db::query(&sql), &binds)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?;
    }

    // Remember which metadata fields were edited by hand so a later provider
    // match can preserve them unless the editor asks to replace (#135).
    if !manual_fields.is_empty() {
        merge_manual_fields(&state, id, &manual_fields).await?;
    }

    Ok(Json(fetch_detail(&state, user.id, id).await?))
}

/// Adds `fields` to `games.manual_fields_json`, keeping existing markers.
pub(crate) async fn merge_manual_fields(
    state: &AppState,
    game_id: i64,
    fields: &[&str],
) -> Result<(), ApiError> {
    let existing = db::query_scalar::<String>("SELECT manual_fields_json FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?
        .unwrap_or_default();
    let mut list = db::parse_json_list(&existing);
    for field in fields {
        if !list.iter().any(|marked| marked == field) {
            list.push((*field).to_string());
        }
    }
    db::query("UPDATE games SET manual_fields_json = ? WHERE id = ?")
        .bind(db::dump_json_list(&list))
        .bind(game_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    Ok(())
}

async fn refresh_metadata(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<(StatusCode, Json<RefreshResponse>), ApiError> {
    user.require_editor()?;
    ensure_game(&state, id).await?;
    let state = state.clone();
    tokio::spawn(async move {
        crate::metadata::enrich_game(&state, id).await;
    });
    Ok((StatusCode::ACCEPTED, Json(RefreshResponse { queued: true })))
}

async fn download_ticket(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    JsonBody(request): JsonBody<TicketRequest>,
) -> Result<Json<TicketResponse>, ApiError> {
    ensure_game(&state, game_id).await?;
    if age_blocked(user.age_limit, game_age_rating(&state, game_id).await?) {
        deny_age_restricted(&state, &user, game_id);
        return Err(age_not_found());
    }
    let version_id = match request.version_id {
        Some(version_id) => {
            let exists = db::query_scalar::<i64>(
                "SELECT id FROM game_versions WHERE id = ? AND game_id = ? AND missing = 0",
            )
            .bind(version_id)
            .bind(game_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(ApiError::from)?;
            exists
                .ok_or_else(|| ApiError::not_found("version_not_found", "Game version not found"))?
        }
        None => preferred_version_id(&state, game_id).await?,
    };

    let (token, expires_at) =
        issue_download_ticket(&state.jwt_secret, user.id, game_id, version_id)?;
    Ok(Json(TicketResponse {
        url: format!("/api/games/{game_id}/download?versionId={version_id}&token={token}"),
        expires_at,
    }))
}

/// Whether a game's age rating exceeds the account's limit (#39).
fn age_blocked(limit: Option<u8>, rating: Option<u8>) -> bool {
    matches!((limit, rating), (Some(limit), Some(rating)) if rating > limit)
}

/// A restricted game is indistinguishable from a missing one.
fn age_not_found() -> ApiError {
    ApiError::not_found("game_not_found", "Game not found")
}

/// Records an age-restricted access attempt without leaking the game to the
/// caller.
fn deny_age_restricted(state: &AppState, user: &AuthUser, game_id: i64) {
    state.events.emit(
        crate::events::Event::new(crate::audit::actions::GAME_ACCESS_DENIED, user.actor())
            .target("game", game_id.to_string())
            .metadata(serde_json::json!({
                "reason": "age_restricted",
                "ageLimit": user.age_limit,
            })),
    );
}

/// Reads a game's age rating, if any.
async fn game_age_rating(state: &AppState, game_id: i64) -> Result<Option<u8>, ApiError> {
    let rating = db::query_scalar::<Option<i64>>("SELECT age_rating FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?
        .flatten();
    Ok(rating.and_then(|value| u8::try_from(value).ok()))
}

async fn newest_version(state: &AppState, game_id: i64) -> Result<i64, ApiError> {
    db::query_scalar::<i64>(
        "SELECT id FROM game_versions WHERE game_id = ? AND missing = 0
         ORDER BY COALESCE(release_date, indexed_at) DESC, id DESC LIMIT 1",
    )
    .bind(game_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("version_not_found", "No downloadable version"))
}

/// Version used when the caller does not name one: the pinned version (#168)
/// while it still exists, otherwise the newest available one.
async fn preferred_version_id(state: &AppState, game_id: i64) -> Result<i64, ApiError> {
    if let Some(pinned) = game_default_version_id(state, game_id).await? {
        let exists = db::query_scalar::<i64>(
            "SELECT id FROM game_versions WHERE id = ? AND game_id = ? AND missing = 0",
        )
        .bind(pinned)
        .bind(game_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?;
        if let Some(id) = exists {
            return Ok(id);
        }
    }
    newest_version(state, game_id).await
}

/// Newest available version other than `exclude`, used as the fallback pin
/// after an editor deletes the pinned archive.
async fn newest_other_version(
    state: &AppState,
    game_id: i64,
    exclude: i64,
) -> Result<Option<i64>, ApiError> {
    db::query_scalar::<i64>(
        "SELECT id FROM game_versions WHERE game_id = ? AND id != ? AND missing = 0
         ORDER BY COALESCE(release_date, indexed_at) DESC, id DESC LIMIT 1",
    )
    .bind(game_id)
    .bind(exclude)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)
}

async fn authorize_version(
    state: &AppState,
    headers: &HeaderMap,
    game_id: i64,
    requested_version: Option<i64>,
    ticket: Option<&str>,
) -> Result<i64, ApiError> {
    if let Some(ticket) = ticket {
        let decoded = crate::auth::decode_download_ticket(&state.jwt_secret, ticket)
            .ok_or_else(ApiError::unauthorized)?;
        if decoded.game_id != game_id {
            return Err(ApiError::forbidden(
                "ticket_mismatch",
                "Ticket does not match this game",
            ));
        }
        if let Some(requested) = requested_version {
            if requested != decoded.version_id {
                return Err(ApiError::forbidden(
                    "ticket_mismatch",
                    "Ticket does not match this version",
                ));
            }
        }
        return Ok(decoded.version_id);
    }

    let token = bearer_token(headers).ok_or_else(ApiError::unauthorized)?;
    let user = authenticate_token(state, &token).await?;
    if age_blocked(user.age_limit, game_age_rating(state, game_id).await?) {
        deny_age_restricted(state, &user, game_id);
        return Err(age_not_found());
    }
    match requested_version {
        Some(version_id) => Ok(version_id),
        None => preferred_version_id(state, game_id).await,
    }
}

async fn download(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    ApiPath(game_id): ApiPath<i64>,
    ApiQuery(query): ApiQuery<DownloadQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _download = state.metrics.download_started();
    let version_id = authorize_version(
        &state,
        &headers,
        game_id,
        query.version_id,
        query.token.as_deref(),
    )
    .await?;

    let version = fetch_version(&state, game_id, version_id).await?;
    if version.missing != 0 {
        return Err(ApiError::not_found(
            "file_not_found",
            "This version is no longer available",
        ));
    }
    // A stalled single-stream client reconnects with `Range` from its last
    // byte, which is a normal request here, so #195 bounded how many times per
    // minute and version one address may do that. A parallel job instead names
    // itself and gets its own budget, address ceiling and part cap (#217).
    let job_id = download_job_id(&headers, &query)?;
    let part_index = download_part_index(&query)?;
    let admission = state
        .download_limits
        .admit(peer.ip(), version.id, job_id.as_deref())
        .map_err(download_limited)?;
    let context = StreamContext {
        job_id,
        part_index,
        admission,
    };
    let _lease = state.transfers.begin(version.id);
    let response = if is_s3_version(&version) {
        if let Some(key) = version.object_key.as_deref() {
            match download_from_storage(&state, &version, key, &headers).await {
                Ok(response) => response,
                Err(error) if can_fall_back_to_local(&error) => {
                    tracing::warn!(
                        game = game_id,
                        version = version_id,
                        object = key,
                        "object storage read failed; trying the local library file"
                    );
                    download_local(&state, &version, &headers).await?
                }
                Err(error) => return Err(error),
            }
        } else {
            download_local(&state, &version, &headers).await?
        }
    } else {
        download_local(&state, &version, &headers).await?
    };
    Ok(log_stream(response, game_id, context, &version))
}

/// Refused admission as the public `429 too_many_download_attempts` error.
fn download_limited(rejection: LimitRejection) -> ApiError {
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "too_many_download_attempts",
        rejection.message(),
    )
    .with_retry_after(rejection.retry_after)
}

/// Job id from `X-Ludex-Job-Id` (preferred) or `?jobId=` (#217).
fn download_job_id(headers: &HeaderMap, query: &DownloadQuery) -> Result<Option<String>, ApiError> {
    let raw = match headers.get(JOB_ID_HEADER) {
        Some(value) => Some(value.to_str().map_err(|_| invalid_job_id())?),
        None => query.job_id.as_deref(),
    };
    parse_job_id(raw).map_err(|_| invalid_job_id())
}

fn invalid_job_id() -> ApiError {
    ApiError::bad_request(
        "invalid_job_id",
        "Job id must be 1-64 characters of A-Z, a-z, 0-9, '-' or '_'",
    )
}

/// Largest accepted `partIndex`; purely a log label, so it is not otherwise
/// constrained.
const MAX_PART_INDEX: u32 = 255;

fn download_part_index(query: &DownloadQuery) -> Result<Option<u32>, ApiError> {
    match query.part_index {
        Some(index) if index > MAX_PART_INDEX => Err(ApiError::bad_request(
            "invalid_part_index",
            "partIndex must be at most 255",
        )),
        other => Ok(other),
    }
}

/// Whether a failed S3 read may fall back to the local library file.
fn can_fall_back_to_local(error: &ApiError) -> bool {
    matches!(
        error.code.as_str(),
        "file_not_found" | "storage_unavailable"
    )
}

/// Serves a version from its library root with `ServeFile` (Range; `If-Range`
/// is enforced here).
///
/// The response body is copied in 1 MiB chunks instead of the tower default
/// (64 KiB) to keep the send path cheap on large archives (#194). A field
/// campaign measured 373 MB/s with `dd bs=512K iflag=direct` against 640-1000
/// MB/s with `dd bs=1M` on the same cold zone (#211), so the chunk is 1 MiB.
/// Logging and the part permit are attached by [`log_stream`] once the
/// response exists.
async fn download_local(
    state: &AppState,
    version: &VersionRow,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let path = local_version_path(state, version).await?;
    let metadata = tokio::fs::metadata(&path).await.map_err(|_| {
        ApiError::not_found("file_not_found", "The library file is missing from disk")
    })?;
    // A cheap archive identity: the client compares it across Range resumes
    // and restarts explicitly when the file was replaced (#195).
    let etag = file_etag(&metadata);
    let mut request = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/")
        .body(Body::empty())
        .map_err(|_| ApiError::internal())?;
    // `ServeFile` in tower-http 0.6 ignores `If-Range`, so a client resuming a
    // replaced archive would get 206 bytes of the new file at the old offset.
    // RFC 9110 §13.1.5 says the range is only honoured when the validator
    // matches; otherwise answer the full 200. Comparing here is cheaper than
    // serving a corrupt splice, and a non-matching date is a restart too.
    if if_range_matches(headers, etag.as_deref()) {
        for name in [header::RANGE, header::IF_RANGE] {
            if let Some(value) = headers.get(&name) {
                request.headers_mut().insert(name, value.clone());
            }
        }
    }

    let response = ServeFile::new(&path)
        .with_buf_chunk_size(1024 * 1024)
        .oneshot(request)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "file streaming failed");
            ApiError::internal()
        })?;
    let (mut parts, body) = response.into_parts();
    let disposition = content_disposition(&version.file_name);
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        parts.headers.insert(header::CONTENT_DISPOSITION, value);
    }
    parts.headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    if let Some(etag) = etag {
        if let Ok(value) = HeaderValue::from_str(&etag) {
            parts.headers.insert(header::ETAG, value);
        }
    }
    Ok(Response::from_parts(parts, Body::new(body)))
}

/// Whether a client `Range` may be honoured under `If-Range` (#211).
///
/// Only a strong entity-tag equal to the current one matches. A missing
/// `If-Range` keeps the plain `Range` behaviour, and an HTTP-date is treated as
/// a mismatch: the archive is restarted from zero rather than spliced, which is
/// always a valid response and never serves inconsistent bytes.
fn if_range_matches(headers: &HeaderMap, etag: Option<&str>) -> bool {
    let Some(value) = headers.get(header::IF_RANGE) else {
        return true;
    };
    let Some(etag) = etag else {
        return false;
    };
    value.to_str().is_ok_and(|value| value.trim() == etag)
}

/// Attaches the per-stream outcome log (#195), the concurrent-part permit
/// (#217) and the full archive digest (#216) to a download response.
fn log_stream(
    response: Response,
    game_id: i64,
    context: StreamContext,
    version: &VersionRow,
) -> Response {
    let (mut parts, body) = response.into_parts();
    insert_archive_sha256(&mut parts.headers, version);
    let status = parts.status.as_u16();
    let (range_start, range_end) = parts
        .headers
        .get(header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(content_range_bounds)
        .map_or_else(
            || (0, full_range_end(&parts.headers)),
            |(start, end)| (start, Some(end)),
        );
    let log = TransferLog::new(game_id, version, context, status, range_start, range_end);
    let stream = LoggedBody {
        inner: Body::new(body).into_data_stream(),
        log: Some(log),
    };
    Response::from_parts(parts, Body::from_stream(stream))
}

/// Last byte of a full (`200`) response, when its length is known.
fn full_range_end(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(|length| length.checked_sub(1))
}

/// Publishes the full archive SHA-256 on a download or stream response (#216).
///
/// The value is the one stored in `game_versions.content_sha256`; a legacy row
/// without one simply omits the header, and clients fall back to the payload
/// field (also `null`) to decide whether verification is possible.
fn insert_archive_sha256(headers: &mut HeaderMap, version: &VersionRow) {
    let Some(digest) = version.content_sha256.as_deref() else {
        return;
    };
    if let Ok(value) = HeaderValue::from_str(digest) {
        headers.insert(
            HeaderName::from_static(crate::content_hash::ARCHIVE_SHA256_HEADER),
            value,
        );
    }
}

/// Bounds of `Content-Range: bytes <start>-<end>/<total>`.
fn content_range_bounds(value: &str) -> Option<(u64, u64)> {
    let range = value.trim().strip_prefix("bytes ")?;
    let (start, rest) = range.split_once('-')?;
    let (end, _) = rest.split_once('/')?;
    Some((start.trim().parse().ok()?, end.trim().parse().ok()?))
}

/// What identifies one download request in the stream log (#217).
struct StreamContext {
    job_id: Option<String>,
    part_index: Option<u32>,
    admission: DownloadAdmission,
}

/// Per-stream outcome, logged once when the body finishes or is dropped.
///
/// The admission is owned here so the concurrent-part permit is released at
/// exactly the moment the stream ends, not when the handler returns.
struct TransferLog {
    game_id: i64,
    version_id: i64,
    status: u16,
    job_id: Option<String>,
    part_index: Option<u32>,
    range_start: u64,
    range_end: Option<u64>,
    /// Size recorded for the archive, used when the response announced none.
    full_size: u64,
    bytes: u64,
    started: Instant,
    finished: bool,
    _admission: DownloadAdmission,
}

impl TransferLog {
    fn new(
        game_id: i64,
        version: &VersionRow,
        context: StreamContext,
        status: u16,
        range_start: u64,
        range_end: Option<u64>,
    ) -> Self {
        let StreamContext {
            job_id,
            part_index,
            admission,
        } = context;
        Self {
            game_id,
            version_id: version.id,
            status,
            job_id,
            part_index,
            range_start,
            range_end,
            full_size: version.size.max(0) as u64,
            bytes: 0,
            started: Instant::now(),
            finished: false,
            _admission: admission,
        }
    }

    fn finish(&mut self, outcome: &'static str) {
        if self.finished {
            return;
        }
        self.finished = true;
        let elapsed_ms = self.started.elapsed().as_millis() as u64;
        if outcome == "complete" {
            tracing::info!(
                game = self.game_id,
                version = self.version_id,
                status = self.status,
                job = ?self.job_id,
                part = ?self.part_index,
                range_start = self.range_start,
                range_end = ?self.range_end,
                bytes = self.bytes,
                elapsed_ms,
                outcome,
                "download stream closed"
            );
        } else {
            tracing::warn!(
                game = self.game_id,
                version = self.version_id,
                status = self.status,
                job = ?self.job_id,
                part = ?self.part_index,
                range_start = self.range_start,
                range_end = ?self.range_end,
                bytes = self.bytes,
                elapsed_ms,
                outcome,
                "download stream closed"
            );
        }
    }
}

impl Drop for TransferLog {
    fn drop(&mut self) {
        let outcome =
            transfer_outcome(self.bytes, self.range_start, self.range_end, self.full_size);
        self.finish(outcome);
    }
}

/// Terminal outcome when the body is dropped without an explicit EOF (#211).
///
/// A response with a known `Content-Length` is never polled again after its
/// last byte: hyper stops asking for frames, so `Poll::Ready(None)` does not
/// arrive and the body is dropped right after a complete transfer. Comparing
/// the counted bytes with the announced window is what distinguishes a finished
/// download from a real client abort; without it, every successful transfer was
/// logged as `client_abort`.
fn transfer_outcome(
    bytes: u64,
    range_start: u64,
    range_end: Option<u64>,
    full_size: u64,
) -> &'static str {
    let expected = match range_end {
        Some(end) => end.checked_sub(range_start).map(|span| span + 1),
        // No `Content-Range` and no `Content-Length`: fall back to the archive
        // size recorded at scan time, when it is known.
        None => Some(full_size).filter(|size| *size > 0),
    };
    match expected {
        Some(expected) if bytes == expected => "complete",
        _ => "client_abort",
    }
}

/// Strong-ish tag from size and modification time, matching the bytes on disk.
fn file_etag(metadata: &std::fs::Metadata) -> Option<String> {
    let modified = metadata.modified().ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(format!("\"{:x}-{:x}\"", metadata.len(), since.as_nanos()))
}

/// Body wrapper that accounts bytes and logs the terminal outcome (#195).
struct LoggedBody<S> {
    inner: S,
    log: Option<TransferLog>,
}

impl<S> Stream for LoggedBody<S>
where
    S: Stream<Item = Result<Bytes, axum::Error>> + Unpin,
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_next(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => {
                // EOF is the one unambiguous end of stream. A body with a
                // `Content-Length` usually stops before this point; `Drop`
                // handles that case by comparing the byte count (#211).
                if let Some(mut log) = this.log.take() {
                    log.finish("complete");
                }
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(error))) => {
                if let Some(mut log) = this.log.take() {
                    log.finish("error");
                }
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(Some(Ok(bytes))) => {
                if let Some(log) = this.log.as_mut() {
                    log.bytes = log.bytes.saturating_add(bytes.len() as u64);
                }
                Poll::Ready(Some(Ok(bytes)))
            }
        }
    }
}

/// Resolves and validates the local file of a version.
async fn local_version_path(state: &AppState, version: &VersionRow) -> Result<PathBuf, ApiError> {
    if version.missing != 0 {
        return Err(ApiError::not_found(
            "file_not_found",
            "This version is no longer available",
        ));
    }
    if version.root_unavailable != 0 {
        return Err(ApiError::not_found(
            "root_unavailable",
            "The library root holding this file is not available",
        ));
    }
    let path = roots::resolve_version_path(
        &state.pool,
        &state.config,
        version.root_id,
        &version.file_path,
    )
    .await
    .ok_or_else(ApiError::internal)?;
    let metadata = tokio::fs::metadata(&path).await.map_err(|_| {
        ApiError::not_found("file_not_found", "The library file is missing from disk")
    })?;
    if !metadata.is_file() {
        return Err(ApiError::not_found(
            "file_not_found",
            "The library file is missing from disk",
        ));
    }
    Ok(path)
}

/// Whether the row must be read from object storage. Rows indexed before S3
/// was enabled keep `storage_backend = local` and are served from disk.
fn is_s3_version(version: &VersionRow) -> bool {
    version.storage_backend.as_deref() == Some(StorageBackend::S3.as_str())
}

/// Proxies an object-store read, honouring a single HTTP Range.
async fn download_from_storage(
    state: &AppState,
    version: &VersionRow,
    key: &str,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let range = match headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
    {
        Some(value) => match parse_range(value) {
            Some(range) => range,
            None => {
                return Err(ApiError::new(
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    "invalid_range",
                    "Only a single byte range is supported",
                ));
            }
        },
        None => None,
    };

    let object = state
        .storage
        .open(key, range)
        .await
        .map_err(|error| map_storage_error(error, "download"))?;

    let content_type = mime_guess::from_path(&version.file_name)
        .first_or_octet_stream()
        .to_string();
    let disposition = content_disposition(&version.file_name);
    let mut builder = Response::builder()
        .status(if object.range.is_some() {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        );
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        builder = builder.header(header::CONTENT_DISPOSITION, value);
    }
    if let Some(digest) = version.content_sha256.as_deref() {
        builder = builder.header(crate::content_hash::ARCHIVE_SHA256_HEADER, digest);
    }
    let length = match object.range {
        Some((start, end)) => {
            builder = builder.header(
                header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{}", object.size),
            );
            end.saturating_sub(start).saturating_add(1)
        }
        None => object.size,
    };
    builder
        .header(header::CONTENT_LENGTH, length)
        .body(object.body)
        .map_err(|_| ApiError::internal())
}

/// Parses `bytes=start-end`, `bytes=start-` and `bytes=-length`.
fn parse_range(value: &str) -> Option<Option<RequestRange>> {
    let value = value.trim();
    let value = value.strip_prefix("bytes=")?;
    if value.contains(',') {
        return None;
    }
    let (start, end) = value.split_once('-')?;
    let start = start.trim();
    let end = end.trim();
    if start.is_empty() {
        let length = end.parse::<u64>().ok()?;
        return (length > 0).then_some(Some(RequestRange::Suffix(length)));
    }
    let start = start.parse::<u64>().ok()?;
    if end.is_empty() {
        return Some(Some(RequestRange::Offset(start)));
    }
    let end = end.parse::<u64>().ok()?;
    (start <= end).then_some(Some(RequestRange::Bounded { start, end }))
}

/// Maps storage failures to stable API errors. Upstream details are logged,
/// never returned: they can contain bucket names or signed URL fragments.
fn map_storage_error(error: StorageError, action: &str) -> ApiError {
    match error {
        StorageError::NotFound => ApiError::not_found(
            "file_not_found",
            "The archive is missing from object storage",
        ),
        StorageError::RangeNotSatisfiable => ApiError::new(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "invalid_range",
            "The requested byte range cannot be satisfied",
        ),
        StorageError::Disabled => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "Object storage is not configured",
        ),
        StorageError::Unavailable(detail) => {
            tracing::warn!(%detail, action, "object storage request failed");
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "storage_unavailable",
                "The object storage backend did not answer the request",
            )
        }
    }
}

/// Stages an object in the data directory so the tar streamer can seek.
async fn stage_object(state: &AppState, version: &VersionRow) -> Result<(PathBuf, u64), ApiError> {
    let key = version
        .object_key
        .as_deref()
        .ok_or_else(|| ApiError::not_found("file_not_found", "Object key is missing"))?;
    let directory = state.config.data_dir.join("tmp");
    let path = directory.join(format!(
        "stream-{}-{}.archive",
        version.id,
        uuid::Uuid::new_v4()
    ));
    let size = match state.storage.download_to_path(key, &path).await {
        Ok(size) => size,
        Err(error) => {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(map_storage_error(error, "stream stage"));
        }
    };
    Ok((path, size))
}

async fn cleanup_staged(path: &std::path::Path, staged: bool) {
    if staged {
        let _ = tokio::fs::remove_file(path).await;
    }
}

async fn stream_version(
    State(state): State<AppState>,
    ApiPath((game_id, version_id)): ApiPath<(i64, i64)>,
    ApiQuery(query): ApiQuery<StreamQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _download = state.metrics.download_started();
    let version_id = authorize_version(
        &state,
        &headers,
        game_id,
        Some(version_id),
        query.token.as_deref(),
    )
    .await?;

    let version = fetch_version(&state, game_id, version_id).await?;
    if version.missing != 0 {
        return Err(ApiError::not_found(
            "file_not_found",
            "This version is no longer available",
        ));
    }
    let _lease = state.transfers.begin(version.id);
    let mut staged = false;
    let path = if is_s3_version(&version) && version.object_key.is_some() {
        // Stream install needs random access (ZIP central directory, RAR
        // headers): the object is staged under `{data_dir}/tmp` and removed
        // once the tar stream ends. Disk use is bounded by one archive at a
        // time per concurrent stream request.
        match stage_object(&state, &version).await {
            Ok((path, _size)) => {
                staged = true;
                path
            }
            Err(error) if can_fall_back_to_local(&error) => {
                tracing::warn!(
                    game = game_id,
                    version = version_id,
                    "object storage staging failed; trying the local library file"
                );
                local_version_path(&state, &version).await?
            }
            Err(error) => return Err(error),
        }
    } else {
        local_version_path(&state, &version).await?
    };
    let cleanup = staged.then(|| path.clone());

    let format = match detect_format(&path) {
        Ok(format) => format,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            cleanup_staged(&path, staged).await;
            return Err(ApiError::not_found(
                "file_not_found",
                "The library file is missing from disk",
            ));
        }
        Err(error) => {
            cleanup_staged(&path, staged).await;
            return Err(ApiError::unprocessable(
                "stream_failed",
                format!("Could not inspect the archive: {error}"),
            ));
        }
    };
    if format == ArchiveFormat::Unknown {
        cleanup_staged(&path, staged).await;
        return Err(stream_unsupported());
    }

    let size_path = path.clone();
    let tar_size = match tokio::task::spawn_blocking(move || archive_tar_size(&size_path)).await {
        Ok(Ok(size)) => size,
        Ok(Err(error)) => {
            cleanup_staged(&path, staged).await;
            return Err(map_archive_error(error));
        }
        Err(error) => {
            cleanup_staged(&path, staged).await;
            return Err(ApiError::unprocessable(
                "stream_failed",
                format!("Archive size task failed: {error}"),
            ));
        }
    };

    let title = fetch_game_title(&state, game_id).await?;
    let file_name = if title.trim().is_empty() {
        "game.tar".to_string()
    } else {
        format!("{}.tar", title.trim())
    };
    let disposition = HeaderValue::from_str(&content_disposition(&file_name))
        .map_err(|_| ApiError::internal())?;

    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-tar"),
        )
        .header(header::CONTENT_LENGTH, tar_size)
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        )
        // A reverse proxy must not buffer a multi-gigabyte stream: the client
        // needs each chunk as it is produced (#204).
        .header(
            HeaderName::from_static("x-accel-buffering"),
            HeaderValue::from_static("no"),
        );
    if let Some(digest) = version.content_sha256.as_deref() {
        builder = builder.header(crate::content_hash::ARCHIVE_SHA256_HEADER, digest);
    }
    builder
        .body(archive_tar_body(path, tar_size, cleanup))
        .map_err(|_| ApiError::internal())
}

// ---------------------------------------------------------------------------
// Resumable per-entry stream installs (#115)
// ---------------------------------------------------------------------------

/// A resolvable archive path for the stream endpoints: either the library file
/// or an object-store archive staged under `{data_dir}/tmp`.
struct StreamSource {
    path: PathBuf,
    staged: bool,
    format: ArchiveFormat,
}

/// Resolves the archive of a version and detects its format.
///
/// Object-store archives are staged (the tar/manifest readers need random
/// access) and removed by the caller/response once it is done with them.
async fn resolve_stream_source(
    state: &AppState,
    version: &VersionRow,
) -> Result<StreamSource, ApiError> {
    let mut staged = false;
    let path = if is_s3_version(version) && version.object_key.is_some() {
        match stage_object(state, version).await {
            Ok((path, _size)) => {
                staged = true;
                path
            }
            Err(error) if can_fall_back_to_local(&error) => {
                tracing::warn!(
                    version = version.id,
                    "object storage staging failed; trying the local library file"
                );
                local_version_path(state, version).await?
            }
            Err(error) => return Err(error),
        }
    } else {
        local_version_path(state, version).await?
    };

    let format = match detect_format(&path) {
        Ok(format) => format,
        Err(error) => {
            cleanup_staged(&path, staged).await;
            return Err(if error.kind() == std::io::ErrorKind::NotFound {
                ApiError::not_found("file_not_found", "The library file is missing from disk")
            } else {
                ApiError::unprocessable(
                    "stream_failed",
                    format!("Could not inspect the archive: {error}"),
                )
            });
        }
    };
    Ok(StreamSource {
        path,
        staged,
        format,
    })
}

/// `GET /api/games/{id}/versions/{versionId}/stream/manifest`
///
/// Ordered ZIP manifest for a resumable per-entry install. Same authorization
/// as `/stream`: a Bearer token or a download ticket. Non-ZIP archives answer
/// `415 stream_unsupported`, which is the client's signal to fall back to the
/// sequential tar stream or the normal resumable download.
async fn stream_manifest(
    State(state): State<AppState>,
    ApiPath((game_id, version_id)): ApiPath<(i64, i64)>,
    ApiQuery(query): ApiQuery<StreamQuery>,
    headers: HeaderMap,
) -> Result<Json<StreamManifest>, ApiError> {
    let version_id = authorize_version(
        &state,
        &headers,
        game_id,
        Some(version_id),
        query.token.as_deref(),
    )
    .await?;

    let version = fetch_version(&state, game_id, version_id).await?;
    if version.missing != 0 {
        return Err(ApiError::not_found(
            "file_not_found",
            "This version is no longer available",
        ));
    }
    let _lease = state.transfers.begin(version.id);
    let source = resolve_stream_source(&state, &version).await?;

    if source.format != ArchiveFormat::Zip {
        cleanup_staged(&source.path, source.staged).await;
        return Err(stream_unsupported());
    }

    let manifest_path = source.path.clone();
    let built = tokio::task::spawn_blocking(move || zip_stream_manifest(&manifest_path)).await;
    cleanup_staged(&source.path, source.staged).await;

    match built {
        Ok(Ok(manifest)) => Ok(Json(manifest)),
        Ok(Err(ArchiveError::UnsupportedFormat)) => Err(stream_unsupported()),
        Ok(Err(error)) => Err(map_archive_error(error)),
        Err(error) => Err(ApiError::unprocessable(
            "stream_failed",
            format!("Manifest task failed: {error}"),
        )),
    }
}

/// A resolved single `Range` of one entry.
struct EntryRange {
    start: u64,
    length: u64,
    partial: bool,
}

/// Resolves the `Range` header of an entry request against the entry size.
///
/// `Err` means the range is malformed or unsatisfiable: the caller answers 416
/// with `Content-Range: bytes */<size>`.
fn resolve_entry_range(raw: Option<&str>, size: u64) -> Result<EntryRange, ()> {
    let Some(raw) = raw else {
        return Ok(EntryRange {
            start: 0,
            length: size,
            partial: false,
        });
    };
    let parsed = parse_range(raw).ok_or(())?.ok_or(())?;
    match parsed {
        RequestRange::Offset(start) => {
            if start >= size {
                return Err(());
            }
            Ok(EntryRange {
                start,
                length: size - start,
                partial: true,
            })
        }
        RequestRange::Bounded { start, end } => {
            if start >= size || end < start {
                return Err(());
            }
            let end = end.min(size - 1);
            Ok(EntryRange {
                start,
                length: end - start + 1,
                partial: true,
            })
        }
        RequestRange::Suffix(length) => {
            if length == 0 || size == 0 {
                return Err(());
            }
            let start = size.saturating_sub(length);
            Ok(EntryRange {
                start,
                length: size - start,
                partial: true,
            })
        }
    }
}

/// `GET /api/games/{id}/versions/{versionId}/stream/entries/{index}`
///
/// Raw content of one ZIP entry, with single-Range support so an interrupted
/// entry resumes at its byte offset (direct seek for stored entries,
/// decompress-and-skip for DEFLATE/Zstd). The response is a plain byte stream;
/// the client already knows the size and CRC-32 from the manifest.
async fn stream_entry(
    State(state): State<AppState>,
    ApiPath((game_id, version_id, index)): ApiPath<(i64, i64, usize)>,
    ApiQuery(query): ApiQuery<StreamQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let started = std::time::Instant::now();
    let _download = state.metrics.download_started();
    let version_id = authorize_version(
        &state,
        &headers,
        game_id,
        Some(version_id),
        query.token.as_deref(),
    )
    .await?;

    let version = fetch_version(&state, game_id, version_id).await?;
    if version.missing != 0 {
        return Err(ApiError::not_found(
            "file_not_found",
            "This version is no longer available",
        ));
    }
    let _lease = state.transfers.begin(version.id);
    let source = resolve_stream_source(&state, &version).await?;

    if source.format != ArchiveFormat::Zip {
        cleanup_staged(&source.path, source.staged).await;
        return Err(stream_unsupported());
    }

    let info_path = source.path.clone();
    let info = match tokio::task::spawn_blocking(move || zip_entry_info(&info_path, index)).await {
        Ok(Ok(info)) => info,
        Ok(Err(ArchiveError::EntryNotFound)) => {
            cleanup_staged(&source.path, source.staged).await;
            return Err(ApiError::not_found(
                "entry_not_found",
                "This archive does not have that entry",
            ));
        }
        Ok(Err(ArchiveError::UnsupportedFormat)) => {
            cleanup_staged(&source.path, source.staged).await;
            return Err(stream_unsupported());
        }
        Ok(Err(error)) => {
            cleanup_staged(&source.path, source.staged).await;
            return Err(map_archive_error(error));
        }
        Err(error) => {
            cleanup_staged(&source.path, source.staged).await;
            return Err(ApiError::unprocessable(
                "stream_failed",
                format!("Entry task failed: {error}"),
            ));
        }
    };

    let requested = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());
    let range = match resolve_entry_range(requested, info.size) {
        Ok(range) => range,
        Err(()) => {
            cleanup_staged(&source.path, source.staged).await;
            let mut response = ApiError::new(
                StatusCode::RANGE_NOT_SATISFIABLE,
                "invalid_range",
                "The requested byte range cannot be satisfied",
            )
            .into_response();
            if let Ok(value) = HeaderValue::from_str(&format!("bytes */{}", info.size)) {
                response.headers_mut().insert(header::CONTENT_RANGE, value);
            }
            response
                .headers_mut()
                .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            return Ok(response);
        }
    };

    if info.is_dir {
        cleanup_staged(&source.path, source.staged).await;
        return Err(ApiError::not_found(
            "entry_not_found",
            "Directories have no content",
        ));
    }

    let file_name = info
        .path
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("entry.bin");
    let content_type = mime_guess::from_path(&info.path).first_or_octet_stream();
    let mut builder = Response::builder()
        .status(if range.partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, content_type.to_string())
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, range.length)
        .header(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        )
        // Never let Caddy/nginx buffer an entry: a buffered multi-hundred-MB
        // entry looks like a stalled download to the client (#204).
        .header(
            HeaderName::from_static("x-accel-buffering"),
            HeaderValue::from_static("no"),
        );
    if let Ok(value) = HeaderValue::from_str(&content_disposition(file_name)) {
        builder = builder.header(header::CONTENT_DISPOSITION, value);
    }
    if range.partial {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!(
                "bytes {}-{}/{}",
                range.start,
                range.start + range.length - 1,
                info.size
            ),
        );
    }

    let cleanup = source.staged.then(|| source.path.clone());
    tracing::info!(
        game_id,
        version_id,
        index,
        path = %info.path,
        size = info.size,
        start = range.start,
        length = range.length,
        partial = range.partial,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "stream entry response"
    );
    builder
        .body(entry_body(
            source.path,
            index,
            range.start,
            range.length,
            cleanup,
            game_id,
            version_id,
        ))
        .map_err(|_| ApiError::internal())
}

async fn local_path(
    State(state): State<AppState>,
    _user: AuthUser,
    ApiPath((game_id, version_id)): ApiPath<(i64, i64)>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<LocalPathResponse>, ApiError> {
    if !state.config.local_mode || !peer.ip().is_loopback() {
        return Err(ApiError::not_found(
            "local_path_unavailable",
            "Local paths are only available in local mode from loopback",
        ));
    }
    let version = fetch_version(&state, game_id, version_id).await?;
    if version.missing != 0 {
        return Err(ApiError::not_found(
            "file_not_found",
            "This version is no longer available",
        ));
    }
    if is_s3_version(&version) {
        return Err(ApiError::not_found(
            "local_path_unavailable",
            "This version is stored in object storage and has no local path",
        ));
    }
    if version.root_unavailable != 0 {
        return Err(ApiError::not_found(
            "root_unavailable",
            "The library root holding this file is not available",
        ));
    }
    let path = roots::resolve_version_path(
        &state.pool,
        &state.config,
        version.root_id,
        &version.file_path,
    )
    .await
    .ok_or_else(ApiError::internal)?;
    let canonical = tokio::fs::canonicalize(&path).await.map_err(|_| {
        ApiError::not_found("file_not_found", "The library file is missing from disk")
    })?;
    Ok(Json(LocalPathResponse {
        path: canonical.to_string_lossy().into_owned(),
    }))
}

async fn set_progress(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    JsonBody(request): JsonBody<ProgressRequest>,
) -> Result<Json<ProgressUpdate>, ApiError> {
    user.require_scope(Scope::Write)?;
    ensure_game(&state, game_id).await?;
    let existed = progress_exists(&state, user.id, game_id).await?;
    let last_played = if request.state == ProgressState::Playing {
        Some(db::db_time(db::now()))
    } else {
        None
    };
    db::query(
        "INSERT INTO progress (user_id, game_id, state, minutes_played, last_played_at)
         VALUES (?, ?, ?, 0, ?)
         ON CONFLICT(user_id, game_id) DO UPDATE SET
            state = excluded.state,
            last_played_at = COALESCE(excluded.last_played_at, progress.last_played_at)",
    )
    .bind(user.id)
    .bind(game_id)
    .bind(request.state.as_str())
    .bind(last_played)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;

    if !existed {
        crate::community::record(
            &state,
            user.id,
            crate::community::kinds::GAME_ADDED,
            Some(game_id),
            None,
            serde_json::json!({}),
        )
        .await;
    }
    match request.state {
        ProgressState::Playing => {
            crate::community::record_deduped(
                &state,
                user.id,
                crate::community::kinds::GAME_PLAYED,
                Some(game_id),
                serde_json::json!({}),
                30,
            )
            .await;
            crate::social::touch_playing(&state, user.id, game_id).await;
        }
        ProgressState::Completed => {
            crate::community::record_deduped(
                &state,
                user.id,
                crate::community::kinds::GAME_COMPLETED,
                Some(game_id),
                serde_json::json!({}),
                60,
            )
            .await;
        }
        _ => {}
    }
    Ok(Json(fetch_progress(&state, user.id, game_id).await?))
}

async fn report_playtime(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    JsonBody(request): JsonBody<PlaytimeRequest>,
) -> Result<Json<ProgressUpdate>, ApiError> {
    user.require_scope(Scope::Write)?;
    ensure_game(&state, game_id).await?;
    let minutes = request.minutes.clamp(1, 1440);
    let existed = progress_exists(&state, user.id, game_id).await?;
    let ended_at = db::now();
    let started_at = ended_at - Duration::minutes(minutes);
    let ended_text = db::db_time(ended_at);
    let started_text = db::db_time(started_at);

    let mut transaction = state.pool.begin().await.map_err(ApiError::from)?;
    db::query(
        "INSERT INTO progress (user_id, game_id, state, minutes_played, last_played_at)
         VALUES (?, ?, 'unplayed', ?, ?)
         ON CONFLICT(user_id, game_id) DO UPDATE SET
            minutes_played = progress.minutes_played + excluded.minutes_played,
            last_played_at = excluded.last_played_at",
    )
    .bind(user.id)
    .bind(game_id)
    .bind(minutes)
    .bind(&ended_text)
    .execute(&mut transaction)
    .await
    .map_err(ApiError::from)?;
    db::query(
        "INSERT INTO play_sessions (user_id, game_id, minutes, started_at, ended_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(user.id)
    .bind(game_id)
    .bind(minutes)
    .bind(&started_text)
    .bind(&ended_text)
    .execute(&mut transaction)
    .await
    .map_err(ApiError::from)?;
    transaction.commit().await.map_err(ApiError::from)?;

    if !existed {
        crate::community::record(
            &state,
            user.id,
            crate::community::kinds::GAME_ADDED,
            Some(game_id),
            None,
            serde_json::json!({}),
        )
        .await;
    }
    crate::community::record_deduped(
        &state,
        user.id,
        crate::community::kinds::GAME_PLAYED,
        Some(game_id),
        serde_json::json!({ "minutes": minutes }),
        30,
    )
    .await;
    crate::social::touch_playing(&state, user.id, game_id).await;

    Ok(Json(fetch_progress(&state, user.id, game_id).await?))
}

async fn set_bookmark(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    JsonBody(request): JsonBody<BookmarkRequest>,
) -> Result<Json<BookmarkResponse>, ApiError> {
    user.require_scope(Scope::Write)?;
    ensure_game(&state, game_id).await?;
    if request.bookmarked {
        db::query(
            "INSERT INTO bookmarks (user_id, game_id) VALUES (?, ?)
             ON CONFLICT(user_id, game_id) DO NOTHING",
        )
        .bind(user.id)
        .bind(game_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    } else {
        db::query("DELETE FROM bookmarks WHERE user_id = ? AND game_id = ?")
            .bind(user.id)
            .bind(game_id)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?;
    }
    Ok(Json(BookmarkResponse {
        bookmarked: request.bookmarked,
    }))
}

async fn stats(State(state): State<AppState>, user: AuthUser) -> Result<Json<Stats>, ApiError> {
    let games = db::query_scalar::<i64>("SELECT COUNT(*) FROM games")
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)?;
    let versions = db::query_scalar::<i64>("SELECT COUNT(*) FROM game_versions WHERE missing = 0")
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)?;
    let total_bytes = db::query_scalar::<i64>(
        "SELECT CAST(COALESCE(SUM(size), 0) AS BIGINT) FROM game_versions WHERE missing = 0",
    )
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;
    let users = db::query_scalar::<i64>("SELECT COUNT(*) FROM users")
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)?;

    let mut by_state: BTreeMap<ProgressState, i64> = ProgressState::ALL
        .iter()
        .copied()
        .map(|state| (state, 0))
        .collect();
    let rows = db::query_as::<(String, i64)>(
        "SELECT state, COUNT(*) FROM progress WHERE user_id = ? GROUP BY state",
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;
    let mut counted = 0;
    for (state, count) in rows {
        if let Ok(state) = state.parse::<ProgressState>() {
            if state != ProgressState::Unplayed {
                by_state.insert(state, count);
                counted += count;
            }
        }
    }
    by_state.insert(ProgressState::Unplayed, (games - counted).max(0));

    Ok(Json(Stats {
        games,
        versions,
        total_bytes,
        users,
        by_state,
    }))
}

async fn ensure_game(state: &AppState, id: i64) -> Result<(), ApiError> {
    let exists = db::query_scalar::<i64>("SELECT id FROM games WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?;
    exists
        .map(|_| ())
        .ok_or_else(|| ApiError::not_found("game_not_found", "Game not found"))
}

async fn fetch_game_title(state: &AppState, game_id: i64) -> Result<String, ApiError> {
    db::query_scalar::<String>("SELECT title FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("game_not_found", "Game not found"))
}

fn stream_unsupported() -> ApiError {
    ApiError::new(
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "stream_unsupported",
        "Stream install is only available for ZIP archives; use the normal download endpoint",
    )
}

fn map_archive_error(error: ArchiveError) -> ApiError {
    match error {
        ArchiveError::PasswordRequired => ApiError::conflict(
            "archive_password_required",
            "This archive is encrypted and cannot be streamed",
        ),
        ArchiveError::UnsupportedFormat => stream_unsupported(),
        ArchiveError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ApiError::not_found("file_not_found", "The library file is missing from disk")
        }
        other => ApiError::unprocessable(
            "stream_failed",
            format!("Could not stream the archive: {other}"),
        ),
    }
}

async fn fetch_version(
    state: &AppState,
    game_id: i64,
    version_id: i64,
) -> Result<VersionRow, ApiError> {
    db::query_as::<VersionRow>(
        "SELECT id, root_id, file_path, file_name, size, version, release_date,
                early_access, missing, root_unavailable, archive_hash, content_sha256,
                indexed_at, updated_at, storage_backend, object_key, etag
         FROM game_versions WHERE id = ? AND game_id = ?",
    )
    .bind(version_id)
    .bind(game_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("version_not_found", "Game version not found"))
}

async fn progress_exists(state: &AppState, user_id: i64, game_id: i64) -> Result<bool, ApiError> {
    let count =
        db::query_scalar::<i64>("SELECT COUNT(*) FROM progress WHERE user_id = ? AND game_id = ?")
            .bind(user_id)
            .bind(game_id)
            .fetch_one(&state.pool)
            .await
            .map_err(ApiError::from)?;
    Ok(count > 0)
}

async fn report_installed(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
) -> Result<Json<InstalledReportResponse>, ApiError> {
    user.require_scope(Scope::Write)?;
    ensure_game(&state, game_id).await?;
    let recorded = crate::community::record_deduped(
        &state,
        user.id,
        crate::community::kinds::GAME_INSTALLED,
        Some(game_id),
        serde_json::json!({ "source": "desktop" }),
        60,
    )
    .await;
    Ok(Json(InstalledReportResponse { recorded }))
}

async fn fetch_progress(
    state: &AppState,
    user_id: i64,
    game_id: i64,
) -> Result<ProgressUpdate, ApiError> {
    let row = db::query_as::<(String, i64, Option<String>)>(
        "SELECT state, minutes_played, last_played_at FROM progress
         WHERE user_id = ? AND game_id = ?",
    )
    .bind(user_id)
    .bind(game_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(match row {
        Some((state, minutes, last_played)) => ProgressUpdate {
            state: state.parse().unwrap_or_default(),
            minutes_played: minutes,
            last_played_at: last_played.as_deref().and_then(db::parse_time),
        },
        None => ProgressUpdate {
            state: ProgressState::Unplayed,
            minutes_played: 0,
            last_played_at: None,
        },
    })
}

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

fn summary_from_detail(row: &GameDetailRow) -> GameSummary {
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

fn clean_labels(values: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.to_ascii_lowercase()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use object_store::memory::InMemory;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    async fn state_with_memory_store() -> (tempfile::TempDir, AppState) {
        let temp = tempfile::tempdir().expect("tempdir");
        for name in ["games", "data", "media"] {
            std::fs::create_dir_all(temp.path().join(name)).expect("mkdir");
        }
        let config = crate::config::Config::for_local(
            temp.path().join("games"),
            temp.path().join("data"),
            temp.path().join("media"),
        );
        let (scan_tx, _scan_rx) = mpsc::channel(1);
        let mut state = AppState::for_tests(config, scan_tx).await;
        state.storage = crate::storage::ArchiveStorage::from_store(Arc::new(InMemory::new()));
        (temp, state)
    }

    fn version_row(key: Option<String>, backend: &str) -> VersionRow {
        VersionRow {
            id: 1,
            root_id: None,
            file_path: "Cat Quest II.zip".to_string(),
            file_name: "Cat Quest II.zip".to_string(),
            size: 16,
            version: None,
            release_date: None,
            early_access: 0,
            missing: 0,
            root_unavailable: 0,
            archive_hash: None,
            content_sha256: None,
            indexed_at: None,
            updated_at: None,
            storage_backend: Some(backend.to_string()),
            object_key: key,
            etag: None,
        }
    }

    #[test]
    fn version_payload_reports_archive_format_and_stream_support() {
        let build = |file_name: &str, id: i64| {
            let (archive_format, stream_supported) = version_transport(file_name);
            GameVersionInfo {
                id,
                file_name: file_name.to_string(),
                size: 16,
                version: None,
                release_date: None,
                early_access: false,
                archive_hash: None,
                content_sha256: None,
                archive_format,
                stream_supported,
                indexed_at: None,
                updated_at: None,
                missing: false,
                root_unavailable: false,
                download_url: format!("/api/games/1/download?versionId={id}"),
            }
        };

        let zip = serde_json::to_value(build("Cat Quest II.zip", 1)).expect("json");
        assert_eq!(zip["archiveFormat"], "zip");
        assert_eq!(zip["streamSupported"].as_bool(), Some(true));
        assert_eq!(zip["fileName"], "Cat Quest II.zip");
        assert_eq!(zip["size"], 16);

        let seven = serde_json::to_value(build("HITMAN 3 (v3.10.1).7z", 2)).expect("json");
        assert_eq!(seven["archiveFormat"], "7z");
        assert_eq!(seven["streamSupported"].as_bool(), Some(false));

        // Unknown extension or legacy row: no format claim, stream off.
        for name in ["repack", "portable.exe", "archive.tar.gz"] {
            let legacy = serde_json::to_value(build(name, 3)).expect("json");
            assert!(legacy.get("archiveFormat").is_none(), "{name}: {legacy}");
            assert_eq!(legacy["streamSupported"].as_bool(), Some(false), "{name}");
        }
    }

    #[test]
    fn stream_unsupported_message_names_zip_only() {
        let error = stream_unsupported();
        assert_eq!(error.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert!(error.message.contains("ZIP archives"), "{}", error.message);
        assert!(!error.message.contains("7z"), "{}", error.message);
        assert!(!error.message.contains("RAR"), "{}", error.message);
    }

    #[test]
    fn transfer_outcome_separates_a_finished_body_from_an_abort() {
        // A ranged body that delivered `end - start + 1` bytes is complete even
        // though hyper never polls it again for EOF (#211, 7.4).
        assert_eq!(transfer_outcome(100, 100, Some(199), 4096), "complete");
        assert_eq!(transfer_outcome(99, 100, Some(199), 4096), "client_abort");
        // A full 200 response: `range_end` is `content-length - 1`.
        assert_eq!(transfer_outcome(4096, 0, Some(4095), 4096), "complete");
        assert_eq!(transfer_outcome(2048, 0, Some(4095), 4096), "client_abort");
        // No announced length: fall back to the recorded archive size.
        assert_eq!(transfer_outcome(4096, 0, None, 4096), "complete");
        assert_eq!(transfer_outcome(4095, 0, None, 4096), "client_abort");
        // Nothing known at all: never claim a completion.
        assert_eq!(transfer_outcome(4096, 0, None, 0), "client_abort");
    }

    #[tokio::test]
    async fn s3_downloads_stream_ranges_and_missing_objects_are_404() {
        let (temp, state) = state_with_memory_store().await;
        let archive = temp.path().join("Cat Quest II.zip");
        std::fs::write(&archive, b"0123456789abcdef").expect("write");
        let stored = state
            .storage
            .publish_archive("identity", "Cat Quest II.zip", &archive)
            .await
            .expect("upload")
            .expect("object");
        let version = version_row(Some(stored.key.clone()), "s3");

        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, "bytes=2-5".parse().expect("range"));
        let response = download_from_storage(&state, &version, &stored.key, &headers)
            .await
            .expect("ranged download");
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok()),
            Some("bytes 2-5/16")
        );
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        assert_eq!(&body[..], b"2345");

        let missing = download_from_storage(&state, &version, "archives/missing.zip", &headers)
            .await
            .expect_err("missing object");
        assert_eq!(missing.status, StatusCode::NOT_FOUND);
        assert_eq!(missing.code, "file_not_found");
    }

    #[tokio::test]
    async fn local_rows_stay_on_disk_and_disabled_storage_is_503() {
        assert!(!is_s3_version(&version_row(None, "local")));
        assert!(is_s3_version(&version_row(Some("k".to_string()), "s3")));

        let (temp, mut state) = state_with_memory_store().await;
        state.storage = crate::storage::ArchiveStorage::local();
        let version = version_row(Some("archives/whatever.zip".to_string()), "s3");
        let error =
            download_from_storage(&state, &version, "archives/whatever.zip", &HeaderMap::new())
                .await
                .expect_err("local storage");
        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.code, "storage_unavailable");

        // The local fallback path resolves the file from the library root.
        let games_dir = state.config.games_dir.clone();
        std::fs::write(games_dir.join("Cat Quest II.zip"), b"local").expect("write");
        let root_id =
            db::query_scalar::<i64>("SELECT id FROM library_roots ORDER BY position, id LIMIT 1")
                .fetch_one(&state.pool)
                .await
                .expect("root");
        let mut fallback = version_row(Some("archives/whatever.zip".to_string()), "s3");
        fallback.root_id = Some(root_id);
        assert!(can_fall_back_to_local(&error));
        assert_eq!(
            local_version_path(&state, &fallback)
                .await
                .expect("local fallback"),
            games_dir.join("Cat Quest II.zip")
        );
        let _ = temp;
    }

    #[tokio::test]
    async fn version_delete_guards_cover_missing_offline_active_and_last() {
        let (_temp, state) = state_with_memory_store().await;
        let mut version = version_row(None, "local");
        assert!(ensure_version_deletable(&state, &version, 1).is_ok());

        version.missing = 1;
        assert_eq!(
            ensure_version_deletable(&state, &version, 1)
                .expect_err("missing")
                .code,
            "version_missing"
        );
        version.missing = 0;

        version.root_unavailable = 1;
        assert_eq!(
            ensure_version_deletable(&state, &version, 1)
                .expect_err("offline root")
                .code,
            "root_unavailable"
        );
        version.root_unavailable = 0;

        let lease = state.transfers.begin(version.id);
        assert!(state.transfers.is_active(version.id));
        assert_eq!(
            ensure_version_deletable(&state, &version, 1)
                .expect_err("in use")
                .code,
            "version_in_use"
        );
        drop(lease);
        assert!(!state.transfers.is_active(version.id));

        assert_eq!(
            ensure_version_deletable(&state, &version, 0)
                .expect_err("last version")
                .code,
            "last_version"
        );
    }

    #[test]
    fn range_parser_handles_the_three_http_forms() {
        assert_eq!(
            parse_range("bytes=0-99").flatten(),
            Some(RequestRange::Bounded { start: 0, end: 99 })
        );
        assert_eq!(
            parse_range("bytes=10-").flatten(),
            Some(RequestRange::Offset(10))
        );
        assert_eq!(
            parse_range("bytes=-10").flatten(),
            Some(RequestRange::Suffix(10))
        );
        assert!(parse_range("bytes=0-1,4-5").is_none());
        assert!(parse_range("bytes=9-3").is_none());
        assert!(parse_range("items=0-1").is_none());
    }
}
