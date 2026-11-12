//! Administrator-only observability dashboard (#170).
//!
//! One endpoint feeds the in-product admin page. It is admin-only because it
//! aggregates audit activity, webhook failures and storage capacity; anything
//! that must stay public already lives on `/metrics` and `/api/health/details`.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::extract::ApiQuery;
use crate::observability::{self, ObservabilitySnapshot, ObservabilityWindow};
use crate::state::AppState;

/// Observability route.
pub fn router() -> Router<AppState> {
    Router::new().route("/admin/observability", get(snapshot))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotQuery {
    /// `1h` (default) or `24h`.
    #[serde(default)]
    window: Option<String>,
}

async fn snapshot(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<SnapshotQuery>,
) -> Result<Json<ObservabilitySnapshot>, ApiError> {
    user.require_admin_access()?;
    let window = match query.window.as_deref() {
        Some(value) => ObservabilityWindow::parse(value).ok_or_else(|| {
            ApiError::bad_request("invalid_window", "window must be '1h' or '24h'")
        })?,
        None => ObservabilityWindow::DEFAULT,
    };
    Ok(Json(observability::snapshot(&state, window).await))
}
