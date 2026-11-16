//! Emulator and ROM platform routes (#52).

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use utoipa::openapi::HttpMethod;

use crate::api_keys::Scope;
use crate::auth::AuthUser;
use crate::db;
use crate::emulators::{
    self, CreateEmulatorRequest, Emulator, EmulatorLaunch, PlatformDefault, PlatformView,
    SetGameEmulatorRequest, UpdateDefaultsRequest, UpdateEmulatorRequest, ValidateCommandRequest,
    ValidateCommandResponse,
};
use crate::error::ApiError;
use crate::extract::{ApiPath, JsonBody};
use crate::openapi::{body, ok, route, Auth, Payload, RouteDoc};
use crate::state::AppState;

/// Emulator routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/emulators", get(list).post(create))
        .route("/emulators/validate", post(validate))
        .route("/emulators/defaults", get(defaults).put(set_defaults))
        .route(
            "/emulators/{id}",
            axum::routing::patch(update).delete(remove),
        )
        .route("/platforms", get(platforms))
        .route("/games/{id}/launch-command", post(launch_command))
        .route("/games/{id}/emulator", put(set_game_emulator))
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<Emulator>>, ApiError> {
    user.require_scope(Scope::Read)?;
    Ok(Json(emulators::list(&state.core.pool).await?))
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<CreateEmulatorRequest>,
) -> Result<(StatusCode, Json<Emulator>), ApiError> {
    user.require_admin_access()?;
    Ok((
        StatusCode::CREATED,
        Json(emulators::create(&state.core.pool, &request).await?),
    ))
}

async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    JsonBody(request): JsonBody<UpdateEmulatorRequest>,
) -> Result<Json<Emulator>, ApiError> {
    user.require_admin_access()?;
    Ok(Json(
        emulators::update(&state.core.pool, id, &request).await?,
    ))
}

async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_admin_access()?;
    if !emulators::delete(&state.core.pool, id).await? {
        return Err(ApiError::not_found(
            "emulator_not_found",
            "Emulator not found",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn validate(
    State(_state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<ValidateCommandRequest>,
) -> Result<Json<ValidateCommandResponse>, ApiError> {
    user.require_admin_access()?;
    match emulators::validate_command(&request.command) {
        Ok(command) => Ok(Json(ValidateCommandResponse {
            valid: true,
            error: None,
            program: Some(command.program),
            args: command.args,
        })),
        Err(message) => Ok(Json(ValidateCommandResponse {
            valid: false,
            error: Some(message),
            program: None,
            args: Vec::new(),
        })),
    }
}

async fn platforms(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<PlatformView>>, ApiError> {
    user.require_scope(Scope::Read)?;
    Ok(Json(emulators::platforms(&state.core.pool).await?))
}

async fn defaults(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<PlatformDefault>>, ApiError> {
    user.require_scope(Scope::Read)?;
    Ok(Json(emulators::defaults(&state.core.pool).await?))
}

async fn set_defaults(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<UpdateDefaultsRequest>,
) -> Result<Json<Vec<PlatformDefault>>, ApiError> {
    user.require_admin_access()?;
    Ok(Json(
        emulators::set_defaults(&state.core.pool, &request).await?,
    ))
}

async fn launch_command(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
) -> Result<Json<EmulatorLaunch>, ApiError> {
    user.require_scope(Scope::Read)?;
    Ok(Json(
        emulators::launch_for_game(&state.core.pool, game_id).await?,
    ))
}

async fn set_game_emulator(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    JsonBody(request): JsonBody<SetGameEmulatorRequest>,
) -> Result<StatusCode, ApiError> {
    user.require_editor()?;
    let kind = db::query_scalar::<String>("SELECT kind FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_optional(&state.core.pool)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("game_not_found", "Game not found"))?;
    if kind != "rom" {
        return Err(ApiError::unprocessable(
            "not_a_rom",
            "Only ROM entries can point at an emulator",
        ));
    }
    if let Some(id) = request.emulator_id {
        let exists =
            db::query_scalar::<i64>("SELECT COUNT(*) FROM emulators WHERE id = ? AND enabled = 1")
                .bind(id)
                .fetch_one(&state.core.pool)
                .await
                .map_err(ApiError::from)?;
        if exists == 0 {
            return Err(ApiError::not_found(
                "emulator_not_found",
                "Emulator not found or disabled",
            ));
        }
    }
    db::query("UPDATE games SET emulator_id = ?, updated_at = ? WHERE id = ?")
        .bind(request.emulator_id)
        .bind(crate::db::db_time(crate::db::now()))
        .bind(game_id)
        .execute(&state.core.pool)
        .await
        .map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// OpenAPI operations served by this module (#237).
pub(crate) const DOCS: &[RouteDoc] = &[
    // Emulators and ROMs (#52)
    route(
        HttpMethod::Get,
        "/emulators",
        "Emulators",
        "List configured emulators",
        Auth::Session,
        Payload::Array("Emulator"),
    ),
    body(
        route(
            HttpMethod::Post,
            "/emulators",
            "Emulators",
            "Register an emulator command",
            Auth::Admin,
            Payload::Schema("Emulator"),
        ),
        "CreateEmulatorRequest",
    ),
    ok(
        body(
            route(
                HttpMethod::Post,
                "/emulators/validate",
                "Emulators",
                "Validate an emulator command template without running it",
                Auth::Admin,
                Payload::Schema("ValidateCommandResponse"),
            ),
            "ValidateCommandRequest",
        ),
        200,
    ),
    route(
        HttpMethod::Get,
        "/emulators/defaults",
        "Emulators",
        "Default emulator per platform",
        Auth::Session,
        Payload::Array("PlatformDefault"),
    ),
    body(
        route(
            HttpMethod::Put,
            "/emulators/defaults",
            "Emulators",
            "Set or clear platform default emulators",
            Auth::Admin,
            Payload::Array("PlatformDefault"),
        ),
        "UpdateDefaultsRequest",
    ),
    body(
        route(
            HttpMethod::Patch,
            "/emulators/{id}",
            "Emulators",
            "Update an emulator",
            Auth::Admin,
            Payload::Schema("Emulator"),
        ),
        "UpdateEmulatorRequest",
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/emulators/{id}",
            "Emulators",
            "Delete an emulator",
            Auth::Admin,
            Payload::None,
        ),
        204,
    ),
    route(
        HttpMethod::Get,
        "/platforms",
        "Emulators",
        "Platform catalog with ROM counts",
        Auth::Session,
        Payload::Array("PlatformView"),
    ),
    ok(
        route(
            HttpMethod::Post,
            "/games/{id}/launch-command",
            "Emulators",
            "Resolve the emulator launch command for a ROM game",
            Auth::Session,
            Payload::Schema("EmulatorLaunch"),
        ),
        200,
    ),
    ok(
        body(
            route(
                HttpMethod::Put,
                "/games/{id}/emulator",
                "Emulators",
                "Set or clear the per-game emulator override",
                Auth::Editor,
                Payload::None,
            ),
            "SetGameEmulatorRequest",
        ),
        204,
    ),
];
