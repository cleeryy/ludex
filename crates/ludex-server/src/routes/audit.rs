//! Administrator-only audit trail browsing.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::extract::ApiQuery;
use crate::models::{AuditLogRow, AuditPage, AuditQuery};
use crate::sql::{bind_query_as, Bind};
use crate::state::AppState;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

const AUDIT_COLUMNS: &str =
    "id, actor_id, actor_kind, action, target_type, target_id, metadata, ip, created_at";

/// Audit routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/audit", get(list))
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<AuditQuery>,
) -> Result<Json<AuditPage>, ApiError> {
    user.require_admin_access()?;

    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut filters: Vec<&str> = Vec::new();
    let mut binds: Vec<Bind> = Vec::new();

    if let Some(before) = query.before {
        filters.push("id < ?");
        binds.push(Bind::Int(before));
    }
    if let Some(actor_id) = query.actor_id {
        filters.push("actor_id = ?");
        binds.push(Bind::Int(actor_id));
    }
    if let Some(actor_kind) = query.actor_kind {
        let actor_kind = actor_kind.trim().to_ascii_lowercase();
        if !matches!(actor_kind.as_str(), "user" | "api_key" | "system") {
            return Err(ApiError::bad_request(
                "invalid_actor_kind",
                "actorKind must be user, api_key or system",
            ));
        }
        filters.push("actor_kind = ?");
        binds.push(Bind::Text(actor_kind));
    }
    if let Some(action) = query.action {
        filters.push("action = ?");
        binds.push(Bind::Text(action));
    }
    if let Some(target_type) = query.target_type {
        filters.push("target_type = ?");
        binds.push(Bind::Text(target_type));
    }
    if let Some(target_id) = query.target_id {
        filters.push("target_id = ?");
        binds.push(Bind::Text(target_id));
    }

    let where_clause = if filters.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", filters.join(" AND "))
    };
    // Fetch one extra row to know whether another page exists.
    binds.push(Bind::Int(limit + 1));

    let sql =
        format!("SELECT {AUDIT_COLUMNS} FROM audit_logs {where_clause} ORDER BY id DESC LIMIT ?");
    let mut rows = bind_query_as::<AuditLogRow>(crate::db::query_as(&sql), &binds)
        .fetch_all(&state.pool)
        .await
        .map_err(ApiError::from)?;

    let has_more = rows.len() > usize::try_from(limit).unwrap_or(0);
    if has_more {
        rows.truncate(usize::try_from(limit).unwrap_or(0));
    }
    let next_before = has_more.then(|| rows.last().map(|row| row.id)).flatten();

    Ok(Json(AuditPage {
        items: rows.iter().map(AuditLogRow::to_public).collect(),
        next_before,
    }))
}
