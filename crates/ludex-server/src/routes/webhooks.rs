//! Administrator-only webhook management.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde_json::json;

use crate::audit::actions;
use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, JsonBody};
use crate::models::{
    CreateWebhookRequest, CreateWebhookResponse, UpdateWebhookRequest, UpdateWebhookResponse,
    Webhook, WebhookRow, WebhookTestResponse,
};
use crate::sql::{bind_query, Bind};
use crate::state::AppState;
use crate::webhooks;

const WEBHOOK_COLUMNS: &str = "id, url, secret, events, enabled, created_at";

/// Webhook routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/webhooks", get(list).post(create))
        .route("/webhooks/{id}", patch(update).delete(remove))
        .route("/webhooks/{id}/test", post(test))
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<Webhook>>, ApiError> {
    user.require_admin_access()?;
    let rows = sqlx::query_as::<_, WebhookRow>(&format!(
        "SELECT {WEBHOOK_COLUMNS} FROM webhooks ORDER BY id DESC"
    ))
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(Json(rows.iter().map(WebhookRow::to_public).collect()))
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<CreateWebhookRequest>,
) -> Result<(StatusCode, Json<CreateWebhookResponse>), ApiError> {
    user.require_admin_access()?;
    let url = webhooks::validate_url(&request.url, state.config.webhooks_allow_private).await?;
    let events = webhooks::validate_events(&request.events)?;
    let enabled = request.enabled.unwrap_or(true);
    let secret = webhooks::generate_secret();
    let now = db::db_time(db::now());

    let result = sqlx::query(
        "INSERT INTO webhooks (url, secret, events, enabled, created_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&url)
    .bind(&secret)
    .bind(webhooks::events_json(&events))
    .bind(i64::from(enabled))
    .bind(&now)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    let id = result.last_insert_rowid();

    state.events.emit(
        Event::new(actions::WEBHOOK_CREATED, user.actor())
            .target("webhook", id.to_string())
            .metadata(json!({ "url": url, "events": events, "enabled": enabled })),
    );

    let row = sqlx::query_as::<_, WebhookRow>(&format!(
        "SELECT {WEBHOOK_COLUMNS} FROM webhooks WHERE id = ?"
    ))
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;

    Ok((
        StatusCode::CREATED,
        Json(CreateWebhookResponse {
            webhook: row.to_public(),
            secret,
        }),
    ))
}

async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    JsonBody(request): JsonBody<UpdateWebhookRequest>,
) -> Result<Json<UpdateWebhookResponse>, ApiError> {
    user.require_admin_access()?;
    let current = fetch_webhook(&state, id).await?;

    let mut sets: Vec<&str> = Vec::new();
    let mut binds: Vec<Bind> = Vec::new();
    let mut changed = serde_json::Map::new();
    let mut new_secret: Option<String> = None;

    if let Some(url) = request.url {
        let url = webhooks::validate_url(&url, state.config.webhooks_allow_private).await?;
        sets.push("url = ?");
        binds.push(Bind::Text(url.clone()));
        changed.insert("url".to_string(), json!(url));
    }
    if let Some(events) = request.events {
        let events = webhooks::validate_events(&events)?;
        sets.push("events = ?");
        binds.push(Bind::Text(webhooks::events_json(&events)));
        changed.insert("events".to_string(), json!(events));
    }
    if let Some(enabled) = request.enabled {
        sets.push("enabled = ?");
        binds.push(Bind::Int(i64::from(enabled)));
        changed.insert("enabled".to_string(), json!(enabled));
    }
    if request.rotate_secret.unwrap_or(false) {
        let secret = webhooks::generate_secret();
        sets.push("secret = ?");
        binds.push(Bind::Text(secret.clone()));
        changed.insert("secretRotated".to_string(), json!(true));
        new_secret = Some(secret);
    }

    if !sets.is_empty() {
        binds.push(Bind::Int(id));
        let sql = format!("UPDATE webhooks SET {} WHERE id = ?", sets.join(", "));
        bind_query(sqlx::query(&sql), &binds)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?;
        state.events.emit(
            Event::new(actions::WEBHOOK_UPDATED, user.actor())
                .target("webhook", id.to_string())
                .metadata(json!({ "changes": changed, "url": current.url })),
        );
    }

    let row = fetch_webhook(&state, id).await?;
    Ok(Json(UpdateWebhookResponse {
        webhook: row.to_public(),
        secret: new_secret,
    }))
}

async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_admin_access()?;
    let row = fetch_webhook(&state, id).await?;
    sqlx::query("DELETE FROM webhooks WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    state.events.emit(
        Event::new(actions::WEBHOOK_DELETED, user.actor())
            .target("webhook", id.to_string())
            .metadata(json!({ "url": row.url })),
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn test(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<(StatusCode, Json<WebhookTestResponse>), ApiError> {
    user.require_admin_access()?;
    let delivery_id = webhooks::queue_test(&state, id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(WebhookTestResponse { delivery_id }),
    ))
}

async fn fetch_webhook(state: &AppState, id: i64) -> Result<WebhookRow, ApiError> {
    sqlx::query_as::<_, WebhookRow>(&format!(
        "SELECT {WEBHOOK_COLUMNS} FROM webhooks WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("webhook_not_found", "Webhook not found"))
}
