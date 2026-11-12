//! Library Health Center routes (#165).
//!
//! Reading a report and starting a new one are editor operations; destructive
//! repair actions additionally require an administrator and `confirm: true`.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::audit::actions;
use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiQuery, JsonBody};
use crate::library_health::{
    self, LibraryHealthActionRequest, LibraryHealthActionResult, LibraryHealthResponse,
    LibraryHealthStartResponse,
};
use crate::state::AppState;

/// Library health routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/health/library", get(report))
        .route("/admin/health/library/scan", post(scan))
        .route("/admin/health/library/actions", post(apply))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HealthQuery {
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
}

async fn report(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<HealthQuery>,
) -> Result<Json<LibraryHealthResponse>, ApiError> {
    user.require_editor()?;
    let severity = query
        .severity
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let category = query
        .category
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    Ok(Json(
        library_health::read_latest(
            &state.pool,
            severity,
            category,
            query.limit.unwrap_or(200),
            query.offset.unwrap_or(0),
        )
        .await?,
    ))
}

async fn scan(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<(StatusCode, Json<LibraryHealthStartResponse>), ApiError> {
    user.require_editor()?;
    let report_id = library_health::start(&state, library_health::HealthTrigger::Manual).await?;
    state.events.emit(
        Event::new(actions::HEALTH_SCAN_STARTED, user.actor())
            .target("library_health_report", report_id.to_string())
            .metadata(serde_json::json!({ "trigger": "manual" })),
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(LibraryHealthStartResponse {
            report_id,
            status: "running".to_string(),
        }),
    ))
}

async fn apply(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<LibraryHealthActionRequest>,
) -> Result<Json<LibraryHealthActionResult>, ApiError> {
    user.require_editor()?;
    let action = request.action.trim();
    if library_health::DESTRUCTIVE_ACTIONS.contains(&action) {
        user.require_admin_access()?;
        if !request.confirm {
            return Err(ApiError::bad_request(
                "confirmation_required",
                "This action needs an explicit confirmation",
            ));
        }
    }
    let result = library_health::apply_action(&state, &request).await?;
    state.events.emit(
        Event::new(actions::HEALTH_ACTION_APPLIED, user.actor())
            .target("library_health_report", "latest")
            .metadata(serde_json::json!({
                "action": result.action,
                "applied": result.applied,
                "skipped": result.skipped,
                "confirm": request.confirm,
                "findingIds": request.finding_ids.len(),
                "deletedMediaFiles": result.media_gc.as_ref().map(|gc| gc.deleted_files),
            })),
    );
    Ok(Json(result))
}
