//! Library browsing and game detail.

use axum::extract::State;
use axum::Json;
use ludex_core::models::{
    GameDetail, GameSummary, GameVersion, LaunchInfo, Paginated, ProviderRef, Trailer,
};

use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::extract::{ApiPath, ApiQuery};
use crate::models::{GameDetailRow, GameListRow, GameQuery, VersionRow};
use crate::sql::{bind_query_as, bind_query_scalar, Bind};
use crate::state::AppState;

use super::download::{age_blocked, age_not_found, deny_age_restricted};
use super::summary::{
    summary_from_detail, summary_from_list, DETAIL_COLUMNS, FROM_JOINS, LIST_COLUMNS,
};
use super::versions::version_transport;

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

pub(super) async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<GameQuery>,
) -> Result<Json<Paginated<GameSummary>>, ApiError> {
    let filters = Filters::from_query(&query, state.core.pool.is_postgres())?
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
            .fetch_all(&state.core.pool)
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
        .fetch_one(&state.core.pool)
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
        .fetch_all(&state.core.pool)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(Paginated {
        items: rows.into_iter().map(summary_from_list).collect(),
        total,
        page,
        limit,
    }))
}

pub(super) async fn random(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<GameQuery>,
) -> Result<Json<GameSummary>, ApiError> {
    let filters = Filters::from_query(&query, state.core.pool.is_postgres())?
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
            .fetch_all(&state.core.pool)
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
        .fetch_optional(&state.core.pool)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("no_game", "No game matches these filters"))?;
    Ok(Json(summary_from_list(row)))
}

pub(super) async fn detail(
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
    .fetch_optional(&state.core.pool)
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
    .fetch_all(&state.core.pool)
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
