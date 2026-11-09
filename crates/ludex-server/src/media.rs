//! Media cache: artwork downloads, URL helpers and safe file names.

use std::path::{Component, Path};

use anyhow::{Context, Result};
use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use tokio_util::io::ReaderStream;

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

/// Downloads a remote image into the media directory and returns its file name.
///
/// Files are content-addressed by the source URL, so repeated enrichment passes
/// never download the same artwork twice.
pub async fn download_media(http: &reqwest::Client, media_dir: &Path, url: &str) -> Result<String> {
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
    let destination = media_dir.join(&name);
    if tokio::fs::try_exists(&destination).await.unwrap_or(false) {
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
    let bytes = response.bytes().await.context("read artwork body")?;
    if bytes.len() > MAX_IMAGE_BYTES {
        anyhow::bail!("artwork exceeds {MAX_IMAGE_BYTES} bytes");
    }
    if bytes.is_empty() {
        anyhow::bail!("empty artwork response");
    }

    tokio::fs::create_dir_all(media_dir)
        .await
        .context("create media directory")?;
    let temporary = media_dir.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
    tokio::fs::write(&temporary, &bytes)
        .await
        .context("write artwork")?;
    match tokio::fs::rename(&temporary, &destination).await {
        Ok(()) => {}
        Err(error) => {
            let _ = tokio::fs::remove_file(&temporary).await;
            if !tokio::fs::try_exists(&destination).await.unwrap_or(false) {
                return Err(error).context("store artwork");
            }
        }
    }
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
pub async fn serve_media(media_dir: &Path, name: &str) -> Result<Response, StatusCode> {
    if !safe_media_name(name) {
        return Err(StatusCode::NOT_FOUND);
    }
    let path = media_dir.join(name);
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if !metadata.is_file() {
        return Err(StatusCode::NOT_FOUND);
    }
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let content_type = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .to_string();
    let body = Body::from_stream(ReaderStream::new(file));

    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_str(&content_type)
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
        )
        .header(header::CONTENT_LENGTH, metadata.len())
        .header(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        )
        .body(body)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Converts a streamed media response into an axum response.
pub async fn media_response(media_dir: &Path, name: &str) -> Response {
    match serve_media(media_dir, name).await {
        Ok(response) => response,
        Err(status) => status.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
