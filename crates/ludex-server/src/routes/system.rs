//! Public system routes: health, setup and server configuration.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use ludex_core::models::{Role, User};

use crate::audit::actions;
use crate::auth::{hash_password, issue_access_token};
use crate::config::MAX_JSON_BYTES;
use crate::db;
use crate::error::ApiError;
use crate::events::{Actor, Event};
use crate::extract::JsonBody;
use crate::models::{
    AuthResponse, FeatureFlags, HealthResponse, ServerConfig, SetupRequest, SetupStatus,
};
use crate::state::AppState;

/// Public routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/setup/status", get(setup_status))
        .route("/setup", post(setup))
        .route("/config", get(config))
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        uptime_seconds: state.uptime_seconds(),
    })
}

async fn setup_status(State(state): State<AppState>) -> Result<Json<SetupStatus>, ApiError> {
    let users = db::count_users(&state.pool).await?;
    let server_name = db::server_name(&state.pool, &state.config.server_name).await;
    Ok(Json(SetupStatus {
        needs_setup: users == 0,
        server_name,
        version: env!("CARGO_PKG_VERSION").to_string(),
    }))
}

async fn setup(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<SetupRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let users = db::count_users(&state.pool).await?;
    if users > 0 {
        return Err(ApiError::conflict(
            "setup_complete",
            "This server has already been set up",
        ));
    }

    let username = super::validate_username(&request.username)?;
    super::validate_password(&request.password)?;
    let password_hash = hash_password(&request.password)?;
    let now = db::db_time(db::now());

    // The `WHERE NOT EXISTS` clause closes the race between the count check
    // above and this insert: two concurrent setup requests can never both win.
    let mut transaction = state.pool.begin().await.map_err(ApiError::from)?;
    let result = sqlx::query(
        "INSERT INTO users
            (username, password_hash, email, role, activated, avatar_path, created_at, updated_at)
         SELECT ?, ?, NULL, ?, 1, NULL, ?, ?
         WHERE NOT EXISTS (SELECT 1 FROM users)",
    )
    .bind(&username)
    .bind(password_hash)
    .bind(Role::Admin.as_str())
    .bind(&now)
    .bind(&now)
    .execute(&mut *transaction)
    .await
    .map_err(|error| {
        if is_unique_violation(&error) {
            ApiError::conflict("username_taken", "Username is already taken")
        } else {
            ApiError::from(error)
        }
    })?;
    if result.rows_affected() == 0 {
        return Err(ApiError::conflict(
            "setup_complete",
            "This server has already been set up",
        ));
    }

    let server_name = request
        .server_name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty());
    if let Some(server_name) = &server_name {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES ('server_name', ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(server_name)
        .execute(&mut *transaction)
        .await
        .map_err(ApiError::from)?;
    }
    transaction.commit().await.map_err(ApiError::from)?;

    let user = User {
        id: result.last_insert_rowid(),
        username,
        email: None,
        role: Role::Admin,
        activated: true,
        avatar_url: None,
        created_at: db::now(),
    };
    state.events.emit(
        Event::new(actions::USER_CREATED, Actor::System)
            .target("user", user.id.to_string())
            .metadata(serde_json::json!({
                "username": user.username,
                "role": user.role.as_str(),
                "source": "setup",
            })),
    );
    state.events.emit(
        Event::new(actions::SETTINGS_UPDATED, Actor::User { id: user.id })
            .target("settings", "server")
            .metadata(serde_json::json!({
                "serverName": server_name,
            })),
    );
    let token = issue_access_token(&state.jwt_secret, user.id, user.role)?;
    Ok(Json(AuthResponse { token, user }))
}

async fn config(State(state): State<AppState>) -> Result<Json<ServerConfig>, ApiError> {
    let server_name = db::server_name(&state.pool, &state.config.server_name).await;
    Ok(Json(ServerConfig {
        server_name,
        version: env!("CARGO_PKG_VERSION"),
        features: FeatureFlags {
            igdb: state.igdb_enabled(),
            steam: true,
            ingest: true,
            local_mode: state.config.local_mode,
            oidc: state.oidc.is_some(),
        },
        max_upload_bytes: MAX_JSON_BYTES as u64,
        oidc_display_name: state.config.oidc_display_name.clone(),
    }))
}

/// Maps a SQLite unique-constraint failure to a conflict.
#[must_use]
pub(crate) fn is_unique_violation(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::Database(database) => database.is_unique_violation(),
        _ => false,
    }
}
