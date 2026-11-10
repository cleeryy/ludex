//! Administrator-only user management.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{delete, get, patch};
use axum::{Json, Router};
use ludex_core::models::Role;
use serde_json::json;

use crate::audit::actions;
use crate::auth::{hash_password, AuthUser};
use crate::db;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, JsonBody};
use crate::models::{CreateUserRequest, UpdateUserRequest, User, UserRow};
use crate::sql::{bind_query, Bind};
use crate::state::AppState;

/// User management routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users", get(list).post(create))
        .route("/users/{id}", patch(update).delete(delete_user))
        .route("/users/{id}/sessions", delete(revoke_sessions))
}

/// Admin: revoke every session of an account (sign out all devices).
async fn revoke_sessions(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    user.require_admin_access()?;
    let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE id = ?")
        .bind(id)
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)?;
    if exists == 0 {
        return Err(ApiError::not_found("user_not_found", "Account not found"));
    }
    let revoked = crate::auth::revoke_all_sessions(&state, id, "admin_revoked").await?;
    state.events.emit(
        Event::new(crate::audit::actions::SESSION_REVOKED, user.actor())
            .target("user", id.to_string())
            .metadata(json!({ "sessions": revoked, "reason": "admin_revoked" })),
    );
    Ok(Json(json!({ "revoked": revoked })))
}

async fn list(State(state): State<AppState>, user: AuthUser) -> Result<Json<Vec<User>>, ApiError> {
    user.require_admin_access()?;
    let rows = sqlx::query_as::<_, UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path,
                age_limit, created_at, updated_at, oidc_issuer, oidc_subject
         FROM users ORDER BY id",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(Json(rows.iter().map(UserRow::to_public).collect()))
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<CreateUserRequest>,
) -> Result<(StatusCode, Json<User>), ApiError> {
    user.require_admin_access()?;
    let username = super::validate_username(&request.username)?;
    super::validate_password(&request.password)?;
    let email = super::validate_email(request.email)?;
    let role = request.role.unwrap_or(Role::User);
    let password_hash = hash_password(&request.password)?;
    let now = db::db_time(db::now());

    let result = sqlx::query(
        "INSERT INTO users
            (username, password_hash, email, role, activated, avatar_path, created_at, updated_at)
         VALUES (?, ?, ?, ?, 1, NULL, ?, ?)",
    )
    .bind(&username)
    .bind(password_hash)
    .bind(&email)
    .bind(role.as_str())
    .bind(&now)
    .bind(&now)
    .execute(&state.pool)
    .await
    .map_err(|error| {
        if super::system::is_unique_violation(&error) {
            ApiError::conflict("username_taken", "Username is already taken")
        } else {
            ApiError::from(error)
        }
    })?;

    let created = sqlx::query_as::<_, UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path,
                age_limit, created_at, updated_at, oidc_issuer, oidc_subject
         FROM users WHERE id = ?",
    )
    .bind(result.last_insert_rowid())
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;

    state.events.emit(
        Event::new(actions::USER_CREATED, user.actor())
            .target("user", created.id.to_string())
            .metadata(json!({
                "username": created.username,
                "role": role.as_str(),
                "source": "admin",
            })),
    );

    Ok((StatusCode::CREATED, Json(created.to_public())))
}

async fn update(
    State(state): State<AppState>,
    caller: AuthUser,
    ApiPath(id): ApiPath<i64>,
    JsonBody(request): JsonBody<UpdateUserRequest>,
) -> Result<Json<User>, ApiError> {
    caller.require_admin_access()?;
    let target = fetch_user(&state, id).await?;
    let current_role = target.role.parse::<Role>().unwrap_or(Role::User);
    let current_active = target.activated != 0;
    let new_role = request.role.unwrap_or(current_role);
    let new_active = request.activated.unwrap_or(current_active);

    if id == caller.id && current_active && !new_active {
        return Err(ApiError::unprocessable(
            "cannot_deactivate_self",
            "You cannot deactivate your own account",
        ));
    }
    if current_role == Role::Admin && current_active && (new_role != Role::Admin || !new_active) {
        ensure_other_active_admin(&state, id).await?;
    }

    // Identity fields are validated up front so a rejected request changes nothing.
    let username = request
        .username
        .map(|value| super::validate_username(&value))
        .transpose()?;
    let email = match request.email {
        Some(value) => Some(super::validate_email(value)?),
        None => None,
    };
    if let Some(password) = request.password.as_deref() {
        super::validate_password(password)?;
    }

    let username_changed = username
        .as_deref()
        .is_some_and(|value| value != target.username);
    let email_changed = email.as_ref().is_some_and(|value| value != &target.email);
    let password_changed = request.password.is_some();

    // The identity provider owns username, email and password for linked accounts.
    if target.is_oidc_managed() && (username_changed || email_changed || password_changed) {
        let issuer = target
            .oidc_issuer
            .as_deref()
            .unwrap_or("your identity provider");
        return Err(ApiError::conflict(
            "managed_by_idp",
            format!("This account is managed by {issuer}; edit it in the identity provider"),
        ));
    }

    let mut changed: Vec<&str> = Vec::new();
    let mut sets = Vec::new();
    let mut binds = Vec::new();
    if username_changed {
        if let Some(username) = username {
            sets.push("username = ?");
            binds.push(Bind::Text(username));
            changed.push("username");
        }
    }
    if let Some(email) = email {
        sets.push("email = ?");
        binds.push(Bind::OptText(email));
        if email_changed {
            changed.push("email");
        }
    }
    if let Some(password) = request.password {
        sets.push("password_hash = ?");
        binds.push(Bind::Text(hash_password(&password)?));
        // The password never reaches the audit trail; a dedicated event records it.
    }
    if request.role.is_some() {
        sets.push("role = ?");
        binds.push(Bind::Text(new_role.as_str().to_string()));
        if new_role != current_role {
            changed.push("role");
        }
    }
    if request.activated.is_some() {
        sets.push("activated = ?");
        binds.push(Bind::Int(i64::from(new_active)));
        if new_active != current_active {
            changed.push("activated");
        }
    }
    if let Some(age_limit) = request.age_limit {
        if let Some(limit) = age_limit {
            if limit > 21 {
                return Err(ApiError::bad_request(
                    "invalid_age_limit",
                    "Age limit must be between 0 and 21",
                ));
            }
        }
        sets.push("age_limit = ?");
        binds.push(Bind::OptInt(age_limit.map(i64::from)));
        if target.age_limit.map(|value| value as u8) != age_limit {
            changed.push("age_limit");
        }
    }
    if sets.is_empty() {
        return Ok(Json(target.to_public()));
    }
    sets.push("updated_at = ?");
    binds.push(Bind::Text(db::db_time(db::now())));
    binds.push(Bind::Int(id));

    let sql = format!("UPDATE users SET {} WHERE id = ?", sets.join(", "));
    bind_query(sqlx::query(&sql), &binds)
        .execute(&state.pool)
        .await
        .map_err(|error| {
            if super::system::is_unique_violation(&error) {
                ApiError::conflict("username_taken", "Username is already taken")
            } else {
                ApiError::from(error)
            }
        })?;

    let updated = fetch_user(&state, id).await?;
    if password_changed {
        state.events.emit(
            Event::new(actions::USER_PASSWORD_RESET, caller.actor())
                .target("user", id.to_string())
                .metadata(json!({ "username": updated.username })),
        );
    }
    if !changed.is_empty() {
        state.events.emit(
            Event::new(actions::USER_UPDATED, caller.actor())
                .target("user", id.to_string())
                .metadata(json!({
                    "username": updated.username,
                    "role": new_role.as_str(),
                    "activated": new_active,
                    "fields": changed,
                })),
        );
    }
    Ok(Json(updated.to_public()))
}

async fn delete_user(
    State(state): State<AppState>,
    caller: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    caller.require_admin_access()?;
    if id == caller.id {
        return Err(ApiError::conflict(
            "cannot_delete_self",
            "You cannot delete your own account",
        ));
    }
    let target = fetch_user(&state, id).await?;
    if target.role == Role::Admin.as_str() && target.activated != 0 {
        ensure_other_active_admin(&state, id).await?;
    }
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    state.events.emit(
        Event::new(actions::USER_DELETED, caller.actor())
            .target("user", id.to_string())
            .metadata(json!({
                "username": target.username,
                "role": target.role,
            })),
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn fetch_user(state: &AppState, id: i64) -> Result<UserRow, ApiError> {
    sqlx::query_as::<_, UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path,
                age_limit, created_at, updated_at, oidc_issuer, oidc_subject
         FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("user_not_found", "User not found"))
}

async fn ensure_other_active_admin(state: &AppState, excluded_id: i64) -> Result<(), ApiError> {
    let others = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM users WHERE role = 'admin' AND activated = 1 AND id != ?",
    )
    .bind(excluded_id)
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;
    if others == 0 {
        return Err(ApiError::conflict(
            "last_admin",
            "The last active administrator cannot be removed or demoted",
        ));
    }
    Ok(())
}
