//! Controller profile routes (#51).

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use utoipa::openapi::HttpMethod;

use crate::api_keys::Scope;
use crate::auth::AuthUser;
use crate::controller_profiles::{
    self, ControllerProfileEntry, ControllerProfilesResponse, SaveControllerProfileRequest,
};
use crate::error::ApiError;
use crate::extract::{ApiPath, JsonBody};
use crate::openapi::{body, ok, route, Auth, Payload, RouteDoc};
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
    Ok(Json(
        controller_profiles::list(&state.core.pool, user.id).await?,
    ))
}

async fn save(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<SaveControllerProfileRequest>,
) -> Result<Json<ControllerProfileEntry>, ApiError> {
    user.require_scope(Scope::Write)?;
    Ok(Json(
        controller_profiles::save(&state.core.pool, user.id, &request).await?,
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
    controller_profiles::delete(&state.core.pool, user.id, game_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// OpenAPI operations served by this module (#237).
pub(crate) const DOCS: &[RouteDoc] = &[
    // Controller profiles (#51)
    route(
        HttpMethod::Get,
        "/controller-profiles",
        "Controller profiles",
        "Controller profiles of the current account and built-in presets",
        Auth::Session,
        Payload::Schema("ControllerProfilesResponse"),
    ),
    body(
        route(
            HttpMethod::Put,
            "/controller-profiles",
            "Controller profiles",
            "Save the account-wide or a per-game controller profile",
            Auth::Session,
            Payload::Schema("ControllerProfileEntry"),
        ),
        "SaveControllerProfileRequest",
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/controller-profiles/{gameId}",
            "Controller profiles",
            "Delete a controller profile scope (0 = account-wide)",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
];
