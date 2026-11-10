//! Media cache: artwork downloads, URL helpers and safe file names.

use std::path::{Component, Path};

use anyhow::{Context, Result};
use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use sha2::{Digest, Sha256};

use crate::state::AppState;
use crate::storage::{MediaBody, StorageBackend};

/// Maximum size of a cached artwork file (25 MiB).
const MAX_IMAGE_BYTES: usize = 25 * 1024 * 1024;
/// Extensions considered valid artwork files.
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "avif"];

/// Public URL for a cached media file.
#[must_use]
pub fn media_url(name: &str) -> String {
    format!("/api/media/{name}")
}

/// Rejects anything that is not a plain file name inside the media directory.
#[must_use]
pub fn safe_media_name(name: &str) -> bool {
    if name.is_empty() || name.starts_with('.') || name.contains('\\') || name.contains('\0') {
        return false;
    }
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

/// Downloads a remote image into the media backend and returns its file name.
///
/// Files are content-addressed by the source URL, so repeated enrichment passes
/// never download the same artwork twice.
pub async fn download_media(
    http: &reqwest::Client,
    storage: &dyn StorageBackend,
    url: &str,
) -> Result<String> {
    let parsed = url::Url::parse(url).with_context(|| format!("invalid image URL: {url}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        anyhow::bail!("unsupported image scheme: {}", parsed.scheme());
    }

    let extension = parsed
        .path()
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .filter(|extension| IMAGE_EXTENSIONS.contains(&extension.as_str()))
        .unwrap_or_else(|| "jpg".to_string());
    let digest = Sha256::digest(url.as_bytes());
    let name = format!("{}.{}", hex::encode(digest), extension);
    if storage.exists_media(&name).await.unwrap_or(false) {
        return Ok(name);
    }

    let response = http.get(parsed).send().await.context("download artwork")?;
    if !response.status().is_success() {
        anyhow::bail!("artwork endpoint returned {}", response.status());
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_IMAGE_BYTES as u64)
    {
        anyhow::bail!("artwork exceeds {MAX_IMAGE_BYTES} bytes");
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.bytes().await.context("read artwork body")?;
    if bytes.len() > MAX_IMAGE_BYTES {
        anyhow::bail!("artwork exceeds {MAX_IMAGE_BYTES} bytes");
    }
    if bytes.is_empty() {
        anyhow::bail!("empty artwork response");
    }
    let content_type = content_type.unwrap_or_else(|| {
        mime_guess::from_path(&name)
            .first_or_octet_stream()
            .to_string()
    });
    storage
        .put_media(&name, bytes, &content_type)
        .await
        .context("store artwork")?;
    Ok(name)
}

/// Builds a `Content-Disposition` attachment header with an ASCII fallback and
/// an RFC 5987 UTF-8 file name.
#[must_use]
pub fn content_disposition(file_name: &str) -> String {
    let fallback: String = file_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ' | '(' | ')') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let fallback = if fallback.trim().is_empty() {
        "download".to_string()
    } else {
        fallback
    };
    format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        fallback.replace(['"', '\\'], "_"),
        percent_encode(file_name)
    )
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(*byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Streams a cached media file with immutable cache headers.
pub async fn serve_media(storage: &dyn StorageBackend, name: &str) -> Result<Response, StatusCode> {
    if !safe_media_name(name) {
        return Err(StatusCode::NOT_FOUND);
    }
    let stored = storage
        .read_media(name)
        .await
        .map_err(|error| {
            tracing::warn!(file = %name, error = %error, "could not read cached artwork");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    let body = match stored.body {
        MediaBody::Bytes(bytes) => Body::from(bytes),
        MediaBody::Stream(stream) => Body::from_stream(stream),
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_str(&stored.content_type)
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
        )
        .header(header::CONTENT_LENGTH, stored.len)
        .header(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        )
        .body(body)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Serves a media file, redirecting to the public object URL when the S3
/// backend is configured with `LUDEX_S3_PUBLIC_BASE_URL`.
pub async fn media_response(state: &AppState, name: &str) -> Response {
    if !safe_media_name(name) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Some(url) = state.storage.media_public_url(name) {
        let mut response = Redirect::temporary(&url).into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=300"),
        );
        return response;
    }
    match serve_media(state.storage.as_ref(), name).await {
        Ok(response) => response,
        Err(status) => status.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::ObjectStorage;
    use axum::http::StatusCode;
    use std::sync::Arc;

    #[tokio::test]
    async fn media_is_streamed_from_object_storage() {
        let storage = ObjectStorage::with_store(
            Arc::new(object_store::memory::InMemory::new()),
            "ludex-test",
            "ludex",
            None,
        );
        storage
            .put_media(
                "abc.jpg",
                bytes::Bytes::from_static(b"object-bytes"),
                "image/jpeg",
            )
            .await
            .unwrap();

        let response = serve_media(&storage, "abc.jpg").await.expect("serve");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("image/jpeg")
        );
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok()),
            Some("12")
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"object-bytes");

        assert_eq!(
            serve_media(&storage, "missing.jpg").await.unwrap_err(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            serve_media(&storage, "../escape.jpg").await.unwrap_err(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn media_names_are_single_components() {
        assert!(safe_media_name("abc123.jpg"));
        assert!(safe_media_name("a-b_c.webp"));
        assert!(!safe_media_name("../secret"));
        assert!(!safe_media_name("dir/file.jpg"));
        assert!(!safe_media_name("dir\\file.jpg"));
        assert!(!safe_media_name(""));
        assert!(!safe_media_name(".tmp-file"));
    }

    #[test]
    fn content_disposition_has_fallback_and_utf8() {
        let header = content_disposition("Jeu (2020).zip");
        assert!(header.starts_with("attachment; filename=\"Jeu (2020).zip\""));
        assert!(header.contains("filename*=UTF-8''Jeu%20%282020%29.zip"));
        let header = content_disposition("\"evil\"\n.zip");
        assert!(header.contains("filename=\"_evil__.zip\""));
    }
}
