//! News posts shown on the server home page.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use ludex_core::models::NewsPost;

use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::extract::{ApiPath, JsonBody};
use crate::models::{CreateNewsRequest, NewsRow, UpdateNewsRequest};
use crate::sql::{bind_query, Bind};
use crate::state::AppState;

/// News routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/news", get(list).post(create)).route(
        "/news/{id}",
        axum::routing::patch(update).delete(delete_post),
    )
}

async fn list(
    State(state): State<AppState>,
    _user: AuthUser,
) -> Result<Json<Vec<NewsPost>>, ApiError> {
    let rows = sqlx::query_as::<_, NewsRow>(
        "SELECT n.id, n.title, n.body, u.username AS author, n.created_at, n.updated_at
         FROM news n LEFT JOIN users u ON u.id = n.author_id
         ORDER BY n.created_at DESC, n.id DESC",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(Json(rows.iter().map(to_post).collect()))
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<CreateNewsRequest>,
) -> Result<(StatusCode, Json<NewsPost>), ApiError> {
    user.require_admin()?;
    let title = validate_text(&request.title, "title")?;
    let body = validate_text(&request.body, "body")?;
    let now = db::db_time(db::now());
    let result = sqlx::query(
        "INSERT INTO news (title, body, author_id, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&title)
    .bind(&body)
    .bind(user.id)
    .bind(&now)
    .bind(&now)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let row = fetch(&state, result.last_insert_rowid()).await?;
    Ok((StatusCode::CREATED, Json(to_post(&row))))
}

async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    JsonBody(request): JsonBody<UpdateNewsRequest>,
) -> Result<Json<NewsPost>, ApiError> {
    user.require_admin()?;
    fetch(&state, id).await?;

    let mut sets = Vec::new();
    let mut binds = Vec::new();
    if let Some(title) = request.title {
        sets.push("title = ?");
        binds.push(Bind::Text(validate_text(&title, "title")?));
    }
    if let Some(body) = request.body {
        sets.push("body = ?");
        binds.push(Bind::Text(validate_text(&body, "body")?));
    }
    if !sets.is_empty() {
        sets.push("updated_at = ?");
        binds.push(Bind::Text(db::db_time(db::now())));
        binds.push(Bind::Int(id));
        let sql = format!("UPDATE news SET {} WHERE id = ?", sets.join(", "));
        bind_query(sqlx::query(&sql), &binds)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?;
    }

    let row = fetch(&state, id).await?;
    Ok(Json(to_post(&row)))
}

async fn delete_post(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_admin()?;
    let result = sqlx::query("DELETE FROM news WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("news_not_found", "News post not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn fetch(state: &AppState, id: i64) -> Result<NewsRow, ApiError> {
    sqlx::query_as::<_, NewsRow>(
        "SELECT n.id, n.title, n.body, u.username AS author, n.created_at, n.updated_at
         FROM news n LEFT JOIN users u ON u.id = n.author_id WHERE n.id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("news_not_found", "News post not found"))
}

fn to_post(row: &NewsRow) -> NewsPost {
    NewsPost {
        id: row.id,
        title: row.title.clone(),
        body: row.body.clone(),
        author: row.author.clone().unwrap_or_else(|| "Ludex".to_string()),
        created_at: parse_or_now(&row.created_at),
        updated_at: parse_or_now(&row.updated_at),
    }
}

fn parse_or_now(value: &str) -> DateTime<Utc> {
    db::parse_time(value).unwrap_or_else(Utc::now)
}

fn validate_text(value: &str, field: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_news",
            format!("News {field} cannot be empty"),
        ));
    }
    Ok(value.to_string())
}
