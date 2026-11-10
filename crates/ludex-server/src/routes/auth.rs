//! Authentication routes: login, sessions, logout and password changes.
//!
//! Login is protected by a per-IP + username throttle (`LoginGuard`) and
//! issues a JWT with a `jti` backed by an `auth_sessions` row. Logout, session
//! revocation and password changes invalidate tokens server-side instead of
//! relying on clients to forget them.

use std::net::SocketAddr;
use std::sync::OnceLock;

use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::api_keys::Scope;
use crate::audit::actions;
use crate::auth::{
    hash_password, revoke_all_sessions, revoke_token, start_session, verify_password, AuthUser,
    SessionInfo,
};
use crate::db;
use crate::error::ApiError;
use crate::events::{Actor, Event};
use crate::extract::{ApiPath, JsonBody};
use crate::models::{
    AuthResponse, AuthSession, AuthSessionRow, ChangePasswordRequest, LoginRequest, UserResponse,
    UserRow,
};
use crate::state::AppState;

/// Authentication and session routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/me", get(me))
        .route("/auth/logout", post(logout))
        .route("/auth/change-password", post(change_password))
        .route("/me/password", post(change_password))
        .route("/auth/sessions", get(sessions))
        .route("/auth/sessions/{id}", delete(revoke_session))
}

async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    JsonBody(request): JsonBody<LoginRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let ip = Some(peer.ip());
    let username = request.username.trim().to_string();
    let key = crate::security::LoginGuard::key(ip, &username);
    if let Err((wait, locked)) = state.login_guard.check(&key) {
        return Err(login_throttled(wait, locked));
    }

    let row = sqlx::query_as::<_, UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path, created_at, updated_at
         FROM users WHERE username = ?",
    )
    .bind(&username)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let Some(row) = row else {
        // Verify against a dummy hash so unknown users cost the same Argon2
        // work as a wrong password (no username enumeration by timing).
        let _ = verify_password(&request.password, dummy_password_hash());
        let (wait, locked) = state.login_guard.record_failure(&key);
        state.events.emit(
            Event::new(actions::LOGIN_FAILED, Actor::System)
                .target("user", &username)
                .metadata(json!({ "reason": "unknown_user" }))
                .ip(ip),
        );
        if locked {
            return Err(login_throttled(wait, true));
        }
        return Err(ApiError::invalid_credentials());
    };
    if !verify_password(&request.password, &row.password_hash) {
        let (wait, locked) = state.login_guard.record_failure(&key);
        state.events.emit(
            Event::new(actions::LOGIN_FAILED, Actor::User { id: row.id })
                .target("user", row.id.to_string())
                .metadata(json!({ "username": row.username, "reason": "bad_password" }))
                .ip(ip),
        );
        if locked {
            return Err(login_throttled(wait, true));
        }
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

    state.login_guard.record_success(&key);
    let user = row.to_public();
    let info = SessionInfo {
        user_agent: headers
            .get(axum::http::header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        ip: Some(peer.ip().to_string()),
    };
    let (issued, session_id) = start_session(&state, user.id, user.role, &info).await?;
    state.events.emit(
        Event::new(actions::LOGIN_SUCCEEDED, Actor::User { id: user.id })
            .target("user", user.id.to_string())
            .metadata(json!({ "username": user.username, "sessionId": session_id }))
            .ip(ip),
    );
    Ok(Json(AuthResponse {
        token: issued.token,
        user,
        expires_at: Some(issued.expires_at),
    }))
}

fn dummy_password_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        hash_password("ludex-timing-equaliser-not-a-real-password")
            .unwrap_or_else(|_| "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$aGFzaA".to_string())
    })
}

fn login_throttled(wait: u64, locked: bool) -> ApiError {
    let message = if locked {
        format!("Too many failed login attempts. Try again in {wait} seconds.")
    } else {
        format!("Too many login attempts. Try again in {wait} seconds.")
    };
    ApiError::too_many_requests("login_throttled", message).with_retry_after(wait)
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

async fn logout(State(state): State<AppState>, user: AuthUser) -> Result<StatusCode, ApiError> {
    if let Some(jti) = user.jti.as_deref() {
        revoke_token(&state, jti, user.id, "logout").await?;
        state.events.emit(
            Event::new(actions::SESSION_REVOKED, user.actor())
                .target("user", user.id.to_string())
                .metadata(json!({ "reason": "logout" })),
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn sessions(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<AuthSession>>, ApiError> {
    let rows = sqlx::query_as::<_, AuthSessionRow>(
        "SELECT id, jti, user_agent, ip, created_at, last_seen_at, expires_at, revoked_at
         FROM auth_sessions WHERE user_id = ? ORDER BY created_at DESC, id DESC",
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(Json(
        rows.iter()
            .map(|row| row.to_public(user.jti.as_deref()))
            .collect(),
    ))
}

async fn revoke_session(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    let row = sqlx::query_as::<_, (i64, i64, String, Option<String>)>(
        "SELECT id, user_id, jti, revoked_at FROM auth_sessions WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("session_not_found", "Session not found"))?;
    if row.1 != user.id {
        user.require_admin()?;
    }
    if row.3.is_none() {
        revoke_token(&state, &row.2, row.1, "session_revoked").await?;
        state.events.emit(
            Event::new(actions::SESSION_REVOKED, user.actor())
                .target("user", row.1.to_string())
                .metadata(json!({ "sessionId": row.0, "reason": "session_revoked" })),
        );
    }
    Ok(StatusCode::NO_CONTENT)
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

    // Every other session dies with the old password; the caller keeps working
    // with the token it just used. Tokens without a `jti` are invalidated too.
    if let Some(jti) = user.jti.as_deref() {
        crate::auth::revoke_other_sessions(&state, user.id, jti, "password_changed").await?;
    } else {
        revoke_all_sessions(&state, user.id, "password_changed").await?;
    }

    state.events.emit(
        Event::new(actions::USER_PASSWORD_CHANGED, user.actor())
            .target("user", user.id.to_string())
            .metadata(json!({ "username": user.username })),
    );
    Ok(StatusCode::NO_CONTENT)
}
