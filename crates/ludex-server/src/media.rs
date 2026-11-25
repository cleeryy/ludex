//! Media cache: artwork downloads, URL helpers and safe file names.

use std::path::{Component, Path};
use std::time::Duration;

use anyhow::{Context, Result};
use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use tokio_util::io::ReaderStream;

use crate::url_guard::UrlGuard;

/// Maximum size of a cached artwork file (25 MiB).
const MAX_IMAGE_BYTES: usize = 25 * 1024 * 1024;
/// Redirects followed when downloading artwork.
const MAX_REDIRECTS: usize = 3;
/// Timeout for one artwork download.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// Extensions considered valid artwork files.
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "avif"];
/// Maximum header bytes inspected when sniffing cached image dimensions.
const HEADER_SNIFF_BYTES: usize = 65_536;

/// Reads `(width, height)` from a PNG, JPEG, or WebP byte slice without
/// decoding the full image. Returns `None` for unsupported or truncated input.
#[must_use]
pub fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    png_dimensions(bytes)
        .or_else(|| jpeg_dimensions(bytes))
        .or_else(|| webp_dimensions(bytes))
        .filter(|(w, h)| *w > 0 && *h > 0)
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Some((width, height))
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 1 < bytes.len() {
        while i < bytes.len() && bytes[i] != 0xFF {
            i += 1;
        }
        while i < bytes.len() && bytes[i] == 0xFF {
            i += 1;
        }
        if i >= bytes.len() {
            return None;
        }
        let marker = bytes[i];
        i += 1;
        if marker == 0xD9 || marker == 0xDA {
            return None;
        }
        if marker == 0x01 || (0xD0..=0xD8).contains(&marker) {
            continue;
        }
        if i + 1 >= bytes.len() {
            return None;
        }
        let length = usize::from(u16::from_be_bytes([bytes[i], bytes[i + 1]]));
        if length < 2 || i + length > bytes.len() {
            return None;
        }
        let is_sof = matches!(
            marker,
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF
        );
        if is_sof && length >= 7 {
            let height = u32::from(u16::from_be_bytes([bytes[i + 3], bytes[i + 4]]));
            let width = u32::from(u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]));
            return Some((width, height));
        }
        i += length;
    }
    None
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 25 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    match &bytes[12..16] {
        b"VP8X" if bytes.len() >= 30 => {
            let width = 1
                + (u32::from(bytes[24])
                    | (u32::from(bytes[25]) << 8)
                    | (u32::from(bytes[26]) << 16));
            let height = 1
                + (u32::from(bytes[27])
                    | (u32::from(bytes[28]) << 8)
                    | (u32::from(bytes[29]) << 16));
            Some((width, height))
        }
        b"VP8L" if bytes[20] == 0x2F => {
            let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
            let width = (bits & 0x3FFF) + 1;
            let height = ((bits >> 14) & 0x3FFF) + 1;
            Some((width, height))
        }
        b"VP8 " if bytes.len() >= 30 && bytes[23..26] == [0x9D, 0x01, 0x2A] => {
            let width = u32::from(u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3FFF);
            let height = u32::from(u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3FFF);
            Some((width, height))
        }
        _ => None,
    }
}

/// Reads the header of a cached media file and returns its `(width, height)`.
pub async fn cached_image_dimensions(media_dir: &Path, name: &str) -> Option<(u32, u32)> {
    use tokio::io::AsyncReadExt;
    if !safe_media_name(name) {
        return None;
    }
    let mut file = tokio::fs::File::open(media_dir.join(name)).await.ok()?;
    let mut buf = vec![0_u8; HEADER_SNIFF_BYTES];
    let mut filled = 0;
    while filled < buf.len() {
        let read = file.read(&mut buf[filled..]).await.ok()?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    image_dimensions(&buf[..filled])
}

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
///
/// The URL is provider- or library-supplied, so the fetch goes through
/// [`UrlGuard`]: http(s) only, public addresses only, redirects followed
/// manually with a policy check on every hop, DNS pinned to the validated
/// address, and a body cap. A rejected URL is an error; callers skip the image.
pub async fn download_media(media_dir: &Path, url: &str, allow_private: bool) -> Result<String> {
    let parsed = crate::url_guard::validate_url(url).context("invalid artwork URL")?;

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

    let guard =
        UrlGuard::new(allow_private).with_limits(MAX_REDIRECTS, MAX_IMAGE_BYTES, DOWNLOAD_TIMEOUT);
    let fetched = guard.get(url).await.context("download artwork")?;
    if !(200..300).contains(&fetched.status) {
        anyhow::bail!("artwork endpoint returned {}", fetched.status);
    }
    reject_non_image(fetched.content_type.as_deref())?;
    if fetched.body.is_empty() {
        anyhow::bail!("empty artwork response");
    }

    tokio::fs::create_dir_all(media_dir)
        .await
        .context("create media directory")?;
    let temporary = media_dir.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
    tokio::fs::write(&temporary, &fetched.body)
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

/// Rejects responses that are clearly not images. A missing header or a
/// generic binary type is accepted because several artwork CDNs omit it.
fn reject_non_image(content_type: Option<&str>) -> Result<()> {
    let Some(content_type) = content_type else {
        return Ok(());
    };
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let image = mime.starts_with("image/")
        || mime == "application/octet-stream"
        || mime == "binary/octet-stream";
    if mime.is_empty() || image {
        return Ok(());
    }
    anyhow::bail!("artwork response is not an image: {mime}")
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

    #[tokio::test]
    async fn private_artwork_urls_are_refused_without_network() {
        let temp = tempfile::tempdir().expect("tempdir");
        for url in [
            "http://169.254.169.254/latest/meta-data/",
            "http://127.0.0.1:8006/cover.jpg",
            "http://[::1]/cover.jpg",
            "file:///etc/passwd",
        ] {
            let error = download_media(temp.path(), url, false)
                .await
                .expect_err("artwork must be refused");
            let message = format!("{error:#}");
            assert!(
                message.contains("invalid artwork URL") || message.contains("private address"),
                "{url}: {message}"
            );
        }
        let stored = std::fs::read_dir(temp.path()).expect("read media dir");
        assert_eq!(stored.count(), 0, "nothing may be cached");
    }

    #[tokio::test]
    async fn loopback_artwork_is_cached_when_private_is_allowed() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buffer = [0_u8; 1024];
            let _ = socket.read(&mut buffer).await;
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: image/png\r\ncontent-length: 4\r\nconnection: close\r\n\r\nPNG!",
                )
                .await
                .expect("write");
            let _ = socket.shutdown().await;
        });
        let temp = tempfile::tempdir().expect("tempdir");
        let url = format!("http://{address}/cover.png");
        let name = download_media(temp.path(), &url, true)
            .await
            .expect("download from loopback");
        assert!(name.ends_with(".png"), "{name}");
        let bytes = std::fs::read(temp.path().join(&name)).expect("read cached artwork");
        assert_eq!(bytes, b"PNG!");
        server.await.expect("server task");
    }

    #[tokio::test]
    async fn non_image_responses_are_refused() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buffer = [0_u8; 1024];
            let _ = socket.read(&mut buffer).await;
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: 13\r\nconnection: close\r\n\r\n<html></html>",
                )
                .await
                .expect("write");
            let _ = socket.shutdown().await;
        });
        let temp = tempfile::tempdir().expect("tempdir");
        let url = format!("http://{address}/cover.jpg");
        let error = download_media(temp.path(), &url, true)
            .await
            .expect_err("html is not artwork");
        assert!(format!("{error:#}").contains("not an image"), "{error:#}");
        server.await.expect("server task");
    }

    #[test]
    fn image_dimensions_parses_png_jpeg_and_webp_headers() {
        let mut png = vec![0_u8; 24];
        png[0..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        png[16..20].copy_from_slice(&600_u32.to_be_bytes());
        png[20..24].copy_from_slice(&900_u32.to_be_bytes());
        assert_eq!(image_dimensions(&png), Some((600, 900)));

        // Minimal JPEG: SOI + APP0 (length 4) + SOF0 (height 215, width 460).
        let jpeg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00,
            0xD7, 0x01, 0xCC, 0x03, 0x01, 0x11, 0x00,
        ];
        assert_eq!(image_dimensions(&jpeg), Some((460, 215)));

        let mut webp = vec![0_u8; 30];
        webp[0..4].copy_from_slice(b"RIFF");
        webp[8..16].copy_from_slice(b"WEBPVP8X");
        // Width 600 -> 599 = 0x0257; Height 900 -> 899 = 0x0383.
        webp[24..27].copy_from_slice(&[0x57, 0x02, 0x00]);
        webp[27..30].copy_from_slice(&[0x83, 0x03, 0x00]);
        assert_eq!(image_dimensions(&webp), Some((600, 900)));

        assert_eq!(image_dimensions(b"PNG!"), None);
    }
}
