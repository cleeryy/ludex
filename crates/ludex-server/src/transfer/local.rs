//! Serving an archive from the library root on local disk.

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method};
use axum::response::Response;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::config::StorageBackend;
use crate::error::ApiError;
use crate::media::content_disposition;
use crate::models::VersionRow;
use crate::roots;
use crate::state::AppState;

use super::body::file_etag;
use super::range::if_range_matches;

/// Serves a version from its library root with `ServeFile` (Range; `If-Range`
/// is enforced here).
///
/// The response body is copied in 1 MiB chunks instead of the tower default
/// (64 KiB) to keep the send path cheap on large archives (#194). A field
/// campaign measured 373 MB/s with `dd bs=512K iflag=direct` against 640-1000
/// MB/s with `dd bs=1M` on the same cold zone (#211), so the chunk is 1 MiB.
/// Logging and the part permit are attached by [`crate::transfer::log_stream`]
/// once the response exists.
pub(crate) async fn serve(
    state: &AppState,
    version: &VersionRow,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let path = local_version_path(state, version).await?;
    let metadata = tokio::fs::metadata(&path).await.map_err(|_| {
        ApiError::not_found("file_not_found", "The library file is missing from disk")
    })?;
    // A cheap archive identity: the client compares it across Range resumes
    // and restarts explicitly when the file was replaced (#195).
    let etag = file_etag(&metadata);
    let mut request = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/")
        .body(Body::empty())
        .map_err(|_| ApiError::internal())?;
    // `ServeFile` in tower-http 0.6 ignores `If-Range`, so a client resuming a
    // replaced archive would get 206 bytes of the new file at the old offset.
    // RFC 9110 §13.1.5 says the range is only honoured when the validator
    // matches; otherwise answer the full 200. Comparing here is cheaper than
    // serving a corrupt splice, and a non-matching date is a restart too.
    if if_range_matches(headers, etag.as_deref()) {
        for name in [header::RANGE, header::IF_RANGE] {
            if let Some(value) = headers.get(&name) {
                request.headers_mut().insert(name, value.clone());
            }
        }
    }

    let response = ServeFile::new(&path)
        .with_buf_chunk_size(1024 * 1024)
        .oneshot(request)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "file streaming failed");
            ApiError::internal()
        })?;
    let (mut parts, body) = response.into_parts();
    let disposition = content_disposition(&version.file_name);
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        parts.headers.insert(header::CONTENT_DISPOSITION, value);
    }
    parts.headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    if let Some(etag) = etag {
        if let Ok(value) = HeaderValue::from_str(&etag) {
            parts.headers.insert(header::ETAG, value);
        }
    }
    Ok(Response::from_parts(parts, Body::new(body)))
}

/// Resolves and validates the local file of a version.
pub(crate) async fn local_version_path(
    state: &AppState,
    version: &VersionRow,
) -> Result<PathBuf, ApiError> {
    if version.missing != 0 {
        return Err(ApiError::not_found(
            "file_not_found",
            "This version is no longer available",
        ));
    }
    if version.root_unavailable != 0 {
        return Err(ApiError::not_found(
            "root_unavailable",
            "The library root holding this file is not available",
        ));
    }
    let path = roots::resolve_version_path(
        &state.pool,
        &state.config,
        version.root_id,
        &version.file_path,
    )
    .await
    .ok_or_else(ApiError::internal)?;
    let metadata = tokio::fs::metadata(&path).await.map_err(|_| {
        ApiError::not_found("file_not_found", "The library file is missing from disk")
    })?;
    if !metadata.is_file() {
        return Err(ApiError::not_found(
            "file_not_found",
            "The library file is missing from disk",
        ));
    }
    Ok(path)
}

/// Whether the row must be read from object storage. Rows indexed before S3
/// was enabled keep `storage_backend = local` and are served from disk.
pub(crate) fn is_s3_version(version: &VersionRow) -> bool {
    version.storage_backend.as_deref() == Some(StorageBackend::S3.as_str())
}
