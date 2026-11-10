//! API key management: list, create and revoke.
//!
//! A key belongs to an account. Users manage their own keys; administrators
//! may list and revoke every key. The plaintext token is returned once, at
//! creation, and never persisted.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use ludex_core::models::Role;
use serde_json::json;

use crate::api_keys::{self, Scope};
use crate::audit::actions;
use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, ApiQuery, JsonBody};
use crate::models::{
    ApiKey, ApiKeyQuery, ApiKeyRow, CreateApiKeyRequest, CreateApiKeyResponse, UserRow,
};
use crate::state::AppState;

/// Longest accepted key label.
const MAX_NAME_CHARS: usize = 100;
/// Maximum number of active keys per account.
const MAX_ACTIVE_KEYS: i64 = 50;

const KEY_COLUMNS: &str = "id, user_id, name, prefix, secret_hash, scopes, created_at, \
last_used_at, revoked_at";

/// API key routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/keys", get(list).post(create))
        .route("/keys/{id}", delete(revoke))
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<ApiKeyQuery>,
) -> Result<Json<Vec<ApiKey>>, ApiError> {
    let all = query.all.unwrap_or(false);
    let rows = if all {
        user.require_admin_access()?;
        sqlx::query_as::<_, ApiKeyRow>(&format!(
            "SELECT {KEY_COLUMNS} FROM api_keys ORDER BY id DESC"
        ))
        .fetch_all(&state.pool)
        .await
        .map_err(ApiError::from)?
    } else {
        sqlx::query_as::<_, ApiKeyRow>(&format!(
            "SELECT {KEY_COLUMNS} FROM api_keys WHERE user_id = ? ORDER BY id DESC"
        ))
        .bind(user.id)
        .fetch_all(&state.pool)
        .await
        .map_err(ApiError::from)?
    };
    Ok(Json(rows.iter().map(ApiKeyRow::to_public).collect()))
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<CreateApiKeyRequest>,
) -> Result<(StatusCode, Json<CreateApiKeyResponse>), ApiError> {
    user.require_scope(Scope::Write)?;
    let name = validate_name(&request.name)?;
    let scopes = parse_requested_scopes(request.scopes.as_deref())?;
    let target_id = request.user_id.unwrap_or(user.id);

    if target_id != user.id {
        user.require_admin_access()?;
    }

    let target = sqlx::query_as::<_, UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path,
                age_limit, created_at, updated_at, oidc_issuer, oidc_subject,
                onboarding_completed_at, onboarding_step
         FROM users WHERE id = ?",
    )
    .bind(target_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("user_not_found", "User not found"))?;
    if target.activated == 0 {
        return Err(ApiError::unprocessable(
            "account_disabled",
            "Cannot create an API key for a disabled account",
        ));
    }

    // Never let a non-administrator mint a key that outranks their account.
    if scopes.contains(&Scope::Admin) {
        user.require_admin_access()?;
        let target_role = target.role.parse::<Role>().unwrap_or(Role::User);
        if target_role != Role::Admin {
            return Err(ApiError::forbidden(
                "forbidden",
                "Admin-scoped keys require an administrator account",
            ));
        }
    }

    let active = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM api_keys WHERE user_id = ? AND revoked_at IS NULL",
    )
    .bind(target_id)
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;
    if active >= MAX_ACTIVE_KEYS {
        return Err(ApiError::conflict(
            "too_many_keys",
            format!("An account may hold at most {MAX_ACTIVE_KEYS} active API keys"),
        ));
    }

    let generated = api_keys::generate();
    let now = db::db_time(db::now());
    let result = sqlx::query(
        "INSERT INTO api_keys
            (user_id, name, prefix, secret_hash, scopes, created_at, last_used_at, revoked_at)
         VALUES (?, ?, ?, ?, ?, ?, NULL, NULL)",
    )
    .bind(target_id)
    .bind(&name)
    .bind(&generated.prefix)
    .bind(&generated.secret_hash)
    .bind(api_keys::scopes_json(&scopes))
    .bind(&now)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    let key_id = result.last_insert_rowid();

    state.events.emit(
        Event::new(actions::API_KEY_CREATED, user.actor())
            .target("api_key", key_id.to_string())
            .metadata(json!({
                "name": name,
                "prefix": generated.prefix,
                "scopes": scopes.iter().map(|scope| scope.as_str()).collect::<Vec<_>>(),
                "userId": target_id,
            })),
    );

    let row =
        sqlx::query_as::<_, ApiKeyRow>(&format!("SELECT {KEY_COLUMNS} FROM api_keys WHERE id = ?"))
            .bind(key_id)
            .fetch_one(&state.pool)
            .await
            .map_err(ApiError::from)?;

    Ok((
        StatusCode::CREATED,
        Json(CreateApiKeyResponse {
            key: row.to_public(),
            secret: generated.token,
        }),
    ))
}

async fn revoke(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_scope(Scope::Write)?;
    let row =
        sqlx::query_as::<_, ApiKeyRow>(&format!("SELECT {KEY_COLUMNS} FROM api_keys WHERE id = ?"))
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::not_found("api_key_not_found", "API key not found"))?;

    if row.user_id != user.id {
        user.require_admin_access()?;
    }
    if row.revoked_at.is_some() {
        return Ok(StatusCode::NO_CONTENT);
    }

    let now = db::db_time(db::now());
    sqlx::query("UPDATE api_keys SET revoked_at = ? WHERE id = ?")
        .bind(&now)
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;

    state.events.emit(
        Event::new(actions::API_KEY_REVOKED, user.actor())
            .target("api_key", id.to_string())
            .metadata(json!({
                "name": row.name,
                "prefix": row.prefix,
                "userId": row.user_id,
            })),
    );
    Ok(StatusCode::NO_CONTENT)
}

fn validate_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_name",
            "A key name is required",
        ));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(ApiError::bad_request(
            "invalid_name",
            format!("Key names are limited to {MAX_NAME_CHARS} characters"),
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            "invalid_name",
            "Key name contains invalid characters",
        ));
    }
    Ok(name.to_string())
}

fn parse_requested_scopes(raw: Option<&[String]>) -> Result<Vec<Scope>, ApiError> {
    let Some(names) = raw else {
        return Ok(vec![Scope::Read]);
    };
    if names.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_scopes",
            "At least one scope is required",
        ));
    }
    let mut scopes = Vec::new();
    for name in names {
        let scope = Scope::parse(name).ok_or_else(|| {
            ApiError::bad_request(
                "invalid_scopes",
                format!("Unknown scope '{name}'; expected read, write or admin"),
            )
        })?;
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }
    scopes.sort();
    Ok(scopes)
}
