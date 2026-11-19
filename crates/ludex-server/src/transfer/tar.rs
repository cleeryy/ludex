//! Sequential tar stream of an archive (#204).

use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use ludex_core::archive::{archive_tar_size, detect_format, ArchiveFormat};

use crate::error::ApiError;
use crate::media::content_disposition;
use crate::models::VersionRow;
use crate::state::AppState;
use crate::stream::archive_tar_body;

use super::object::{cleanup_staged, resolve_archive_path};
use super::{map_archive_error, stream_unsupported};

/// Streams a version as a tar archive, staging an object-store archive first
/// because the tar reader needs random access.
pub(crate) async fn serve(
    state: &AppState,
    game_id: i64,
    version: &VersionRow,
) -> Result<Response, ApiError> {
    // Stream install needs random access (ZIP central directory, RAR headers):
    // an object-store archive is staged under `{data_dir}/tmp` and removed once
    // the tar stream ends. Disk use is bounded by one archive at a time per
    // concurrent stream request.
    let mut staged = false;
    let path = resolve_archive_path(state, version, &mut staged).await?;
    let cleanup = staged.then(|| path.clone());

    let format = match detect_format(&path) {
        Ok(format) => format,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            cleanup_staged(&path, staged).await;
            return Err(ApiError::not_found(
                "file_not_found",
                "The library file is missing from disk",
            ));
        }
        Err(error) => {
            cleanup_staged(&path, staged).await;
            return Err(ApiError::unprocessable(
                "stream_failed",
                format!("Could not inspect the archive: {error}"),
            ));
        }
    };
    if format == ArchiveFormat::Unknown {
        cleanup_staged(&path, staged).await;
        return Err(stream_unsupported());
    }

    let size_path = path.clone();
    let tar_size = match tokio::task::spawn_blocking(move || archive_tar_size(&size_path)).await {
        Ok(Ok(size)) => size,
        Ok(Err(error)) => {
            cleanup_staged(&path, staged).await;
            return Err(map_archive_error(error));
        }
        Err(error) => {
            cleanup_staged(&path, staged).await;
            return Err(ApiError::unprocessable(
                "stream_failed",
                format!("Archive size task failed: {error}"),
            ));
        }
    };

    // Filename of the tar: `<title>.tar`, falling back to `game.tar` when the
    // title is blank.
    let title = crate::db::query_scalar::<String>("SELECT title FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_optional(&state.core.pool)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("game_not_found", "Game not found"))?;
    let file_name = if title.trim().is_empty() {
        "game.tar".to_string()
    } else {
        format!("{}.tar", title.trim())
    };
    let disposition = HeaderValue::from_str(&content_disposition(&file_name))
        .map_err(|_| ApiError::internal())?;

    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-tar"),
        )
        .header(header::CONTENT_LENGTH, tar_size)
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(
            header::CACHE_CONTROL,
            // `no-transform` stops Caddy/nginx from re-encoding the tar (#310).
            HeaderValue::from_static("private, no-store, no-transform"),
        )
        // A reverse proxy must not buffer a multi-gigabyte stream: the client
        // needs each chunk as it is produced (#204). Caddy ignores this
        // header; `Incremental` below covers it.
        .header(
            HeaderName::from_static("x-accel-buffering"),
            HeaderValue::from_static("no"),
        )
        // RFC 10036 incremental forwarding: Caddy 2.11.7+ forwards the
        // response head before the first body byte (#310).
        .header(
            HeaderName::from_static("incremental"),
            HeaderValue::from_static("?1"),
        );
    if let Some(digest) = version.content_sha256.as_deref() {
        builder = builder.header(crate::content_hash::ARCHIVE_SHA256_HEADER, digest);
    }
    builder
        .body(archive_tar_body(path, tar_size, cleanup))
        .map_err(|_| ApiError::internal())
}
