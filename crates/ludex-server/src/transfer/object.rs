//! Object-store transfer: proxying, staging and error mapping.

use std::path::PathBuf;

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;

use crate::error::ApiError;
use crate::media::content_disposition;
use crate::models::VersionRow;
use crate::state::AppState;
use crate::storage::StorageError;

use super::range::parse_range;

/// Whether a failed S3 read may fall back to the local library file.
pub(crate) fn can_fall_back_to_local(error: &ApiError) -> bool {
    matches!(
        error.code.as_str(),
        "file_not_found" | "storage_unavailable"
    )
}

/// Proxies an object-store read, honouring a single HTTP Range.
pub(crate) async fn serve(
    state: &AppState,
    version: &VersionRow,
    key: &str,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let range = match headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
    {
        Some(value) => match parse_range(value) {
            Some(range) => range,
            None => {
                return Err(ApiError::new(
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    "invalid_range",
                    "Only a single byte range is supported",
                ));
            }
        },
        None => None,
    };

    let object = state
        .transfer
        .storage
        .open(key, range)
        .await
        .map_err(|error| map_storage_error(error, "download"))?;

    let content_type = mime_guess::from_path(&version.file_name)
        .first_or_octet_stream()
        .to_string();
    let disposition = content_disposition(&version.file_name);
    let mut builder = Response::builder()
        .status(if object.range.is_some() {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        );
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        builder = builder.header(header::CONTENT_DISPOSITION, value);
    }
    if let Some(digest) = version.content_sha256.as_deref() {
        builder = builder.header(crate::content_hash::ARCHIVE_SHA256_HEADER, digest);
    }
    let length = match object.range {
        Some((start, end)) => {
            builder = builder.header(
                header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{}", object.size),
            );
            end.saturating_sub(start).saturating_add(1)
        }
        None => object.size,
    };
    builder
        .header(header::CONTENT_LENGTH, length)
        .body(object.body)
        .map_err(|_| ApiError::internal())
}

/// Maps storage failures to stable API errors. Upstream details are logged,
/// never returned: they can contain bucket names or signed URL fragments.
pub(crate) fn map_storage_error(error: StorageError, action: &str) -> ApiError {
    match error {
        StorageError::NotFound => ApiError::not_found(
            "file_not_found",
            "The archive is missing from object storage",
        ),
        StorageError::RangeNotSatisfiable => ApiError::new(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "invalid_range",
            "The requested byte range cannot be satisfied",
        ),
        StorageError::Disabled => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "Object storage is not configured",
        ),
        StorageError::Unavailable(detail) => {
            tracing::warn!(%detail, action, "object storage request failed");
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "storage_unavailable",
                "The object storage backend did not answer the request",
            )
        }
    }
}

/// Stages an object in the data directory so the tar streamer can seek.
pub(crate) async fn stage_object(
    state: &AppState,
    version: &VersionRow,
) -> Result<(PathBuf, u64), ApiError> {
    let key = version
        .object_key
        .as_deref()
        .ok_or_else(|| ApiError::not_found("file_not_found", "Object key is missing"))?;
    let directory = state.core.config.data_dir.join("tmp");
    let path = directory.join(format!(
        "stream-{}-{}.archive",
        version.id,
        uuid::Uuid::new_v4()
    ));
    let size = match state.transfer.storage.download_to_path(key, &path).await {
        Ok(size) => size,
        Err(error) => {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(map_storage_error(error, "stream stage"));
        }
    };
    Ok((path, size))
}

/// Resolves the archive path of a version, staging an object-store archive
/// under `{data_dir}/tmp` when the reader needs random access.
///
/// `staged` is set when the caller owns the returned temporary file and must
/// remove it with [`cleanup_staged`].
pub(crate) async fn resolve_archive_path(
    state: &AppState,
    version: &VersionRow,
    staged: &mut bool,
) -> Result<PathBuf, ApiError> {
    if super::local::is_s3_version(version) && version.object_key.is_some() {
        match stage_object(state, version).await {
            Ok((path, _size)) => {
                *staged = true;
                Ok(path)
            }
            Err(error) if can_fall_back_to_local(&error) => {
                tracing::warn!(
                    version = version.id,
                    "object storage staging failed; trying the local library file"
                );
                super::local::local_version_path(state, version).await
            }
            Err(error) => Err(error),
        }
    } else {
        super::local::local_version_path(state, version).await
    }
}

/// Removes a staged object once the response that borrowed it is done.
pub(crate) async fn cleanup_staged(path: &std::path::Path, staged: bool) {
    if staged {
        let _ = tokio::fs::remove_file(path).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::test_support::{state_with_memory_store, version_row};
    use axum::http::header;

    #[tokio::test]
    async fn s3_downloads_stream_ranges_and_missing_objects_are_404() {
        let (temp, state) = state_with_memory_store().await;
        let archive = temp.path().join("Cat Quest II.zip");
        std::fs::write(&archive, b"0123456789abcdef").expect("write");
        let stored = state
            .transfer
            .storage
            .publish_archive("identity", "Cat Quest II.zip", &archive)
            .await
            .expect("upload")
            .expect("object");
        let version = version_row(Some(stored.key.clone()), "s3");

        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, "bytes=2-5".parse().expect("range"));
        let response = serve(&state, &version, &stored.key, &headers)
            .await
            .expect("ranged download");
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok()),
            Some("bytes 2-5/16")
        );
        let body = http_body_util::BodyExt::collect(response.into_body())
            .await
            .expect("body")
            .to_bytes();
        assert_eq!(&body[..], b"2345");

        let missing = serve(&state, &version, "archives/missing.zip", &headers)
            .await
            .expect_err("missing object");
        assert_eq!(missing.status, StatusCode::NOT_FOUND);
        assert_eq!(missing.code, "file_not_found");
    }

    #[tokio::test]
    async fn local_rows_stay_on_disk_and_disabled_storage_is_503() {
        assert!(!super::super::local::is_s3_version(&version_row(
            None, "local"
        )));
        assert!(super::super::local::is_s3_version(&version_row(
            Some("k".to_string()),
            "s3"
        )));

        let (temp, mut state) = state_with_memory_store().await;
        state.set_storage(crate::storage::ArchiveStorage::local());
        let version = version_row(Some("archives/whatever.zip".to_string()), "s3");
        let error = serve(&state, &version, "archives/whatever.zip", &HeaderMap::new())
            .await
            .expect_err("local storage");
        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.code, "storage_unavailable");

        // The local fallback path resolves the file from the library root.
        let games_dir = state.core.config.games_dir.clone();
        std::fs::write(games_dir.join("Cat Quest II.zip"), b"local").expect("write");
        let root_id = crate::db::query_scalar::<i64>(
            "SELECT id FROM library_roots ORDER BY position, id LIMIT 1",
        )
        .fetch_one(&state.core.pool)
        .await
        .expect("root");
        let mut fallback = version_row(Some("archives/whatever.zip".to_string()), "s3");
        fallback.root_id = Some(root_id);
        assert!(can_fall_back_to_local(&error));
        assert_eq!(
            super::super::local::local_version_path(&state, &fallback)
                .await
                .expect("local fallback"),
            games_dir.join("Cat Quest II.zip")
        );
        let _ = temp;
    }
}
