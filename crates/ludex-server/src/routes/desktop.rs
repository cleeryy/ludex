//! Public desktop download metadata (#164).
//!
//! `GET /api/desktop/download` is unauthenticated on purpose: a visitor who
//! cannot sign in yet still needs a way to install the client. It answers with
//! links only (never the binary) and relies on the global per-IP API rate
//! limiter plus the ten-minute release cache for upstream protection.

use axum::extract::{Query, State};
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderMap, HeaderValue};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use crate::desktop_download::{self, DesktopDownload, Platform};
use crate::error::ApiError;
use crate::state::AppState;

/// Desktop download routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/desktop/download", get(download))
}

#[derive(Debug, Deserialize)]
struct DownloadQuery {
    #[serde(default)]
    platform: Option<String>,
}

async fn download(
    State(state): State<AppState>,
    Query(query): Query<DownloadQuery>,
) -> Result<(HeaderMap, Json<DesktopDownload>), ApiError> {
    let platform = match query.platform.as_deref() {
        None => None,
        Some(value) if value.trim().is_empty() => None,
        Some(value) => Some(Platform::parse(value).ok_or_else(|| {
            ApiError::bad_request(
                "invalid_platform",
                "platform must be windows, linux or macos",
            )
        })?),
    };

    let download = desktop_download::download(&state.http, &state.config, platform).await;

    let mut headers = HeaderMap::new();
    // The server cache is authoritative (10 min by default); let browsers keep
    // a copy for half that window without ever holding a stale installer.
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    Ok((headers, Json(download)))
}
