//! Administrator-only user management.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, patch};
use axum::{Json, Router};
use ludex_core::models::Role;

use crate::auth::{hash_password, AuthUser};
use crate::db;
use crate::error::ApiError;
use crate::extract::{ApiPath, JsonBody};
use crate::models::{CreateUserRequest, UpdateUserRequest, UserRow};
use crate::sql::{bind_query, Bind};
use crate::state::AppState;

/// User management routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users", get(list).post(create))
        .route("/users/{id}", patch(update).delete(delete_user))
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<ludex_core::models::User>>, ApiError> {
    user.require_admin()?;
    let rows = sqlx::query_as::<_, UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path, created_at, updated_at
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
) -> Result<(StatusCode, Json<ludex_core::models::User>), ApiError> {
    user.require_admin()?;
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
        "SELECT id, username, password_hash, email, role, activated, avatar_path, created_at, updated_at
         FROM users WHERE id = ?",
    )
    .bind(result.last_insert_rowid())
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;

    Ok((StatusCode::CREATED, Json(created.to_public())))
}

async fn update(
    State(state): State<AppState>,
    caller: AuthUser,
    ApiPath(id): ApiPath<i64>,
    JsonBody(request): JsonBody<UpdateUserRequest>,
) -> Result<Json<ludex_core::models::User>, ApiError> {
    caller.require_admin()?;
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

    let mut sets = Vec::new();
    let mut binds = Vec::new();
    if let Some(email) = request.email {
        sets.push("email = ?");
        let email = match email {
            Some(value) => super::validate_email(Some(value))?,
            None => None,
        };
        binds.push(Bind::OptText(email));
    }
    if request.role.is_some() {
        sets.push("role = ?");
        binds.push(Bind::Text(new_role.as_str().to_string()));
    }
    if request.activated.is_some() {
        sets.push("activated = ?");
        binds.push(Bind::Int(i64::from(new_active)));
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
        .map_err(ApiError::from)?;

    let updated = fetch_user(&state, id).await?;
    Ok(Json(updated.to_public()))
}

async fn delete_user(
    State(state): State<AppState>,
    caller: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    caller.require_admin()?;
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
    Ok(StatusCode::NO_CONTENT)
}

async fn fetch_user(state: &AppState, id: i64) -> Result<UserRow, ApiError> {
    sqlx::query_as::<_, UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path, created_at, updated_at
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
