//! ZIP manifest and per-entry stream routes (#115).

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::Json;
use ludex_core::archive::StreamManifest;

use crate::error::ApiError;
use crate::extract::{ApiPath, ApiQuery};
use crate::models::StreamQuery;
use crate::state::AppState;
use crate::transfer::zip;

use super::download::authorized_version;

/// `GET /api/games/{id}/versions/{versionId}/stream/manifest`
///
/// Ordered ZIP manifest for a resumable per-entry install. Same authorization
/// as `/stream`: a Bearer token or a download ticket. Non-ZIP archives answer
/// `415 stream_unsupported`, which is the client's signal to fall back to the
/// sequential tar stream or the normal resumable download.
pub(super) async fn stream_manifest(
    State(state): State<AppState>,
    ApiPath((game_id, version_id)): ApiPath<(i64, i64)>,
    ApiQuery(query): ApiQuery<StreamQuery>,
    headers: HeaderMap,
) -> Result<Json<StreamManifest>, ApiError> {
    let (version, _lease) = authorized_version(
        &state,
        &headers,
        game_id,
        Some(version_id),
        query.token.as_deref(),
    )
    .await?;
    zip::manifest(&state, &version).await
}

/// `GET /api/games/{id}/versions/{versionId}/stream/entries/{index}`
///
/// Raw content of one ZIP entry, with single-Range support so an interrupted
/// entry resumes at its byte offset. A compressed entry past the server's
/// decompression budget answers `409 entry_resume_restart`; `?restart=1`
/// streams it from byte 0 (#309).
pub(super) async fn stream_entry(
    State(state): State<AppState>,
    ApiPath((game_id, version_id, index)): ApiPath<(i64, i64, usize)>,
    ApiQuery(query): ApiQuery<StreamQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _download = state.ops.metrics.download_started();
    let (version, _lease) = authorized_version(
        &state,
        &headers,
        game_id,
        Some(version_id),
        query.token.as_deref(),
    )
    .await?;
    zip::entry(
        &state,
        game_id,
        &version,
        index,
        &headers,
        query.restart_requested(),
    )
    .await
}
