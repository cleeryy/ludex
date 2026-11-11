//! Achievement routes (#117, #118): definitions, unlocks, scan and profiles.

use crate::db;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::json;

use crate::achievements::{
    self, Achievement, DefinitionInput, DefinitionPatch, GameAchievements, ScanReport, ScanRequest,
    UnlockRequest, UserAchievements,
};
use crate::api_keys::Scope;
use crate::audit::actions;
use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, JsonBody};
use crate::state::AppState;

/// Achievement routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/games/{id}/achievements",
            get(list_for_game).post(create_definition),
        )
        .route("/games/{id}/achievements/scan", post(scan))
        .route(
            "/games/{id}/achievements/{key}",
            patch(update_definition).delete(delete_definition),
        )
        .route("/games/{id}/achievements/{key}/unlock", post(unlock))
        .route("/users/{id}/achievements", get(list_for_user))
        .route("/me/achievements", get(list_for_me))
}

/// Result of an unlock call.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UnlockResponse {
    /// True when this call created the unlock, false when it already existed.
    unlocked: bool,
    /// Current achievement state for the account.
    achievement: Achievement,
}

/// Result of a manual definition write.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DefinitionResponse {
    /// Game the definition belongs to.
    game_id: i64,
    /// Normalised key.
    key: String,
}

async fn list_for_game(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
) -> Result<Json<GameAchievements>, ApiError> {
    user.require_scope(Scope::Read)?;
    ensure_game(&state, game_id).await?;
    Ok(Json(
        achievements::for_game(&state.pool, user.id, game_id).await?,
    ))
}

async fn list_for_me(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<UserAchievements>, ApiError> {
    user.require_scope(Scope::Read)?;
    let (_, response) = achievements::for_user(&state.pool, user.id).await?;
    Ok(Json(response))
}

async fn list_for_user(
    State(state): State<AppState>,
    viewer: AuthUser,
    ApiPath(user_id): ApiPath<i64>,
) -> Result<Json<UserAchievements>, ApiError> {
    viewer.require_scope(Scope::Read)?;
    let (public, response) = achievements::for_user(&state.pool, user_id).await?;
    // Achievement showcases follow the community profile opt-in: a private
    // profile is indistinguishable from a missing one for everyone else.
    if !public.public_profile && viewer.id != user_id {
        return Err(ApiError::not_found(
            "profile_not_found",
            "Profile not found",
        ));
    }
    Ok(Json(response))
}

async fn scan(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    JsonBody(request): JsonBody<ScanRequest>,
) -> Result<Json<ScanReport>, ApiError> {
    user.require_editor()?;
    let report = achievements::scan(&state, user.id, game_id, request).await?;
    Ok(Json(report))
}

async fn create_definition(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    JsonBody(request): JsonBody<DefinitionInput>,
) -> Result<(StatusCode, Json<DefinitionResponse>), ApiError> {
    user.require_admin_access()?;
    ensure_game(&state, game_id).await?;
    let id = achievements::create_definition(&state.pool, game_id, &request).await?;
    let key = db::query_scalar::<String>("SELECT key FROM achievements WHERE id = ?")
        .bind(id)
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)?;
    state.events.emit(
        Event::new(actions::ACHIEVEMENT_DEFINITION_CREATED, user.actor())
            .target("achievement", format!("{game_id}:{key}"))
            .metadata(json!({ "gameId": game_id, "key": key })),
    );
    Ok((
        StatusCode::CREATED,
        Json(DefinitionResponse { game_id, key }),
    ))
}

async fn update_definition(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath((game_id, key)): ApiPath<(i64, String)>,
    JsonBody(request): JsonBody<DefinitionPatch>,
) -> Result<Json<DefinitionResponse>, ApiError> {
    user.require_admin_access()?;
    ensure_game(&state, game_id).await?;
    achievements::update_definition(&state.pool, game_id, &key, &request).await?;
    state.events.emit(
        Event::new(actions::ACHIEVEMENT_DEFINITION_UPDATED, user.actor())
            .target("achievement", format!("{game_id}:{key}"))
            .metadata(json!({ "gameId": game_id, "key": key })),
    );
    Ok(Json(DefinitionResponse { game_id, key }))
}

async fn delete_definition(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath((game_id, key)): ApiPath<(i64, String)>,
) -> Result<StatusCode, ApiError> {
    user.require_admin_access()?;
    ensure_game(&state, game_id).await?;
    achievements::delete_definition(&state.pool, game_id, &key).await?;
    state.events.emit(
        Event::new(actions::ACHIEVEMENT_DEFINITION_DELETED, user.actor())
            .target("achievement", format!("{game_id}:{key}"))
            .metadata(json!({ "gameId": game_id, "key": key })),
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn unlock(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath((game_id, key)): ApiPath<(i64, String)>,
    JsonBody(request): JsonBody<UnlockRequest>,
) -> Result<Json<UnlockResponse>, ApiError> {
    user.require_editor()?;
    ensure_game(&state, game_id).await?;
    let target = request.user_id.unwrap_or(user.id);
    if target != user.id {
        user.require_admin()?;
        let exists = db::query_scalar::<i64>("SELECT id FROM users WHERE id = ?")
            .bind(target)
            .fetch_optional(&state.pool)
            .await
            .map_err(ApiError::from)?;
        if exists.is_none() {
            return Err(ApiError::not_found("user_not_found", "User not found"));
        }
    }
    let source = request
        .source
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("manual");
    let source: String = source.chars().take(32).collect();
    let (achievement, created) =
        achievements::unlock(&state, target, game_id, &key, &source, user.actor()).await?;
    Ok(Json(UnlockResponse {
        unlocked: created,
        achievement,
    }))
}

async fn ensure_game(state: &AppState, game_id: i64) -> Result<(), ApiError> {
    let exists = db::query_scalar::<i64>("SELECT id FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?;
    if exists.is_none() {
        return Err(ApiError::not_found("game_not_found", "Game not found"));
    }
    Ok(())
}
