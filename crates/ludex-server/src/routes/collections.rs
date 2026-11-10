//! Collections: manual shelves and smart rule-based lists.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use ludex_core::models::{GameSummary, Paginated};
use serde_json::json;

use crate::api_keys::Scope;
use crate::auth::AuthUser;
use crate::collections::{self, Candidate};
use crate::community;
use crate::db;
use crate::error::ApiError;
use crate::extract::{ApiPath, ApiQuery, JsonBody};
use crate::models::{
    Collection, CollectionGamesQuery, CollectionGamesRequest, CollectionGamesResponse,
    CollectionKind, CollectionRow, CreateCollectionRequest, GameListRow, SmartRules,
    UpdateCollectionRequest,
};
use crate::routes::games::{summary_from_list, FROM_JOINS, LIST_COLUMNS};
use crate::sql::{bind_query, bind_query_as, bind_query_scalar, Bind};
use crate::state::AppState;

pub(crate) const COLLECTION_COLUMNS: &str =
    "c.id, c.user_id, c.name, c.description, c.is_public, c.kind, \
c.rules_json, c.created_at, c.updated_at, \
(SELECT COUNT(*) FROM collection_games cg WHERE cg.collection_id = c.id) AS game_count, \
u.username AS owner_username, u.display_name AS owner_display_name";

pub(crate) const COLLECTION_FROM: &str = " FROM collections c JOIN users u ON u.id = c.user_id";

/// Collection routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/collections", get(list).post(create))
        .route(
            "/collections/{id}",
            get(detail).patch(update).delete(delete),
        )
        .route("/collections/{id}/games", get(games).post(add_games))
        .route(
            "/collections/{id}/games/{game_id}",
            axum::routing::delete(remove_game),
        )
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<Collection>>, ApiError> {
    let sql = format!(
        "SELECT {COLLECTION_COLUMNS}{COLLECTION_FROM}
         WHERE c.user_id = ? OR c.is_public = 1
         ORDER BY (c.user_id = ?) DESC, c.name COLLATE NOCASE ASC, c.id ASC"
    );
    let rows = bind_query_as(
        sqlx::query_as::<_, CollectionRow>(&sql),
        &[Bind::Int(user.id), Bind::Int(user.id)],
    )
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;
    let rows = decorate_counts(&state, user.id, rows).await?;
    Ok(Json(
        rows.iter().map(|row| row.to_public(user.id)).collect(),
    ))
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<CreateCollectionRequest>,
) -> Result<(StatusCode, Json<Collection>), ApiError> {
    user.require_scope(Scope::Write)?;
    let name = validate_name(&request.name)?;
    let description = clean_optional(request.description, 2000, "description")?;
    let kind = request.kind.unwrap_or(CollectionKind::Manual);
    let is_public = request.is_public.unwrap_or(false);
    let rules = request.rules.unwrap_or_default();
    if kind == CollectionKind::Smart {
        collections::validate(&rules)
            .map_err(|reason| ApiError::bad_request("invalid_rules", reason))?;
    }
    let rules_json = serde_json::to_string(&rules).map_err(ApiError::from)?;
    let now = db::db_time(db::now());

    let result = sqlx::query(
        "INSERT INTO collections (user_id, name, description, is_public, kind, rules_json, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(user.id)
    .bind(&name)
    .bind(&description)
    .bind(i64::from(is_public))
    .bind(kind.as_str())
    .bind(&rules_json)
    .bind(&now)
    .bind(&now)
    .execute(&state.pool)
    .await
    .map_err(|error| {
        if super::system::is_unique_violation(&error) {
            ApiError::conflict(
                "collection_exists",
                "You already have a collection with that name",
            )
        } else {
            ApiError::from(error)
        }
    })?;
    let id = result.last_insert_rowid();

    if kind == CollectionKind::Manual {
        if let Some(game_ids) = request.game_ids {
            insert_games(&state, id, &game_ids).await?;
        }
    }

    let row = load(&state, id).await?;
    community::record(
        &state,
        user.id,
        community::kinds::COLLECTION_CREATED,
        None,
        Some(id),
        json!({
            "name": name,
            "kind": kind.as_str(),
            "gameCount": row.game_count,
        }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(row.to_public(user.id))))
}

async fn detail(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<Collection>, ApiError> {
    let row = visible_collection(&state, user.id, id).await?;
    let row = decorate_counts(&state, user.id, vec![row]).await?.remove(0);
    Ok(Json(row.to_public(user.id)))
}

async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    JsonBody(request): JsonBody<UpdateCollectionRequest>,
) -> Result<Json<Collection>, ApiError> {
    user.require_scope(Scope::Write)?;
    let current = owned_collection(&state, user.id, id).await?;
    let current_kind = CollectionKind::parse(&current.kind).unwrap_or(CollectionKind::Manual);

    let mut sets = Vec::new();
    let mut binds = Vec::new();

    if let Some(name) = request.name {
        let name = validate_name(&name)?;
        sets.push("name = ?");
        binds.push(Bind::Text(name));
    }
    if let Some(description) = request.description {
        let description = clean_optional(description, 2000, "description")?;
        sets.push("description = ?");
        binds.push(Bind::OptText(description));
    }
    if let Some(is_public) = request.is_public {
        sets.push("is_public = ?");
        binds.push(Bind::Int(i64::from(is_public)));
    }

    let next_kind = request.kind.unwrap_or(current_kind);
    if request.kind.is_some() {
        sets.push("kind = ?");
        binds.push(Bind::Text(next_kind.as_str().to_string()));
    }

    let rules_changed = request.rules.is_some();
    let next_rules = match request.rules {
        Some(rules) => rules,
        None => serde_json::from_str::<SmartRules>(&current.rules_json).unwrap_or_default(),
    };
    if rules_changed {
        sets.push("rules_json = ?");
        binds.push(Bind::Text(
            serde_json::to_string(&next_rules).map_err(ApiError::from)?,
        ));
    }
    if next_kind == CollectionKind::Smart
        && (rules_changed || request.kind == Some(CollectionKind::Smart))
    {
        collections::validate(&next_rules)
            .map_err(|reason| ApiError::bad_request("invalid_rules", reason))?;
    }

    if !sets.is_empty() {
        sets.push("updated_at = ?");
        binds.push(Bind::Text(db::db_time(db::now())));
        binds.push(Bind::Int(id));
        let sql = format!("UPDATE collections SET {} WHERE id = ?", sets.join(", "));
        bind_query(sqlx::query(&sql), &binds)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?;
    }

    // Switching a manual shelf to smart retires its explicit membership; the
    // rules are the only source of truth from then on.
    if current_kind == CollectionKind::Manual && next_kind == CollectionKind::Smart {
        sqlx::query("DELETE FROM collection_games WHERE collection_id = ?")
            .bind(id)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?;
    }

    let row = load(&state, id).await?;
    Ok(Json(row.to_public(user.id)))
}

async fn delete(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_scope(Scope::Write)?;
    owned_collection(&state, user.id, id).await?;
    sqlx::query("DELETE FROM collections WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn games(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    ApiQuery(query): ApiQuery<CollectionGamesQuery>,
) -> Result<Json<Paginated<GameSummary>>, ApiError> {
    let row = visible_collection(&state, user.id, id).await?;
    let page = query.page.unwrap_or(1).max(1);
    let limit = query.limit.unwrap_or(48).clamp(1, 200);
    let offset = (page - 1).saturating_mul(limit);

    if CollectionKind::parse(&row.kind) == Some(CollectionKind::Smart) {
        let rules: SmartRules = serde_json::from_str(&row.rules_json).unwrap_or_default();
        let sql = format!("SELECT {LIST_COLUMNS}{FROM_JOINS} ORDER BY g.sort_title ASC, g.id ASC");
        let rows = bind_query_as(
            sqlx::query_as::<_, GameListRow>(&sql),
            &[Bind::Int(user.id), Bind::Int(user.id)],
        )
        .fetch_all(&state.pool)
        .await
        .map_err(ApiError::from)?;
        let matched: Vec<GameListRow> = rows
            .into_iter()
            .filter(|candidate| collections::evaluate(&rules, &candidate_from_row(candidate)))
            .collect();
        let total = i64::try_from(matched.len()).unwrap_or(i64::MAX);
        let items = matched
            .into_iter()
            .skip(usize::try_from(offset).unwrap_or(usize::MAX))
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .map(summary_from_list)
            .collect();
        return Ok(Json(Paginated {
            items,
            total,
            page,
            limit,
        }));
    }

    let count_sql = "SELECT COUNT(*) FROM collection_games cg WHERE cg.collection_id = ?";
    let total = bind_query_scalar(sqlx::query_scalar::<_, i64>(count_sql), &[Bind::Int(id)])
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)?;

    let sql = format!(
        "SELECT {LIST_COLUMNS} FROM games g \
         JOIN collection_games cg ON cg.game_id = g.id AND cg.collection_id = ? \
         LEFT JOIN progress p ON p.game_id = g.id AND p.user_id = ? \
         LEFT JOIN bookmarks b ON b.game_id = g.id AND b.user_id = ? \
         ORDER BY cg.added_at DESC, g.id DESC LIMIT ? OFFSET ?"
    );
    let rows = bind_query_as(
        sqlx::query_as::<_, GameListRow>(&sql),
        &[
            Bind::Int(id),
            Bind::Int(user.id),
            Bind::Int(user.id),
            Bind::Int(limit),
            Bind::Int(offset),
        ],
    )
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

async fn add_games(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    JsonBody(request): JsonBody<CollectionGamesRequest>,
) -> Result<Json<CollectionGamesResponse>, ApiError> {
    user.require_scope(Scope::Write)?;
    let row = owned_collection(&state, user.id, id).await?;
    if CollectionKind::parse(&row.kind) == Some(CollectionKind::Smart) {
        return Err(ApiError::conflict(
            "smart_collection",
            "Smart collections are built from rules; edit the rules instead",
        ));
    }
    if request.game_ids.is_empty() {
        return Ok(Json(CollectionGamesResponse {
            added: 0,
            total: row.game_count,
        }));
    }
    if request.game_ids.len() > 500 {
        return Err(ApiError::bad_request(
            "too_many_games",
            "Add at most 500 games at a time",
        ));
    }
    let added = insert_games(&state, id, &request.game_ids).await?;
    let total = count_members(&state, id).await?;
    Ok(Json(CollectionGamesResponse { added, total }))
}

async fn remove_game(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath((id, game_id)): ApiPath<(i64, i64)>,
) -> Result<StatusCode, ApiError> {
    user.require_scope(Scope::Write)?;
    let row = owned_collection(&state, user.id, id).await?;
    if CollectionKind::parse(&row.kind) == Some(CollectionKind::Smart) {
        return Err(ApiError::conflict(
            "smart_collection",
            "Smart collections are built from rules; edit the rules instead",
        ));
    }
    sqlx::query("DELETE FROM collection_games WHERE collection_id = ? AND game_id = ?")
        .bind(id)
        .bind(game_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    sqlx::query("UPDATE collections SET updated_at = ? WHERE id = ?")
        .bind(db::db_time(db::now()))
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn insert_games(
    state: &AppState,
    collection_id: i64,
    game_ids: &[i64],
) -> Result<i64, ApiError> {
    let mut unique = Vec::new();
    for id in game_ids {
        if !unique.contains(id) {
            unique.push(*id);
        }
    }
    let placeholders = vec!["?"; unique.len()].join(", ");
    let sql = format!("SELECT id FROM games WHERE id IN ({placeholders})");
    let binds: Vec<Bind> = unique.iter().copied().map(Bind::Int).collect();
    let existing = bind_query_scalar(sqlx::query_scalar::<_, i64>(&sql), &binds)
        .fetch_all(&state.pool)
        .await
        .map_err(ApiError::from)?;
    if existing.len() != unique.len() {
        return Err(ApiError::not_found(
            "game_not_found",
            "One of those games is not in the library",
        ));
    }

    let now = db::db_time(db::now());
    let mut added = 0;
    for game_id in unique {
        let result = sqlx::query(
            "INSERT INTO collection_games (collection_id, game_id, added_at) VALUES (?, ?, ?)
             ON CONFLICT(collection_id, game_id) DO NOTHING",
        )
        .bind(collection_id)
        .bind(game_id)
        .bind(&now)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
        added += result.rows_affected();
    }
    sqlx::query("UPDATE collections SET updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(collection_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    i64::try_from(added).map_err(|_| ApiError::internal())
}

async fn count_members(state: &AppState, collection_id: i64) -> Result<i64, ApiError> {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM collection_games WHERE collection_id = ?")
        .bind(collection_id)
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)
}

/// Collections owned by `owner_id` that `viewer_id` may read.
pub(crate) async fn collections_for(
    state: &AppState,
    viewer_id: i64,
    owner_id: i64,
) -> Result<Vec<Collection>, ApiError> {
    let sql = format!(
        "SELECT {COLLECTION_COLUMNS}{COLLECTION_FROM}
         WHERE c.user_id = ? AND (c.is_public = 1 OR c.user_id = ?)
         ORDER BY c.name COLLATE NOCASE ASC, c.id ASC"
    );
    let rows = bind_query_as(
        sqlx::query_as::<_, CollectionRow>(&sql),
        &[Bind::Int(owner_id), Bind::Int(viewer_id)],
    )
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;
    let rows = decorate_counts(state, viewer_id, rows).await?;
    Ok(rows.iter().map(|row| row.to_public(viewer_id)).collect())
}

/// Fills `game_count` for smart collections by evaluating their rules against
/// the viewer's library. Manual rows already carry their member count.
async fn decorate_counts(
    state: &AppState,
    viewer_id: i64,
    mut rows: Vec<CollectionRow>,
) -> Result<Vec<CollectionRow>, ApiError> {
    if !rows
        .iter()
        .any(|row| CollectionKind::parse(&row.kind) == Some(CollectionKind::Smart))
    {
        return Ok(rows);
    }
    let sql = format!("SELECT {LIST_COLUMNS}{FROM_JOINS}");
    let games = bind_query_as(
        sqlx::query_as::<_, GameListRow>(&sql),
        &[Bind::Int(viewer_id), Bind::Int(viewer_id)],
    )
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;
    for row in &mut rows {
        if CollectionKind::parse(&row.kind) == Some(CollectionKind::Smart) {
            let rules: SmartRules = serde_json::from_str(&row.rules_json).unwrap_or_default();
            row.game_count = i64::try_from(
                games
                    .iter()
                    .filter(|game| collections::evaluate(&rules, &candidate_from_row(game)))
                    .count(),
            )
            .unwrap_or(i64::MAX);
        }
    }
    Ok(rows)
}

async fn load(state: &AppState, id: i64) -> Result<CollectionRow, ApiError> {
    let sql = format!("SELECT {COLLECTION_COLUMNS}{COLLECTION_FROM} WHERE c.id = ?");
    bind_query_as(sqlx::query_as::<_, CollectionRow>(&sql), &[Bind::Int(id)])
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("collection_not_found", "Collection not found"))
}

async fn visible_collection(
    state: &AppState,
    viewer_id: i64,
    id: i64,
) -> Result<CollectionRow, ApiError> {
    let row = load(state, id).await?;
    if row.user_id == viewer_id || row.is_public != 0 {
        Ok(row)
    } else {
        // Private collections are indistinguishable from missing ones.
        Err(ApiError::not_found(
            "collection_not_found",
            "Collection not found",
        ))
    }
}

async fn owned_collection(
    state: &AppState,
    owner_id: i64,
    id: i64,
) -> Result<CollectionRow, ApiError> {
    let row = load(state, id).await?;
    if row.user_id == owner_id {
        Ok(row)
    } else {
        Err(ApiError::not_found(
            "collection_not_found",
            "Collection not found",
        ))
    }
}

fn candidate_from_row(row: &GameListRow) -> Candidate {
    Candidate {
        title: row.title.clone(),
        release_date: row.release_date.clone(),
        genres: db::parse_json_list(&row.genres_json),
        rating: row.rating,
        max_players: row.max_players.and_then(|value| u32::try_from(value).ok()),
        minutes_played: row.minutes_played,
        provider_slug: row.provider_slug.clone(),
        version_count: row.version_count,
    }
}

fn validate_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_name",
            "Give the collection a name",
        ));
    }
    if name.chars().count() > 80 {
        return Err(ApiError::bad_request(
            "invalid_name",
            "Collection names are limited to 80 characters",
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            "invalid_name",
            "Collection name contains invalid characters",
        ));
    }
    Ok(name.to_string())
}

fn clean_optional(
    value: Option<String>,
    max: usize,
    field: &str,
) -> Result<Option<String>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > max {
        return Err(ApiError::bad_request(
            "invalid_description",
            format!("{field} is limited to {max} characters"),
        ));
    }
    Ok(Some(value.to_string()))
}
