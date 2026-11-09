//! Library scan and folder routes.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};

use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::models::{LibraryFolders, ScanResponse, ScanStatus};
use crate::state::AppState;

/// Library routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/library/scan", get(scan_status).post(scan))
        .route("/library/folders", get(folders))
}

async fn scan_status(State(state): State<AppState>, _user: AuthUser) -> Json<ScanStatus> {
    Json(state.scan.read().await.clone())
}

async fn scan(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<(StatusCode, Json<ScanResponse>), ApiError> {
    user.require_editor()?;
    let started = crate::scanner::trigger(&state).await;
    Ok((StatusCode::ACCEPTED, Json(ScanResponse { started })))
}

async fn folders(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<LibraryFolders>, ApiError> {
    user.require_admin()?;
    Ok(Json(LibraryFolders {
        games_dir: state.config.games_dir.display().to_string(),
        media_dir: state.config.media_dir.display().to_string(),
        data_dir: state.config.data_dir.display().to_string(),
    }))
}
