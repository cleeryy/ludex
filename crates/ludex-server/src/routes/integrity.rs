//! Administrator-only integrity check routes (#43).

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::audit::actions;
use crate::auth::AuthUser;
use crate::content_hash::{self, ContentHashStatus};
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, ApiQuery, JsonBody};
use crate::integrity::{self, IntegrityReport, IntegrityScope};
use crate::models::{IntegrityCheckRequest, IntegrityCheckResponse, IntegrityReportDetail};
use crate::state::AppState;

/// Integrity routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/integrity/check", post(check))
        .route("/integrity/reports", get(list))
        .route("/integrity/reports/{id}", get(read))
        .route(
            "/integrity/content-hashes",
            get(content_hashes).post(start_content_hashes),
        )
}

/// `GET /api/integrity/content-hashes` — full archive hash backfill state
/// (#216). Admin only: it exposes nothing beyond counts, but it belongs with
/// the other integrity screens.
async fn content_hashes(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<ContentHashStatus>, ApiError> {
    user.require_admin_access()?;
    let mut status = state.content_hashes.status();
    status.pending = content_hash::pending_count(&state).await;
    Ok(Json(status))
}

/// Starts a full backfill in the background. `202` when the run was queued;
/// `200` with the live status when a run was already active (nothing is lost:
/// the active run keeps picking up pending rows).
async fn start_content_hashes(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<(StatusCode, Json<ContentHashStatus>), ApiError> {
    user.require_admin_access()?;
    let started = content_hash::schedule(&state, 0);
    let mut status = state.content_hashes.status();
    status.pending = content_hash::pending_count(&state).await;
    let code = if started {
        StatusCode::ACCEPTED
    } else {
        StatusCode::OK
    };
    Ok((code, Json(status)))
}

async fn check(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<IntegrityCheckRequest>,
) -> Result<(StatusCode, Json<IntegrityCheckResponse>), ApiError> {
    user.require_admin_access()?;
    let scope = match request.scope.as_deref() {
        Some(value) => IntegrityScope::parse(value).ok_or_else(|| {
            ApiError::bad_request(
                "invalid_scope",
                "Scope must be 'all', 'archives' or 'media'",
            )
        })?,
        None => IntegrityScope::All,
    };
    let report_id = integrity::start(&state, scope).await?;
    state.events.emit(
        Event::new(actions::INTEGRITY_CHECK_STARTED, user.actor())
            .target("integrity_report", report_id.to_string())
            .metadata(serde_json::json!({ "scope": scope.as_str() })),
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(IntegrityCheckResponse {
            report_id,
            status: "running".to_string(),
        }),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    #[serde(default)]
    limit: Option<i64>,
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<Vec<IntegrityReport>>, ApiError> {
    user.require_admin_access()?;
    let limit = query.limit.unwrap_or(20);
    Ok(Json(integrity::list_reports(&state.pool, limit).await?))
}

async fn read(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<IntegrityReportDetail>, ApiError> {
    user.require_admin_access()?;
    let (report, findings) = integrity::read_report(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("report_not_found", "Integrity report not found"))?;
    Ok(Json(IntegrityReportDetail { report, findings }))
}
