//! Custom artwork uploads (#38).
//!
//! Editors can replace a bad or missing cover/background with an image they
//! own. The server validates the magic bytes (never trusts the client MIME),
//! writes the file under `{media_dir}/custom/` and points the game row at it.
//! Files are content-addressed by timestamp and game id so a new upload is
//! immediately visible without disabling HTTP caching.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Maximum accepted artwork size (8 MiB).
pub const MAX_ARTWORK_BYTES: usize = 8 * 1024 * 1024;

/// Which artwork slot an upload targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtworkKind {
    /// Front cover shown on cards and the game page.
    Cover,
    /// Wide hero/background image.
    Background,
}

impl ArtworkKind {
    /// Parses the multipart `kind` field.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cover" => Some(Self::Cover),
            "background" | "hero" => Some(Self::Background),
            _ => None,
        }
    }

    /// Stable lowercase label used in JSON, file names and audit metadata.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cover => "cover",
            Self::Background => "background",
        }
    }

    /// Database column updated by this kind.
    #[must_use]
    pub const fn column(self) -> &'static str {
        match self {
            Self::Cover => "cover_path",
            Self::Background => "background_path",
        }
    }
}

/// Detected image format from the first bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// PNG (`\x89PNG\r\n\x1a\n`).
    Png,
    /// JPEG (`\xFF\xD8\xFF`).
    Jpeg,
    /// WebP (`RIFF....WEBP`).
    WebP,
    /// Animated GIF.
    Gif,
}

impl ImageFormat {
    /// File extension used on disk.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::WebP => "webp",
            Self::Gif => "gif",
        }
    }
}

/// Sniffs the image format from its magic bytes.
///
/// SVG is deliberately rejected: it is XML and can carry scripts when served
/// with the wrong content type.
#[must_use]
pub fn detect_image(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(ImageFormat::Png);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(ImageFormat::Jpeg);
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(ImageFormat::WebP);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(ImageFormat::Gif);
    }
    None
}

/// Empty or whitespace bytes are not an image even if padded.
#[must_use]
pub fn is_acceptable(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.len() <= MAX_ARTWORK_BYTES && detect_image(bytes).is_some()
}

/// Result of storing one upload.
#[derive(Debug, Clone)]
pub struct StoredArtwork {
    /// File name inside the media cache (served by `/api/media/{name}`).
    pub name: String,
    /// Detected format label.
    pub format: &'static str,
    /// Number of bytes written.
    pub size: usize,
    /// SHA-256 of the file, for provenance without keeping the upload.
    pub sha256: String,
}

/// Writes the image into the media directory and returns its media name.
///
/// The write is atomic (temporary file + rename). The file name contains the
/// game id, kind, a timestamp and a short content hash, so two uploads never
/// collide and clients can cache aggressively.
pub async fn store(
    media_dir: &Path,
    game_id: i64,
    kind: ArtworkKind,
    bytes: &[u8],
) -> std::io::Result<StoredArtwork> {
    let format = detect_image(bytes).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "unsupported image format (PNG, JPEG, WebP or GIF required)",
        )
    })?;
    tokio::fs::create_dir_all(media_dir).await?;

    let hash = hex::encode(Sha256::digest(bytes));
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%6f");
    // One path component: the media route only serves plain file names.
    let name = format!(
        "custom-{game_id}-{}-{stamp}-{}.{}",
        kind.as_str(),
        &hash[..12],
        format.extension()
    );
    let final_path: PathBuf = media_dir.join(&name);
    let temp_path = media_dir.join(format!(".upload-{}.part", uuid::Uuid::new_v4()));

    tokio::fs::write(&temp_path, bytes).await?;
    if let Err(error) = tokio::fs::rename(&temp_path, &final_path).await {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(error);
    }

    Ok(StoredArtwork {
        name,
        format: format.extension(),
        size: bytes.len(),
        sha256: hash,
    })
}

/// Removes a previously stored custom upload, ignoring "not found".
///
/// Only files named `custom-*` are removable: provider art is owned by the
/// metadata refresh and must never be deleted through this path.
pub async fn remove(media_dir: &Path, name: &str) {
    if !name.starts_with("custom-") {
        return;
    }
    let path = media_dir.join(name);
    // Defense in depth: the joined path must stay inside the media directory.
    let Ok(media_root) = media_dir.canonicalize() else {
        return;
    };
    let Ok(candidate) = path.canonicalize() else {
        return;
    };
    if candidate.starts_with(&media_root) {
        let _ = tokio::fs::remove_file(candidate).await;
    }
}
