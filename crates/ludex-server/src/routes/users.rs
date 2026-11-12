//! Administrator-only user management.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use ludex_core::models::Role;
use serde_json::json;

use crate::audit::actions;
use crate::auth::{generate_password, hash_password, AuthUser};
use crate::db;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, JsonBody};
use crate::models::{
    CreateUserRequest, ResetPasswordRequest, ResetPasswordResult, UpdateUserRequest, User, UserRow,
};
use crate::sql::{bind_query, Bind};
use crate::state::AppState;

/// User management routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users", get(list).post(create))
        .route("/users/{id}", patch(update).delete(delete_user))
        .route("/users/{id}/sessions", delete(revoke_sessions))
        .route("/users/{id}/reset-password", post(reset_password))
}

/// Admin: revoke every session of an account (sign out all devices).
async fn revoke_sessions(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    user.require_admin_access()?;
    let exists = db::query_scalar::<i64>("SELECT COUNT(*) FROM users WHERE id = ?")
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
    let rows = db::query_as::<UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path,
                age_limit, created_at, updated_at, oidc_issuer, oidc_subject,
                onboarding_completed_at, onboarding_step,
         CAST(COALESCE((SELECT 1 FROM user_mfa m WHERE m.user_id = users.id AND m.enabled_at IS NOT NULL), 0) AS BIGINT) AS mfa_enabled,
         must_change_password, locale
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

    let user_id = db::query(
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
    .returning_id(&state.pool)
    .await
    .map_err(|error| {
        if super::system::is_unique_violation(&error) {
            ApiError::conflict("username_taken", "Username is already taken")
        } else {
            ApiError::from(error)
        }
    })?;

    let created = db::query_as::<UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path,
                age_limit, created_at, updated_at, oidc_issuer, oidc_subject,
                onboarding_completed_at, onboarding_step,
         CAST(COALESCE((SELECT 1 FROM user_mfa m WHERE m.user_id = users.id AND m.enabled_at IS NOT NULL), 0) AS BIGINT) AS mfa_enabled,
         must_change_password, locale
         FROM users WHERE id = ?",
    )
    .bind(user_id)
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
    bind_query(db::query(&sql), &binds)
        .execute(&state.pool)
        .await
        .map_err(|error| {
            if super::system::is_unique_violation(&error) {
                ApiError::conflict("username_taken", "Username is already taken")
            } else {
                ApiError::from(error)
            }
        })?;

    if password_changed {
        // The old password must stop working everywhere, not only for the
        // account holder who happens to sign in next (#166).
        crate::auth::revoke_all_sessions(&state, id, "password_reset").await?;
    }
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
    db::query("DELETE FROM users WHERE id = ?")
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

pub(crate) async fn fetch_user(state: &AppState, id: i64) -> Result<UserRow, ApiError> {
    db::query_as::<UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path,
                age_limit, created_at, updated_at, oidc_issuer, oidc_subject,
                onboarding_completed_at, onboarding_step,
         CAST(COALESCE((SELECT 1 FROM user_mfa m WHERE m.user_id = users.id AND m.enabled_at IS NOT NULL), 0) AS BIGINT) AS mfa_enabled,
         must_change_password, locale
         FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("user_not_found", "User not found"))
}

async fn ensure_other_active_admin(state: &AppState, excluded_id: i64) -> Result<(), ApiError> {
    let others = db::query_scalar::<i64>(
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

/// Admin: reset an account password and sign every device out (#166).
///
/// Without a `password` the server generates one and returns it once; the
/// account is flagged `mustChangePassword` by default so the administrator
/// hands over a temporary credential, not a permanent one.
async fn reset_password(
    State(state): State<AppState>,
    caller: AuthUser,
    ApiPath(id): ApiPath<i64>,
    JsonBody(request): JsonBody<ResetPasswordRequest>,
) -> Result<Json<ResetPasswordResult>, ApiError> {
    caller.require_admin_access()?;
    let target = fetch_user(&state, id).await?;
    if target.is_oidc_managed() {
        return Err(ApiError::conflict(
            "managed_by_idp",
            "This account signs in through the identity provider; reset the password there",
        ));
    }
    let require_change = request.require_change.unwrap_or(true);
    let (password, generated) = match request.password {
        Some(password) => {
            super::validate_password(&password)?;
            (password, None)
        }
        None => {
            let password = generate_password();
            (password.clone(), Some(password))
        }
    };
    let password_hash = hash_password(&password)?;
    db::query(
        "UPDATE users SET password_hash = ?, must_change_password = ?, updated_at = ? WHERE id = ?",
    )
    .bind(password_hash)
    .bind(i64::from(u8::from(require_change)))
    .bind(db::db_time(db::now()))
    .bind(id)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let revoked = crate::auth::revoke_all_sessions(&state, id, "admin_password_reset").await?;
    state.events.emit(
        Event::new(actions::USER_PASSWORD_RESET, caller.actor())
            .target("user", id.to_string())
            .metadata(json!({
                "username": target.username,
                "mustChangePassword": require_change,
                "sessions": revoked,
            })),
    );
    Ok(Json(ResetPasswordResult {
        username: target.username,
        password: generated,
        revoked: i64::try_from(revoked).unwrap_or(i64::MAX),
        must_change_password: require_change,
    }))
}
