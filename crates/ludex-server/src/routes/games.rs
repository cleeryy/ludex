//! Game library routes: browsing, metadata editing, progress, downloads.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::Duration;
use ludex_core::archive::{archive_tar_size, detect_format, ArchiveError, ArchiveFormat};
use ludex_core::models::{
    GameDetail, GameSummary, GameVersion, LatestVersion, LaunchInfo, Paginated, ProgressState,
    ProgressUpdate, ProviderRef, Stats, Trailer,
};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::api_keys::Scope;
use crate::auth::{authenticate_token, bearer_token, issue_download_ticket, AuthUser};
use crate::db;
use crate::error::ApiError;
use crate::extract::{ApiPath, ApiQuery, JsonBody};
use crate::media::content_disposition;
use crate::models::{
    BookmarkRequest, BookmarkResponse, DownloadQuery, GameDetailRow, GameListRow, GameQuery,
    InstalledReportResponse, LocalPathResponse, PlaytimeRequest, ProgressRequest, RefreshResponse,
    StreamQuery, TicketRequest, TicketResponse, UpdateGameRequest, VersionRow,
};
use crate::roots;
use crate::sql::{bind_query, bind_query_as, bind_query_scalar, Bind};
use crate::state::AppState;
use crate::stream::archive_tar_body;

pub(crate) const LIST_COLUMNS: &str = "g.id, g.title, g.sort_title, g.release_date, g.cover_path, \
g.background_path, g.rating, g.genres_json, g.early_access, g.alt_titles_json, g.max_players, \
g.provider_slug, \
COALESCE(p.state, 'unplayed') AS state, COALESCE(p.minutes_played, 0) AS minutes_played, \
CASE WHEN b.user_id IS NULL THEN 0 ELSE 1 END AS bookmarked, p.last_played_at, \
(SELECT COUNT(*) FROM game_versions v WHERE v.game_id = g.id AND v.missing = 0) AS version_count, \
(SELECT COALESCE(SUM(v.size), 0) FROM game_versions v WHERE v.game_id = g.id AND v.missing = 0) AS total_size";

const DETAIL_COLUMNS: &str = "g.id, g.title, g.sort_title, g.description, g.release_date, \
g.cover_path, g.background_path, g.screenshots_json, g.trailers_json, g.genres_json, \
g.developers_json, g.publishers_json, g.tags_json, g.rating, g.age_rating, g.early_access, \
g.alt_titles_json, g.max_players, \
g.provider_slug, g.provider_id, g.provider_url, g.launch_executable, g.launch_parameters, \
g.installer_executable, g.installer_parameters, g.uninstaller_executable, \
g.uninstaller_parameters, COALESCE(p.state, 'unplayed') AS state, \
COALESCE(p.minutes_played, 0) AS minutes_played, \
CASE WHEN b.user_id IS NULL THEN 0 ELSE 1 END AS bookmarked, p.last_played_at, \
(SELECT COUNT(*) FROM game_versions v WHERE v.game_id = g.id AND v.missing = 0) AS version_count, \
(SELECT COALESCE(SUM(v.size), 0) FROM game_versions v WHERE v.game_id = g.id AND v.missing = 0) AS total_size";

pub(crate) const FROM_JOINS: &str = " FROM games g \
LEFT JOIN progress p ON p.game_id = g.id AND p.user_id = ? \
LEFT JOIN bookmarks b ON b.game_id = g.id AND b.user_id = ?";

/// Game routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/games", get(list))
        .route("/games/random", get(random))
        .route("/games/{id}", get(detail).patch(update_game))
        .route("/games/{id}/refresh-metadata", post(refresh_metadata))
        .route("/games/{id}/download-ticket", post(download_ticket))
        .route("/games/{id}/download", get(download))
        .route("/games/{id}/versions/latest", get(latest_version))
        .route(
            "/games/{id}/versions/{version_id}/stream",
            get(stream_version),
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
    fn from_query(query: &GameQuery) -> Result<Self, ApiError> {
        let mut clauses = Vec::new();
        let mut binds = Vec::new();

        if let Some(genre) = query
            .genre
            .as_deref()
            .map(str::trim)
            .filter(|g| !g.is_empty())
        {
            clauses.push(
                "EXISTS (SELECT 1 FROM json_each(g.genres_json) WHERE value = ?)".to_string(),
            );
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
    let filters = Filters::from_query(&query)?;
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
        let rows = bind_query_as(sqlx::query_as::<_, GameListRow>(&sql), &binds)
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
    let total = bind_query_scalar(sqlx::query_scalar::<_, i64>(&count_sql), &count_binds)
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

    let rows = bind_query_as(sqlx::query_as::<_, GameListRow>(&sql), &binds)
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
    let filters = Filters::from_query(&query)?;
    let mut binds = vec![Bind::Int(user.id), Bind::Int(user.id)];
    binds.extend(filters.binds.iter().cloned());

    if let Some(search) = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        let sql = format!(
            "SELECT {LIST_COLUMNS}{FROM_JOINS}{} LIMIT 5000",
            filters.where_clause()
        );
        let rows = bind_query_as(sqlx::query_as::<_, GameListRow>(&sql), &binds)
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
    let row = bind_query_as(sqlx::query_as::<_, GameListRow>(&sql), &binds)
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
    Ok(Json(fetch_detail(&state, user.id, id).await?))
}

async fn fetch_detail(state: &AppState, user_id: i64, id: i64) -> Result<GameDetail, ApiError> {
    let sql = format!("SELECT {DETAIL_COLUMNS}{FROM_JOINS} WHERE g.id = ?");
    let row = bind_query_as(
        sqlx::query_as::<_, GameDetailRow>(&sql),
        &[Bind::Int(user_id), Bind::Int(user_id), Bind::Int(id)],
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("game_not_found", "Game not found"))?;

    let versions = sqlx::query_as::<_, VersionRow>(
        "SELECT id, root_id, file_path, file_name, size, version, release_date,
                early_access, missing, root_unavailable, archive_hash, updated_at
         FROM game_versions WHERE game_id = ? AND missing = 0
         ORDER BY COALESCE(release_date, indexed_at) DESC, id DESC",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;

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
        versions: versions
            .into_iter()
            .map(|version| GameVersion {
                id: version.id,
                file_name: version.file_name,
                size: version.size,
                version: version.version,
                release_date: version.release_date.as_deref().and_then(db::parse_time),
                early_access: version.early_access != 0,
                archive_hash: version.archive_hash,
                updated_at: version.updated_at.as_deref().and_then(db::parse_time),
                download_url: format!("/api/games/{id}/download?versionId={}", version.id),
            })
            .collect(),
        launch,
    })
}

/// Newest available version with the metadata desktop clients need to detect
/// updates (version label, archive fingerprint, size, last update).
async fn latest_version(
    State(state): State<AppState>,
    _user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<LatestVersion>, ApiError> {
    ensure_game(&state, id).await?;

    let row = sqlx::query_as::<_, VersionRow>(
        "SELECT id, root_id, file_path, file_name, size, version, release_date,
                early_access, missing, root_unavailable, archive_hash, updated_at
         FROM game_versions WHERE game_id = ? AND missing = 0
         ORDER BY COALESCE(release_date, indexed_at) DESC, id DESC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("no_versions", "This game has no available version"))?;

    Ok(Json(LatestVersion {
        game_id: id,
        version_id: row.id,
        version_label: row.version,
        file_name: row.file_name,
        archive_hash: row.archive_hash,
        archive_size: row.size,
        updated_at: row.updated_at.as_deref().and_then(db::parse_time),
        early_access: row.early_access != 0,
        download_url: format!("/api/games/{id}/download?versionId={}", row.id),
    }))
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
        sets.push("description = ?");
        binds.push(Bind::OptText(
            description
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        ));
    }
    if let Some(release_date) = request.release_date {
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
        sets.push("genres_json = ?");
        binds.push(Bind::Text(db::dump_json_list(&clean_labels(genres))));
    }
    if let Some(developers) = request.developers {
        sets.push("developers_json = ?");
        binds.push(Bind::Text(db::dump_json_list(&clean_labels(developers))));
    }
    if let Some(publishers) = request.publishers {
        sets.push("publishers_json = ?");
        binds.push(Bind::Text(db::dump_json_list(&clean_labels(publishers))));
    }
    if let Some(tags) = request.tags {
        sets.push("tags_json = ?");
        binds.push(Bind::Text(db::dump_json_list(&clean_labels(tags))));
    }
    if let Some(rating) = request.rating {
        sets.push("rating = ?");
        binds.push(Bind::OptFloat(rating.map(|value| value.clamp(0.0, 100.0))));
    }
    if let Some(age_rating) = request.age_rating {
        sets.push("age_rating = ?");
        binds.push(Bind::OptInt(age_rating.map(i64::from)));
    }
    if let Some(early_access) = request.early_access {
        sets.push("early_access = ?");
        binds.push(Bind::Int(i64::from(early_access)));
    }
    if let Some(alt_titles) = request.alt_titles {
        sets.push("alt_titles_json = ?");
        binds.push(Bind::Text(db::dump_json_list(&clean_labels(alt_titles))));
    }
    if let Some(players) = request.players {
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
        bind_query(sqlx::query(&sql), &binds)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?;
    }

    Ok(Json(fetch_detail(&state, user.id, id).await?))
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
    let version_id = match request.version_id {
        Some(version_id) => {
            let exists = sqlx::query_scalar::<_, i64>(
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
        None => newest_version(&state, game_id).await?,
    };

    let (token, expires_at) =
        issue_download_ticket(&state.jwt_secret, user.id, game_id, version_id)?;
    Ok(Json(TicketResponse {
        url: format!("/api/games/{game_id}/download?versionId={version_id}&token={token}"),
        expires_at,
    }))
}

async fn newest_version(state: &AppState, game_id: i64) -> Result<i64, ApiError> {
    sqlx::query_scalar::<_, i64>(
        "SELECT id FROM game_versions WHERE game_id = ? AND missing = 0
         ORDER BY COALESCE(release_date, indexed_at) DESC, id DESC LIMIT 1",
    )
    .bind(game_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("version_not_found", "No downloadable version"))
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
    authenticate_token(state, &token).await?;
    match requested_version {
        Some(version_id) => Ok(version_id),
        None => newest_version(state, game_id).await,
    }
}

async fn download(
    State(state): State<AppState>,
    ApiPath(game_id): ApiPath<i64>,
    ApiQuery(query): ApiQuery<DownloadQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
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

    let mut request = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/")
        .body(Body::empty())
        .map_err(|_| ApiError::internal())?;
    for name in [header::RANGE, header::IF_RANGE] {
        if let Some(value) = headers.get(&name) {
            request.headers_mut().insert(name, value.clone());
        }
    }

    let response = ServeFile::new(&path)
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
    Ok(Response::from_parts(parts, Body::new(body)))
}

async fn stream_version(
    State(state): State<AppState>,
    ApiPath((game_id, version_id)): ApiPath<(i64, i64)>,
    ApiQuery(query): ApiQuery<StreamQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
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

    let format = match detect_format(&path) {
        Ok(format) => format,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::not_found(
                "file_not_found",
                "The library file is missing from disk",
            ));
        }
        Err(error) => {
            return Err(ApiError::unprocessable(
                "stream_failed",
                format!("Could not inspect the archive: {error}"),
            ));
        }
    };
    if !matches!(format, ArchiveFormat::Zip | ArchiveFormat::SevenZip) {
        return Err(stream_unsupported());
    }

    let size_path = path.clone();
    let tar_size = tokio::task::spawn_blocking(move || archive_tar_size(&size_path))
        .await
        .map_err(|error| {
            ApiError::unprocessable(
                "stream_failed",
                format!("Archive size task failed: {error}"),
            )
        })?
        .map_err(map_archive_error)?;

    let title = fetch_game_title(&state, game_id).await?;
    let file_name = if title.trim().is_empty() {
        "game.tar".to_string()
    } else {
        format!("{}.tar", title.trim())
    };
    let disposition = HeaderValue::from_str(&content_disposition(&file_name))
        .map_err(|_| ApiError::internal())?;

    Response::builder()
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
        .body(archive_tar_body(path, tar_size))
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
    sqlx::query(
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
    sqlx::query(
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
    .execute(&mut *transaction)
    .await
    .map_err(ApiError::from)?;
    sqlx::query(
        "INSERT INTO play_sessions (user_id, game_id, minutes, started_at, ended_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(user.id)
    .bind(game_id)
    .bind(minutes)
    .bind(&started_text)
    .bind(&ended_text)
    .execute(&mut *transaction)
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
        sqlx::query(
            "INSERT INTO bookmarks (user_id, game_id) VALUES (?, ?)
             ON CONFLICT(user_id, game_id) DO NOTHING",
        )
        .bind(user.id)
        .bind(game_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    } else {
        sqlx::query("DELETE FROM bookmarks WHERE user_id = ? AND game_id = ?")
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
    let games = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM games")
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)?;
    let versions =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM game_versions WHERE missing = 0")
            .fetch_one(&state.pool)
            .await
            .map_err(ApiError::from)?;
    let total_bytes = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(SUM(size), 0) FROM game_versions WHERE missing = 0",
    )
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;
    let users = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)?;

    let mut by_state: BTreeMap<ProgressState, i64> = ProgressState::ALL
        .iter()
        .copied()
        .map(|state| (state, 0))
        .collect();
    let rows = sqlx::query_as::<_, (String, i64)>(
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
    let exists = sqlx::query_scalar::<_, i64>("SELECT id FROM games WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?;
    exists
        .map(|_| ())
        .ok_or_else(|| ApiError::not_found("game_not_found", "Game not found"))
}

async fn fetch_game_title(state: &AppState, game_id: i64) -> Result<String, ApiError> {
    sqlx::query_scalar::<_, String>("SELECT title FROM games WHERE id = ?")
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
        "Stream install is only available for ZIP and 7z archives; use the normal download endpoint",
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
    sqlx::query_as::<_, VersionRow>(
        "SELECT id, root_id, file_path, file_name, size, version, release_date,
                early_access, missing, root_unavailable, archive_hash, updated_at
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
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM progress WHERE user_id = ? AND game_id = ?",
    )
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
    let row = sqlx::query_as::<_, (String, i64, Option<String>)>(
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
        total_size: row.total_size,
        state: row.state.parse().unwrap_or_default(),
        minutes_played: row.minutes_played,
        bookmarked: row.bookmarked != 0,
        last_played_at: row.last_played_at.as_deref().and_then(db::parse_time),
        max_players: row.max_players.and_then(|value| u32::try_from(value).ok()),
        matched_field: None,
        matched_title: None,
        match_score: None,
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
        total_size: row.total_size,
        state: row.state.parse().unwrap_or_default(),
        minutes_played: row.minutes_played,
        bookmarked: row.bookmarked != 0,
        last_played_at: row.last_played_at.as_deref().and_then(db::parse_time),
        max_players: row.max_players.and_then(|value| u32::try_from(value).ok()),
        matched_field: None,
        matched_title: None,
        match_score: None,
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
