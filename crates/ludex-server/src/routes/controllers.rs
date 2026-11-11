//! Controller profile routes (#51).

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};

use crate::api_keys::Scope;
use crate::auth::AuthUser;
use crate::controller_profiles::{
    self, ControllerProfileEntry, ControllerProfilesResponse, SaveControllerProfileRequest,
};
use crate::error::ApiError;
use crate::extract::{ApiPath, JsonBody};
use crate::state::AppState;

/// Controller profile routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/controller-profiles", get(list).put(save))
        .route("/controller-profiles/{gameId}", delete(remove))
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<ControllerProfilesResponse>, ApiError> {
    user.require_scope(Scope::Read)?;
    Ok(Json(controller_profiles::list(&state.pool, user.id).await?))
}

async fn save(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<SaveControllerProfileRequest>,
) -> Result<Json<ControllerProfileEntry>, ApiError> {
    user.require_scope(Scope::Write)?;
    Ok(Json(
        controller_profiles::save(&state.pool, user.id, &request).await?,
    ))
}

async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_scope(Scope::Write)?;
    if game_id < 0 {
        return Err(ApiError::bad_request(
            "invalid_game_id",
            "gameId must be zero (account profile) or a positive game id",
        ));
    }
    controller_profiles::delete(&state.pool, user.id, game_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
