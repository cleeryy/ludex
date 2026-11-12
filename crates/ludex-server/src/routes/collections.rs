//! Collections: manual shelves and smart rule-based lists.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use ludex_core::models::{GameSummary, Paginated};
use serde_json::json;

use crate::api_keys::Scope;
use crate::audit::actions;
use crate::auth::AuthUser;
use crate::collection_share::{
    self, CollectionShareLink, CreateCollectionShareRequest, ImportCollectionRequest,
    ImportCollectionResponse, NewCollectionShare,
};
use crate::collections::{self, Candidate};
use crate::community;
use crate::db;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, ApiQuery, JsonBody};
use crate::models::{
    Collection, CollectionGamesQuery, CollectionGamesRequest, CollectionGamesResponse,
    CollectionKind, CollectionRow, CollectionVisibility, CreateCollectionRequest, GameListRow,
    SmartRules, UpdateCollectionRequest,
};
use crate::routes::games::{summary_from_list, FROM_JOINS, LIST_COLUMNS};
use crate::share::ShareDenial;
use crate::sql::{bind_query, bind_query_as, bind_query_scalar, Bind};
use crate::state::AppState;

pub(crate) const COLLECTION_COLUMNS: &str =
    "c.id, c.user_id, c.name, c.description, c.is_public, c.visibility, c.copied_from_id, c.kind, \
c.rules_json, c.created_at, c.updated_at, \
(SELECT COUNT(*) FROM collection_games cg WHERE cg.collection_id = c.id) AS game_count, \
u.username AS owner_username, u.display_name AS owner_display_name";

pub(crate) const COLLECTION_FROM: &str = " FROM collections c JOIN users u ON u.id = c.user_id";

/// Longest accepted share label.
const MAX_LABEL_CHARS: usize = 100;
/// Longest accepted password, to bound the Argon2 cost.
const MAX_PASSWORD_CHARS: usize = 1024;
/// Smallest accepted share password.
const MIN_PASSWORD_CHARS: usize = 4;
/// Expiry bounds, in hours (one hour to one year).
const MIN_EXPIRY_HOURS: i64 = 1;
const MAX_EXPIRY_HOURS: i64 = 8_760;

/// Collection routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/collections", get(list).post(create))
        .route("/collections/import", post(import))
        .route(
            "/collections/{id}",
            get(detail).patch(update).delete(delete),
        )
        .route("/collections/{id}/games", get(games).post(add_games))
        .route(
            "/collections/{id}/games/{game_id}",
            axum::routing::delete(remove_game),
        )
        .route("/collections/{id}/share", post(create_share))
        .route("/collections/{id}/shares", get(list_shares))
        .route(
            "/collections/{id}/shares/{share_id}",
            axum::routing::delete(revoke_share),
        )
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<Collection>>, ApiError> {
    let sql = format!(
        "SELECT {COLLECTION_COLUMNS}{COLLECTION_FROM}
         WHERE c.user_id = ? OR c.visibility = 'public'
         ORDER BY (c.user_id = ?) DESC, lower(c.name) ASC, c.id ASC"
    );
    let rows = bind_query_as(
        db::query_as::<CollectionRow>(&sql),
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
    let visibility = resolve_visibility(request.visibility, request.is_public);
    let is_public = visibility.is_listed();
    let rules = request.rules.unwrap_or_default();
    if kind == CollectionKind::Smart {
        collections::validate(&rules)
            .map_err(|reason| ApiError::bad_request("invalid_rules", reason))?;
    }
    let rules_json = serde_json::to_string(&rules).map_err(ApiError::from)?;
    let now = db::db_time(db::now());

    let id = db::query(
        "INSERT INTO collections
            (user_id, name, description, is_public, visibility, kind, rules_json, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(user.id)
    .bind(&name)
    .bind(&description)
    .bind(i64::from(is_public))
    .bind(visibility.as_str())
    .bind(kind.as_str())
    .bind(&rules_json)
    .bind(&now)
    .bind(&now)
    .returning_id(&state.pool)
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

    if kind == CollectionKind::Manual {
        if let Some(game_ids) = request.game_ids {
            insert_games(&state, id, &game_ids).await?;
        }
    }

    let row = load(&state, id).await?;
    // Private and unlisted shelves stay out of the public feed: their name and
    // existence must never leak through a profile (#173).
    if visibility.is_listed() {
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
                "visibility": visibility.as_str(),
            }),
        )
        .await;
    }
    // Sharing a new shelf notifies the owner's friends (#161).
    if is_public {
        crate::social::notify_collection_shared(&state, user.id, id, &name).await;
    }
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
    let current_visibility =
        CollectionVisibility::parse(&current.visibility).unwrap_or(CollectionVisibility::Private);
    let mut next_visibility = current_visibility;

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
    if let Some(visibility) = resolve_optional_visibility(request.visibility, request.is_public) {
        next_visibility = visibility;
        sets.push("visibility = ?");
        binds.push(Bind::Text(visibility.as_str().to_string()));
        sets.push("is_public = ?");
        binds.push(Bind::Int(i64::from(visibility.is_listed())));
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
        bind_query(db::query(&sql), &binds)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?;
    }

    // Switching a manual shelf to smart retires its explicit membership; the
    // rules are the only source of truth from then on.
    if current_kind == CollectionKind::Manual && next_kind == CollectionKind::Smart {
        db::query("DELETE FROM collection_games WHERE collection_id = ?")
            .bind(id)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?;
    }

    let row = load(&state, id).await?;
    // Publishing an existing shelf is a feed event; unpublishing it is not.
    if next_visibility.is_listed() && !current_visibility.is_listed() {
        community::record(
            &state,
            user.id,
            community::kinds::COLLECTION_CREATED,
            None,
            Some(id),
            json!({
                "name": row.name,
                "kind": row.kind,
                "gameCount": row.game_count,
                "visibility": next_visibility.as_str(),
                "published": true,
            }),
        )
        .await;
        // Newly sharing the shelf notifies the owner's friends (#161).
        crate::social::notify_collection_shared(&state, user.id, id, &row.name).await;
    }
    Ok(Json(row.to_public(user.id)))
}

async fn delete(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_scope(Scope::Write)?;
    owned_collection(&state, user.id, id).await?;
    db::query("DELETE FROM collections WHERE id = ?")
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
            db::query_as::<GameListRow>(&sql),
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
    let total = bind_query_scalar(db::query_scalar::<i64>(count_sql), &[Bind::Int(id)])
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
        db::query_as::<GameListRow>(&sql),
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
    db::query("DELETE FROM collection_games WHERE collection_id = ? AND game_id = ?")
        .bind(id)
        .bind(game_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    db::query("UPDATE collections SET updated_at = ? WHERE id = ?")
        .bind(db::db_time(db::now()))
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn create_share(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    JsonBody(request): JsonBody<CreateCollectionShareRequest>,
) -> Result<(StatusCode, Json<CollectionShareLink>), ApiError> {
    user.require_scope(Scope::Write)?;
    let row = owned_collection(&state, user.id, id).await?;
    if CollectionVisibility::parse(&row.visibility).unwrap_or(CollectionVisibility::Private)
        == CollectionVisibility::Private
    {
        return Err(ApiError::conflict(
            "collection_private",
            "Make the collection unlisted or public before sharing it",
        ));
    }

    let expires_at = match request.expires_in_hours {
        Some(hours) if (MIN_EXPIRY_HOURS..=MAX_EXPIRY_HOURS).contains(&hours) => {
            Some(db::now() + chrono::Duration::hours(hours))
        }
        Some(_) => {
            return Err(ApiError::bad_request(
                "invalid_expiry",
                format!("expiresInHours must be between {MIN_EXPIRY_HOURS} and {MAX_EXPIRY_HOURS}"),
            ))
        }
        None => None,
    };
    let password = validate_share_password(request.password.as_deref())?;
    let label = validate_share_label(request.label.as_deref())?;

    let token = collection_share::generate_token();
    let token_hash = collection_share::hash_token(&token);
    let password_hash = password
        .as_deref()
        .map(crate::auth::hash_password)
        .transpose()
        .map_err(ApiError::from)?;

    let share_id = collection_share::insert(
        &state.pool,
        &NewCollectionShare {
            token_hash: &token_hash,
            collection_id: id,
            label: label.as_deref(),
            created_by: user.id,
            expires_at,
            password_hash: password_hash.as_deref(),
        },
    )
    .await?;

    state.events.emit(
        Event::new(actions::COLLECTION_SHARED, user.actor())
            .target("collection", id.to_string())
            .metadata(json!({
                "shareId": share_id,
                "hasPassword": password.is_some(),
                "expiresAt": expires_at.map(db::db_time),
            })),
    );

    let row = collection_share::get(&state.pool, share_id)
        .await?
        .ok_or_else(ApiError::internal)?;
    Ok((StatusCode::CREATED, Json(row.to_link(Some(token)))))
}

async fn list_shares(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<Vec<CollectionShareLink>>, ApiError> {
    let _ = owned_collection(&state, user.id, id).await?;
    let rows = collection_share::list_for_collection(&state.pool, id).await?;
    Ok(Json(rows.iter().map(|row| row.to_link(None)).collect()))
}

async fn revoke_share(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath((id, share_id)): ApiPath<(i64, i64)>,
) -> Result<StatusCode, ApiError> {
    user.require_scope(Scope::Write)?;
    let _ = owned_collection(&state, user.id, id).await?;
    let row = collection_share::get(&state.pool, share_id)
        .await?
        .filter(|row| row.collection_id == id)
        .ok_or_else(|| ApiError::not_found("collection_share_not_found", "Share link not found"))?;
    if row.revoked_at.is_none() {
        collection_share::revoke(&state.pool, share_id).await?;
        state.events.emit(
            Event::new(actions::COLLECTION_SHARE_REVOKED, user.actor())
                .target("collection", id.to_string())
                .metadata(json!({ "shareId": share_id })),
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Copies a public or link-shared collection into the caller's account.
///
/// Private collections are never importable, even by their owner's token. The
/// target name is unique per account: importing the same share twice merges
/// missing games into the existing copy instead of duplicating the shelf.
async fn import(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<ImportCollectionRequest>,
) -> Result<(StatusCode, Json<ImportCollectionResponse>), ApiError> {
    user.require_scope(Scope::Write)?;
    let source_id = match (request.source_id, request.token.as_deref()) {
        (Some(id), None) => {
            let row = load(&state, id).await?;
            if CollectionVisibility::parse(&row.visibility).unwrap_or(CollectionVisibility::Private)
                != CollectionVisibility::Public
            {
                return Err(ApiError::not_found(
                    "collection_not_found",
                    "Collection not found",
                ));
            }
            row.id
        }
        (None, Some(token)) if !token.trim().is_empty() => {
            let token_hash = collection_share::hash_token(token.trim());
            if !crate::share::allow_attempt(&token_hash) {
                return Err(ApiError::too_many_requests(
                    "rate_limited",
                    "Too many access attempts; retry in a minute",
                ));
            }
            let link = collection_share::find_by_token(&state.pool, &token_hash).await?;
            // One password verification per attempt, whatever the token state.
            let password_ok =
                collection_share::verify_access(link.as_ref(), request.password.as_deref());
            let Some(link) = link else {
                return Err(ShareDenial::NotFound.not_found_error());
            };
            if let Err(denial) = collection_share::validate(&link, db::now()) {
                return Err(denial.access_error());
            }
            if !password_ok {
                return Err(ApiError::new(
                    StatusCode::UNAUTHORIZED,
                    "invalid_share_password",
                    "This share link requires a password",
                ));
            }
            link.collection_id
        }
        _ => {
            return Err(ApiError::bad_request(
                "invalid_source",
                "Provide exactly one of sourceId and token",
            ))
        }
    };

    let source = load(&state, source_id).await?;
    if CollectionVisibility::parse(&source.visibility).unwrap_or(CollectionVisibility::Private)
        == CollectionVisibility::Private
    {
        // A link created before the collection was made private must not keep
        // working; private shelves are never reachable by any import path.
        return Err(ApiError::not_found(
            "collection_not_found",
            "Collection not found",
        ));
    }

    let source_kind = CollectionKind::parse(&source.kind).unwrap_or(CollectionKind::Manual);
    let source_rules: SmartRules = serde_json::from_str(&source.rules_json).unwrap_or_default();
    // Smart rules are copied as rules when they are still valid; a broken tree
    // falls back to an empty manual shelf instead of importing an error.
    let target_kind = if source_kind == CollectionKind::Smart
        && collections::validate(&source_rules).is_ok()
        && !source_rules.rules.is_empty()
    {
        CollectionKind::Smart
    } else {
        CollectionKind::Manual
    };
    let rules_json = if target_kind == CollectionKind::Smart {
        serde_json::to_string(&source_rules).map_err(ApiError::from)?
    } else {
        "{}".to_string()
    };

    let name = validate_name(request.name.as_deref().unwrap_or(&source.name))?;
    let existing = db::query_as::<CollectionRow>(&format!(
        "SELECT {COLLECTION_COLUMNS}{COLLECTION_FROM}
         WHERE c.user_id = ? AND lower(c.name) = lower(?)"
    ))
    .bind(user.id)
    .bind(&name)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let now = db::db_time(db::now());
    let (target_id, created) = match existing {
        Some(row) => {
            if row.copied_from_id != Some(source.id) {
                return Err(ApiError::conflict(
                    "collection_exists",
                    "You already have a collection with that name",
                ));
            }
            (row.id, false)
        }
        None => {
            let id = db::query(
                "INSERT INTO collections
                    (user_id, name, description, is_public, visibility, copied_from_id, kind,
                     rules_json, created_at, updated_at)
                 VALUES (?, ?, ?, 0, 'private', ?, ?, ?, ?, ?)",
            )
            .bind(user.id)
            .bind(&name)
            .bind(&source.description)
            .bind(source.id)
            .bind(target_kind.as_str())
            .bind(&rules_json)
            .bind(&now)
            .bind(&now)
            .returning_id(&state.pool)
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
            (id, true)
        }
    };

    let (added, skipped) = if target_kind == CollectionKind::Manual {
        let source_games = db::query_scalar::<i64>(
            "SELECT game_id FROM collection_games
              WHERE collection_id = ? ORDER BY added_at ASC, game_id ASC",
        )
        .bind(source.id)
        .fetch_all(&state.pool)
        .await
        .map_err(ApiError::from)?;
        copy_games(&state, target_id, &source_games).await?
    } else {
        (0, 0)
    };

    let row = load(&state, target_id).await?;
    state.events.emit(
        Event::new(actions::COLLECTION_IMPORTED, user.actor())
            .target("collection", target_id.to_string())
            .metadata(json!({
                "sourceId": source.id,
                "sourceKind": source_kind.as_str(),
                "created": created,
                "added": added,
                "skipped": skipped,
            })),
    );
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        Json(ImportCollectionResponse {
            collection: row.to_public(user.id),
            created,
            added,
            skipped,
        }),
    ))
}

/// Inserts source games into a manual target, skipping rows that are already
/// present or that no longer exist in the library.
async fn copy_games(
    state: &AppState,
    target_id: i64,
    game_ids: &[i64],
) -> Result<(i64, i64), ApiError> {
    let mut unique = Vec::new();
    for id in game_ids {
        if !unique.contains(id) {
            unique.push(*id);
        }
    }
    let now = db::db_time(db::now());
    let mut added = 0;
    let mut skipped = 0;
    for game_id in &unique {
        let result = db::query(
            "INSERT INTO collection_games (collection_id, game_id, added_at)
             SELECT ?, ?, ? WHERE EXISTS (SELECT 1 FROM games WHERE id = ?)
             ON CONFLICT(collection_id, game_id) DO NOTHING",
        )
        .bind(target_id)
        .bind(game_id)
        .bind(&now)
        .bind(game_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
        if result.rows_affected() == 1 {
            added += 1;
        } else {
            skipped += 1;
        }
    }
    db::query("UPDATE collections SET updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(target_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    Ok((added, skipped))
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
    let existing = bind_query_scalar(db::query_scalar::<i64>(&sql), &binds)
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
        let result = db::query(
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
    db::query("UPDATE collections SET updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(collection_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    i64::try_from(added).map_err(|_| ApiError::internal())
}

async fn count_members(state: &AppState, collection_id: i64) -> Result<i64, ApiError> {
    db::query_scalar::<i64>("SELECT COUNT(*) FROM collection_games WHERE collection_id = ?")
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
         WHERE c.user_id = ? AND c.visibility = 'public'
         ORDER BY lower(c.name) ASC, c.id ASC"
    );
    let rows = bind_query_as(db::query_as::<CollectionRow>(&sql), &[Bind::Int(owner_id)])
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
        db::query_as::<GameListRow>(&sql),
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
    bind_query_as(db::query_as::<CollectionRow>(&sql), &[Bind::Int(id)])
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
    let visible = CollectionVisibility::parse(&row.visibility)
        .unwrap_or(CollectionVisibility::Private)
        .is_listed();
    if row.user_id == viewer_id || visible {
        Ok(row)
    } else {
        // Private and unlisted collections are indistinguishable from missing
        // ones: an unlisted shelf is reachable only through its share token.
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

fn resolve_visibility(
    explicit: Option<CollectionVisibility>,
    legacy: Option<bool>,
) -> CollectionVisibility {
    explicit.unwrap_or_else(|| {
        if legacy.unwrap_or(false) {
            CollectionVisibility::Public
        } else {
            CollectionVisibility::Private
        }
    })
}

fn resolve_optional_visibility(
    explicit: Option<CollectionVisibility>,
    legacy: Option<bool>,
) -> Option<CollectionVisibility> {
    explicit.or_else(|| {
        legacy.map(|public| {
            if public {
                CollectionVisibility::Public
            } else {
                CollectionVisibility::Private
            }
        })
    })
}

fn validate_share_password(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    match raw {
        Some(value) if value.trim().is_empty() => Ok(None),
        Some(value) if value.chars().count() < MIN_PASSWORD_CHARS => Err(ApiError::bad_request(
            "invalid_password",
            format!("Share passwords must be at least {MIN_PASSWORD_CHARS} characters"),
        )),
        Some(value) if value.chars().count() > MAX_PASSWORD_CHARS => Err(ApiError::bad_request(
            "invalid_password",
            format!("Share passwords are limited to {MAX_PASSWORD_CHARS} characters"),
        )),
        Some(value) => Ok(Some(value.to_string())),
        None => Ok(None),
    }
}

fn validate_share_label(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(label) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if label.chars().count() > MAX_LABEL_CHARS {
        return Err(ApiError::bad_request(
            "invalid_label",
            format!("Share labels are limited to {MAX_LABEL_CHARS} characters"),
        ));
    }
    if label.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            "invalid_label",
            "Share labels contain invalid characters",
        ));
    }
    Ok(Some(label.to_string()))
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
