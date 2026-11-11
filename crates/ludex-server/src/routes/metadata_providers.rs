//! Administrator-only metadata provider management (#28).
//!
//! Providers are HTTP sidecars: no third-party code runs inside the server.
//! The bearer token is write-only, encrypted at rest and never serialised by
//! any endpoint; `secretConfigured` tells the UI whether one is stored.

use std::time::Instant;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::audit::actions;
use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, JsonBody};
use crate::providers::{
    self, MetadataProviderConfig, MetadataProviderView, ProviderRecord, ProviderTestResponse,
};
use crate::state::AppState;

const PROVIDER_COLUMNS: &str =
    "id, name, base_url, token_enc, priority, enabled, mapping_json, created_at, updated_at";
const DEFAULT_PRIORITY: i64 = 100;
const MIN_PRIORITY: i64 = -1_000_000;
const MAX_PRIORITY: i64 = 1_000_000;

/// Admin metadata provider routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/metadata-providers", get(list).post(create))
        .route("/admin/metadata-providers/reorder", post(reorder))
        .route("/admin/metadata-providers/{id}", put(update).delete(remove))
        .route("/admin/metadata-providers/{id}/test", post(test))
}

/// `POST /api/admin/metadata-providers` request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateMetadataProviderRequest {
    /// Unique, human-readable name.
    pub name: String,
    /// Base URL; `/search` and `/games/{id}` are appended.
    pub base_url: String,
    /// Optional write-only bearer token.
    #[serde(default)]
    pub token: Option<String>,
    /// Lower runs first. Defaults to 100.
    #[serde(default)]
    pub priority: Option<i64>,
    /// Defaults to `false`: a new provider stays inactive until enabled.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Canonical field -> dot path in a non-canonical response.
    #[serde(default)]
    pub mapping: Option<Value>,
}

/// `PUT /api/admin/metadata-providers/{id}` request.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMetadataProviderRequest {
    /// New unique name.
    #[serde(default)]
    pub name: Option<String>,
    /// New base URL.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Write-only replacement token. Absent or blank keeps the stored one.
    #[serde(default)]
    pub token: Option<String>,
    /// Remove the stored token.
    #[serde(default)]
    pub clear_token: bool,
    /// New priority.
    #[serde(default)]
    pub priority: Option<i64>,
    /// New enabled flag.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Replacement mapping object.
    #[serde(default)]
    pub mapping: Option<Value>,
}

/// `POST /api/admin/metadata-providers/{id}/test` request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestMetadataProviderRequest {
    /// Title sent as `?title=`.
    pub title: String,
    /// Optional release year sent as `?year=`.
    #[serde(default)]
    pub year: Option<u16>,
}

/// `POST /api/admin/metadata-providers/reorder` request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReorderMetadataProvidersRequest {
    /// Provider ids in the desired order; priorities are rewritten 10, 20, ...
    pub ids: Vec<i64>,
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<MetadataProviderView>>, ApiError> {
    user.require_admin_access()?;
    let rows = providers::load_records(&state.pool)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(rows.iter().map(ProviderRecord::to_view).collect()))
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<CreateMetadataProviderRequest>,
) -> Result<(StatusCode, Json<MetadataProviderView>), ApiError> {
    user.require_admin_access()?;
    let name = providers::validate_name(&request.name)?;
    ensure_name_available(&state, &name, None).await?;
    let base_url = resolve_base_url(&state, &request.base_url).await?;
    let priority = validate_priority(request.priority.unwrap_or(DEFAULT_PRIORITY))?;
    let enabled = request.enabled.unwrap_or(false);
    let mapping = match request.mapping.as_ref() {
        Some(value) => providers::validate_mapping(value)?,
        None => providers::Mapping::new(),
    };
    let mapping_json = serde_json::to_string(&mapping).unwrap_or_else(|_| "{}".to_string());
    let token_enc = match request.token.as_deref().map(str::trim) {
        Some(token) if !token.is_empty() => {
            Some(providers::encrypt_token(&state.jwt_secret, token)?)
        }
        _ => None,
    };
    let now = db::db_time(db::now());

    let id = db::query(
        "INSERT INTO metadata_providers
            (name, base_url, token_enc, priority, enabled, mapping_json, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&name)
    .bind(&base_url)
    .bind(token_enc.as_deref())
    .bind(priority)
    .bind(i64::from(enabled))
    .bind(&mapping_json)
    .bind(&now)
    .bind(&now)
    .returning_id(&state.pool)
    .await
    .map_err(map_unique_error)?;

    state.events.emit(
        Event::new(actions::METADATA_PROVIDER_UPDATED, user.actor())
            .target("metadata_provider", id.to_string())
            .metadata(json!({
                "id": id,
                "name": name,
                "enabled": enabled,
                "priority": priority,
                "fields": ["created"],
            })),
    );
    reload(&state).await;
    Ok((StatusCode::CREATED, Json(fetch_view(&state, id).await?)))
}

async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    JsonBody(request): JsonBody<UpdateMetadataProviderRequest>,
) -> Result<Json<MetadataProviderView>, ApiError> {
    user.require_admin_access()?;
    let current = fetch_record(&state, id).await?;
    let mut sets: Vec<&str> = Vec::new();
    let mut binds: Vec<crate::sql::Bind> = Vec::new();
    let mut changed: Vec<String> = Vec::new();
    let mut name = current.name.clone();
    let mut enabled = current.enabled != 0;
    let mut priority = current.priority;

    if let Some(raw) = request.name.as_deref() {
        name = providers::validate_name(raw)?;
        if name != current.name {
            ensure_name_available(&state, &name, Some(id)).await?;
            sets.push("name = ?");
            binds.push(crate::sql::Bind::Text(name.clone()));
            changed.push("name".to_string());
        }
    }
    if let Some(raw) = request.base_url.as_deref() {
        let base_url = resolve_base_url(&state, raw).await?;
        sets.push("base_url = ?");
        binds.push(crate::sql::Bind::Text(base_url));
        changed.push("baseUrl".to_string());
    }
    if let Some(value) = request.priority {
        priority = validate_priority(value)?;
        sets.push("priority = ?");
        binds.push(crate::sql::Bind::Int(priority));
        changed.push("priority".to_string());
    }
    if let Some(value) = request.enabled {
        enabled = value;
        sets.push("enabled = ?");
        binds.push(crate::sql::Bind::Int(i64::from(enabled)));
        changed.push("enabled".to_string());
    }
    if let Some(value) = request.mapping.as_ref() {
        let mapping = providers::validate_mapping(value)?;
        let mapping_json = serde_json::to_string(&mapping).unwrap_or_else(|_| "{}".to_string());
        sets.push("mapping_json = ?");
        binds.push(crate::sql::Bind::Text(mapping_json));
        changed.push("mapping".to_string());
    }
    let replacement = request
        .token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty());
    if let Some(token) = replacement {
        let encrypted = providers::encrypt_token(&state.jwt_secret, token)?;
        sets.push("token_enc = ?");
        binds.push(crate::sql::Bind::Text(encrypted));
        changed.push("token".to_string());
    } else if request.clear_token {
        sets.push("token_enc = NULL");
        changed.push("token".to_string());
    }

    if !sets.is_empty() {
        sets.push("updated_at = ?");
        binds.push(crate::sql::Bind::Text(db::db_time(db::now())));
        binds.push(crate::sql::Bind::Int(id));
        let sql = format!(
            "UPDATE metadata_providers SET {} WHERE id = ?",
            sets.join(", ")
        );
        crate::sql::bind_query(db::query(&sql), &binds)
            .execute(&state.pool)
            .await
            .map_err(map_unique_error)?;
        state.events.emit(
            Event::new(actions::METADATA_PROVIDER_UPDATED, user.actor())
                .target("metadata_provider", id.to_string())
                .metadata(json!({
                    "id": id,
                    "name": name,
                    "enabled": enabled,
                    "priority": priority,
                    "fields": changed,
                })),
        );
        reload(&state).await;
    }

    Ok(Json(fetch_view(&state, id).await?))
}

async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_admin_access()?;
    let row = fetch_record(&state, id).await?;
    // SQLite may run without `foreign_keys`; delete history explicitly so a
    // removed provider never leaves rows behind.
    db::query("DELETE FROM provider_fetches WHERE provider_id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    db::query("DELETE FROM metadata_providers WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    state.events.emit(
        Event::new(actions::METADATA_PROVIDER_UPDATED, user.actor())
            .target("metadata_provider", id.to_string())
            .metadata(json!({
                "id": id,
                "name": row.name,
                "enabled": row.enabled != 0,
                "priority": row.priority,
                "fields": ["deleted"],
            })),
    );
    reload(&state).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn reorder(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<ReorderMetadataProvidersRequest>,
) -> Result<Json<Vec<MetadataProviderView>>, ApiError> {
    user.require_admin_access()?;
    if request.ids.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_provider_order",
            "At least one provider id is required",
        ));
    }
    let known: Vec<i64> = providers::load_records(&state.pool)
        .await
        .map_err(ApiError::from)?
        .into_iter()
        .map(|row| row.id)
        .collect();
    if request.ids.iter().any(|id| !known.contains(id)) {
        return Err(ApiError::bad_request(
            "invalid_provider_order",
            "Unknown provider id in the requested order",
        ));
    }
    let mut transaction = state.pool.begin().await.map_err(ApiError::from)?;
    for (index, id) in request.ids.iter().enumerate() {
        let priority = i64::try_from(index + 1).unwrap_or(1) * 10;
        db::query("UPDATE metadata_providers SET priority = ?, updated_at = ? WHERE id = ?")
            .bind(priority)
            .bind(db::db_time(db::now()))
            .bind(id)
            .execute(&mut transaction)
            .await
            .map_err(ApiError::from)?;
    }
    transaction.commit().await.map_err(ApiError::from)?;
    state.events.emit(
        Event::new(actions::METADATA_PROVIDER_UPDATED, user.actor())
            .target("metadata_providers", "reorder")
            .metadata(json!({
                "ids": request.ids,
                "fields": ["priority"],
            })),
    );
    reload(&state).await;
    list(State(state), user).await
}

async fn test(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    JsonBody(request): JsonBody<TestMetadataProviderRequest>,
) -> Result<Json<ProviderTestResponse>, ApiError> {
    user.require_admin_access()?;
    let title = request.title.trim();
    if title.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_provider_query",
            "A title is required to test a provider",
        ));
    }
    let record = fetch_record(&state, id).await?;
    let config = MetadataProviderConfig::from_record(
        &record,
        &state.jwt_secret,
        state.config.providers_allow_private,
    )
    .map_err(ApiError::from)?;

    let started = Instant::now();
    let result = providers::search(&config, title, request.year).await;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    providers::record_fetch(&state.pool, id, None, result.as_ref().map(Vec::len)).await;
    Ok(Json(match result {
        Ok(candidates) => ProviderTestResponse {
            ok: true,
            candidates,
            error: None,
            status_code: Some(200),
            elapsed_ms,
        },
        Err(error) => ProviderTestResponse {
            ok: false,
            candidates: Vec::new(),
            error: Some(error.to_string()),
            status_code: error.status_code(),
            elapsed_ms,
        },
    }))
}

async fn resolve_base_url(state: &AppState, raw: &str) -> Result<String, ApiError> {
    providers::resolve_target(raw.trim(), state.config.providers_allow_private)
        .await
        .map(|(url, _)| url.as_str().trim_end_matches('/').to_string())
        .map_err(|error| ApiError::bad_request("invalid_provider_url", error))
}

fn validate_priority(priority: i64) -> Result<i64, ApiError> {
    if (MIN_PRIORITY..=MAX_PRIORITY).contains(&priority) {
        Ok(priority)
    } else {
        Err(ApiError::bad_request(
            "invalid_provider_priority",
            format!("Priority must be between {MIN_PRIORITY} and {MAX_PRIORITY}"),
        ))
    }
}

async fn ensure_name_available(
    state: &AppState,
    name: &str,
    exclude: Option<i64>,
) -> Result<(), ApiError> {
    let count = db::query_scalar::<i64>(
        "SELECT COUNT(*) FROM metadata_providers WHERE name = ? AND id != ?",
    )
    .bind(name)
    .bind(exclude.unwrap_or(0))
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;
    if count > 0 {
        return Err(ApiError::bad_request(
            "provider_name_taken",
            format!("A provider named `{name}` already exists"),
        ));
    }
    Ok(())
}

fn map_unique_error(error: sqlx::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("UNIQUE constraint failed") {
        ApiError::bad_request("provider_name_taken", "Provider name already exists")
    } else {
        ApiError::from(error)
    }
}

async fn fetch_record(state: &AppState, id: i64) -> Result<ProviderRecord, ApiError> {
    db::query_as::<ProviderRecord>(&format!(
        "SELECT {PROVIDER_COLUMNS} FROM metadata_providers WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| {
        ApiError::not_found("metadata_provider_not_found", "Metadata provider not found")
    })
}

async fn fetch_view(state: &AppState, id: i64) -> Result<MetadataProviderView, ApiError> {
    Ok(fetch_record(state, id).await?.to_view())
}

/// Replaces the in-memory provider list after a mutation.
async fn reload(state: &AppState) {
    let configs = providers::load_enabled(
        &state.pool,
        &state.jwt_secret,
        state.config.providers_allow_private,
    )
    .await;
    *state.providers.write().await = configs;
}
