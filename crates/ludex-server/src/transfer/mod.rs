//! Archive transfer plumbing used by the download and stream routes (#237).
//!
//! Moved out of `routes/games.rs` so every path shares one implementation:
//! [`range`] parses `Range`/`If-Range`, [`local`] serves the library file,
//! [`object`] proxies or stages an object-store archive, [`tar`] streams a
//! sequential tar, [`zip`] serves the resumable ZIP manifest and entries, and
//! [`body`] attaches byte accounting plus the per-stream outcome log.

pub(crate) mod body;
pub(crate) mod local;
pub(crate) mod object;
pub(crate) mod range;
pub(crate) mod tar;
pub(crate) mod zip;

#[cfg(test)]
pub(crate) mod test_support;

pub(crate) use body::{log_stream, StreamContext};

use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use ludex_core::archive::ArchiveError;

use crate::error::ApiError;
use crate::models::VersionRow;
use crate::state::AppState;

/// Serves one archive download: object storage first when the row lives there,
/// falling back to the local library file when the object read fails (#168,
/// #211).
pub(crate) async fn serve_archive(
    state: &AppState,
    game_id: i64,
    version: &VersionRow,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    if local::is_s3_version(version) {
        if let Some(key) = version.object_key.as_deref() {
            match object::serve(state, version, key, headers).await {
                Ok(response) => return Ok(response),
                Err(error) if object::can_fall_back_to_local(&error) => {
                    tracing::warn!(
                        game = game_id,
                        version = version.id,
                        object = key,
                        "object storage read failed; trying the local library file"
                    );
                }
                Err(error) => return Err(error),
            }
        }
    }
    local::serve(state, version, headers).await
}

/// `415` answered when a client asks for the ZIP-only stream endpoints on a
/// non-ZIP archive; the client falls back to the normal download.
pub(crate) fn stream_unsupported() -> ApiError {
    ApiError::new(
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "stream_unsupported",
        "Stream install is only available for ZIP archives; use the normal download endpoint",
    )
}

/// Maps an archive failure to the `stream_failed` surface.
pub(crate) fn map_archive_error(error: ArchiveError) -> ApiError {
    match error {
        ArchiveError::PasswordRequired => ApiError::conflict(
            "archive_password_required",
            "This archive is encrypted and cannot be streamed",
        ),
        ArchiveError::UnsupportedFormat => stream_unsupported(),
        ArchiveError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ApiError::not_found("file_not_found", "The library file is missing from disk")
        }
        other => ApiError::unprocessable(
            "stream_failed",
            format!("Could not stream the archive: {other}"),
        ),
    }
}
