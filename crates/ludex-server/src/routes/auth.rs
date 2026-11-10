//! Authentication routes.

use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use std::net::SocketAddr;

use crate::api_keys::Scope;
use crate::audit::actions;
use crate::auth::{hash_password, issue_access_token, verify_password, AuthUser};
use crate::db;
use crate::error::ApiError;
use crate::events::{Actor, Event};
use crate::extract::JsonBody;
use crate::models::{AuthResponse, ChangePasswordRequest, LoginRequest, UserResponse, UserRow};
use crate::state::AppState;

/// Authentication routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/me", get(me))
        .route("/auth/logout", post(logout))
        .route("/auth/change-password", post(change_password))
}

async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    JsonBody(request): JsonBody<LoginRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let ip = Some(peer.ip());
    let row = sqlx::query_as::<_, UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path, created_at, updated_at
         FROM users WHERE username = ?",
    )
    .bind(request.username.trim())
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let Some(row) = row else {
        state.events.emit(
            Event::new(actions::LOGIN_FAILED, Actor::System)
                .target("user", request.username.trim())
                .metadata(json!({ "reason": "unknown_user" }))
                .ip(ip),
        );
        return Err(ApiError::invalid_credentials());
    };
    if !verify_password(&request.password, &row.password_hash) {
        state.events.emit(
            Event::new(actions::LOGIN_FAILED, Actor::User { id: row.id })
                .target("user", row.id.to_string())
                .metadata(json!({ "username": row.username, "reason": "bad_password" }))
                .ip(ip),
        );
        return Err(ApiError::invalid_credentials());
    }
    if row.activated == 0 {
        state.events.emit(
            Event::new(actions::LOGIN_FAILED, Actor::User { id: row.id })
                .target("user", row.id.to_string())
                .metadata(json!({ "username": row.username, "reason": "account_disabled" }))
                .ip(ip),
        );
        return Err(ApiError::forbidden(
            "account_disabled",
            "This account is disabled",
        ));
    }

    let user = row.to_public();
    let token = issue_access_token(&state.jwt_secret, user.id, user.role)?;
    state.events.emit(
        Event::new(actions::LOGIN_SUCCEEDED, Actor::User { id: user.id })
            .target("user", user.id.to_string())
            .metadata(json!({ "username": user.username }))
            .ip(ip),
    );
    Ok(Json(AuthResponse { token, user }))
}

async fn me(State(state): State<AppState>, user: AuthUser) -> Result<Json<UserResponse>, ApiError> {
    let row = sqlx::query_as::<_, UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path, created_at, updated_at
         FROM users WHERE id = ?",
    )
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(ApiError::unauthorized)?;
    Ok(Json(UserResponse {
        user: row.to_public(),
    }))
}

async fn logout(_user: AuthUser) -> StatusCode {
    // Tokens are stateless; the client discards it.
    StatusCode::NO_CONTENT
}

async fn change_password(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<ChangePasswordRequest>,
) -> Result<StatusCode, ApiError> {
    user.require_scope(Scope::Write)?;
    let row = sqlx::query_as::<_, UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path, created_at, updated_at
         FROM users WHERE id = ?",
    )
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(ApiError::unauthorized)?;

    if !verify_password(&request.current_password, &row.password_hash) {
        return Err(ApiError::bad_request(
            "invalid_password",
            "Current password is incorrect",
        ));
    }
    super::validate_password(&request.new_password)?;
    let password_hash = hash_password(&request.new_password)?;
    sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?")
        .bind(password_hash)
        .bind(db::db_time(db::now()))
        .bind(user.id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    state.events.emit(
        Event::new(actions::USER_PASSWORD_CHANGED, user.actor())
            .target("user", user.id.to_string())
            .metadata(json!({ "username": user.username })),
    );
    Ok(StatusCode::NO_CONTENT)
}
