//! Account avatar uploads (#183).
//!
//! The server never trusts the client MIME type: PNG, JPEG and WebP are
//! recognised by their magic bytes, the pixel dimensions are read from the
//! image header, and only then is the file written into the media directory.
//! SVG and HTML are rejected because they can carry scripts when a browser
//! opens the media URL directly.
//!
//! Names are content-addressed (`avatar-<user-id>-<sha256>.<ext>`) so a new
//! upload never reuses the previous URL and clients can cache aggressively.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Largest avatar accepted by the upload route (2 MiB).
pub const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;
/// Largest width or height accepted from the image header.
pub const MAX_AVATAR_DIMENSION: u32 = 4096;

/// Image formats accepted for an avatar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvatarFormat {
    /// PNG.
    Png,
    /// JPEG.
    Jpeg,
    /// WebP.
    WebP,
}

impl AvatarFormat {
    /// Extension used on disk and in the media URL.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::WebP => "webp",
        }
    }

    /// Stable label used in audit metadata.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::WebP => "webp",
        }
    }

    /// Whether a client file extension is consistent with this format.
    #[must_use]
    fn accepts_extension(self, extension: &str) -> bool {
        match self {
            Self::Png => extension == "png",
            Self::Jpeg => matches!(extension, "jpg" | "jpeg"),
            Self::WebP => extension == "webp",
        }
    }
}

/// Why an upload was rejected before anything was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvatarRejection {
    /// The multipart file field was missing or empty.
    Empty,
    /// The payload exceeds [`MAX_AVATAR_BYTES`].
    TooLarge,
    /// The magic bytes are not PNG, JPEG or WebP (SVG and HTML land here).
    Unsupported,
    /// The client file name says one format, the bytes say another.
    ExtensionMismatch,
    /// The header is unreadable or the image exceeds the pixel cap.
    Dimensions,
}

impl AvatarRejection {
    /// Stable API error code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Empty => "invalid_avatar",
            Self::TooLarge => "avatar_too_large",
            Self::Unsupported | Self::ExtensionMismatch => "invalid_avatar_type",
            Self::Dimensions => "invalid_avatar_dimensions",
        }
    }

    /// Human-readable message safe to show to the account holder.
    #[must_use]
    pub fn message(self) -> String {
        match self {
            Self::Empty => "Choose an image file to upload".to_string(),
            Self::TooLarge => format!(
                "Avatars must be at most {} MiB",
                MAX_AVATAR_BYTES / (1024 * 1024)
            ),
            Self::Unsupported => {
                "Unsupported image. Use a PNG, JPEG or WebP file; SVG is not allowed".to_string()
            }
            Self::ExtensionMismatch => {
                "The file extension does not match the image content".to_string()
            }
            Self::Dimensions => format!(
                "The image header is unreadable or larger than {MAX_AVATAR_DIMENSION}×{MAX_AVATAR_DIMENSION}"
            ),
        }
    }
}

/// Validated avatar ready to be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvatarImage {
    /// Sniffed format.
    pub format: AvatarFormat,
    /// Width in pixels from the header.
    pub width: u32,
    /// Height in pixels from the header.
    pub height: u32,
    /// SHA-256 of the uploaded bytes.
    pub sha256: String,
    /// Number of bytes uploaded.
    pub size: usize,
}

/// Result of storing one avatar upload.
#[derive(Debug, Clone)]
pub struct StoredAvatar {
    /// Media file name, served by `/api/media/{name}`.
    pub name: String,
    /// Sniffed format.
    pub format: AvatarFormat,
    /// Number of bytes written.
    pub size: usize,
    /// SHA-256 of the file.
    pub sha256: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Sniffs PNG, JPEG or WebP from the leading bytes.
#[must_use]
pub fn detect_format(bytes: &[u8]) -> Option<AvatarFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(AvatarFormat::Png);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(AvatarFormat::Jpeg);
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(AvatarFormat::WebP);
    }
    None
}

/// Validates one upload: size, magic bytes, extension and dimensions.
///
/// The file name is only inspected, never trusted: an empty or missing
/// extension is accepted, a known extension must match the sniffed format.
pub fn inspect(bytes: &[u8], file_name: &str) -> Result<AvatarImage, AvatarRejection> {
    if bytes.is_empty() {
        return Err(AvatarRejection::Empty);
    }
    if bytes.len() > MAX_AVATAR_BYTES {
        return Err(AvatarRejection::TooLarge);
    }
    let format = detect_format(bytes).ok_or(AvatarRejection::Unsupported)?;
    if let Some((_, extension)) = file_name.rsplit_once('.') {
        let extension = extension.trim().to_ascii_lowercase();
        if !extension.is_empty() && !format.accepts_extension(&extension) {
            return Err(AvatarRejection::ExtensionMismatch);
        }
    }
    let (width, height) = dimensions(bytes, format).ok_or(AvatarRejection::Dimensions)?;
    if width == 0 || height == 0 || width > MAX_AVATAR_DIMENSION || height > MAX_AVATAR_DIMENSION {
        return Err(AvatarRejection::Dimensions);
    }
    Ok(AvatarImage {
        format,
        width,
        height,
        sha256: hex::encode(Sha256::digest(bytes)),
        size: bytes.len(),
    })
}

/// Reads the pixel dimensions from the PNG, JPEG or WebP header.
#[must_use]
pub fn dimensions(bytes: &[u8], format: AvatarFormat) -> Option<(u32, u32)> {
    match format {
        AvatarFormat::Png => png_dimensions(bytes),
        AvatarFormat::Jpeg => jpeg_dimensions(bytes),
        AvatarFormat::WebP => webp_dimensions(bytes),
    }
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    // Signature (8) + chunk length (4) + `IHDR` (4) + width/height (8).
    if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut index = 2;
    while index + 4 <= bytes.len() {
        if bytes[index] != 0xFF {
            return None;
        }
        let mut marker = bytes[index + 1];
        let mut cursor = index + 2;
        // Any number of 0xFF fill bytes may precede a marker.
        while marker == 0xFF && cursor < bytes.len() {
            marker = bytes[cursor];
            cursor += 1;
        }
        // Standalone markers carry no segment length.
        if marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            index = cursor;
            continue;
        }
        // Start of scan: no frame header was found before the entropy data.
        if marker == 0xDA || cursor + 2 > bytes.len() {
            return None;
        }
        let length = usize::from(u16::from_be_bytes([bytes[cursor], bytes[cursor + 1]]));
        if length < 2 || cursor + length > bytes.len() {
            return None;
        }
        // SOF0..SOF15 except DHT (C4), JPG (C8) and DAC (CC).
        if matches!(
            marker,
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF
        ) {
            if length < 7 {
                return None;
            }
            let height = u32::from(u16::from_be_bytes([bytes[cursor + 3], bytes[cursor + 4]]));
            let width = u32::from(u16::from_be_bytes([bytes[cursor + 5], bytes[cursor + 6]]));
            return Some((width, height));
        }
        index = cursor + length;
    }
    None
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 30 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    match &bytes[12..16] {
        // Lossy key frame: 3-byte frame tag, start code, 14-bit dimensions.
        b"VP8 " => {
            if &bytes[23..26] != b"\x9d\x01\x2a" {
                return None;
            }
            let width = u32::from(u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3FFF);
            let height = u32::from(u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3FFF);
            Some((width, height))
        }
        // Lossless: 0x2F signature then 14+14 bits of width/height minus one.
        b"VP8L" => {
            if bytes[20] != 0x2F {
                return None;
            }
            let packed = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
            let width = (packed & 0x3FFF) + 1;
            let height = ((packed >> 14) & 0x3FFF) + 1;
            Some((width, height))
        }
        // Extended format: canvas size is stored as 24-bit little-endian - 1.
        b"VP8X" => {
            let width = 1 + u32::from_le_bytes([bytes[24], bytes[25], bytes[26], 0]);
            let height = 1 + u32::from_le_bytes([bytes[27], bytes[28], bytes[29], 0]);
            Some((width, height))
        }
        _ => None,
    }
}

/// Writes a validated avatar into the media directory.
///
/// The write is atomic and the name contains the user id and the content hash,
/// so a replacement never reuses the previous URL.
pub async fn store(
    media_dir: &Path,
    user_id: i64,
    image: &AvatarImage,
    bytes: &[u8],
) -> std::io::Result<StoredAvatar> {
    tokio::fs::create_dir_all(media_dir).await?;
    let name = format!(
        "avatar-{user_id}-{}.{}",
        image.sha256,
        image.format.extension()
    );
    let destination: PathBuf = media_dir.join(&name);
    if tokio::fs::try_exists(&destination).await.unwrap_or(false) {
        return Ok(StoredAvatar {
            name,
            format: image.format,
            size: image.size,
            sha256: image.sha256.clone(),
            width: image.width,
            height: image.height,
        });
    }

    let temporary = media_dir.join(format!(".avatar-{}.part", uuid::Uuid::new_v4()));
    tokio::fs::write(&temporary, bytes).await?;
    if let Err(error) = tokio::fs::rename(&temporary, &destination).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }
    Ok(StoredAvatar {
        name,
        format: image.format,
        size: image.size,
        sha256: image.sha256.clone(),
        width: image.width,
        height: image.height,
    })
}

/// Whether `name` is an avatar managed for `user_id`.
#[must_use]
pub fn is_managed_name(name: &str, user_id: i64) -> bool {
    if !crate::media::safe_media_name(name) {
        return false;
    }
    name.strip_prefix("avatar-")
        .and_then(|rest| rest.split_once('-'))
        .is_some_and(|(owner, _)| owner == user_id.to_string())
}

/// Removes a managed avatar, ignoring missing files and anything else.
///
/// Artwork and provider images are never touched: the name must belong to this
/// account and follow the `avatar-<user-id>-<sha256>.<ext>` pattern.
pub async fn remove(media_dir: &Path, user_id: i64, name: &str) {
    if !is_managed_name(name, user_id) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes
    }

    fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8];
        // APP0 with a payload, then SOF0 with the dimensions.
        bytes.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00]);
        bytes.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&[0x03, 0x01, 0x11, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01]);
        bytes
    }

    fn webp_lossy(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = b"RIFF\x00\x00\x00\x00WEBPVP8 ".to_vec();
        // Chunk size, 3-byte frame tag, start code, 14-bit dimensions.
        bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0x9d, 0x01, 0x2a]);
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes
    }

    #[test]
    fn detects_supported_formats_and_rejects_markup() {
        assert_eq!(detect_format(&png(16, 16)), Some(AvatarFormat::Png));
        assert_eq!(detect_format(&jpeg(16, 16)), Some(AvatarFormat::Jpeg));
        assert_eq!(detect_format(&webp_lossy(16, 16)), Some(AvatarFormat::WebP));
        assert_eq!(
            detect_format(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            None
        );
        assert_eq!(detect_format(b"<!doctype html><html></html>"), None);
        assert_eq!(detect_format(b"GIF89a"), None);
    }

    #[test]
    fn reads_dimensions_from_each_header() {
        assert_eq!(png_dimensions(&png(1920, 1080)), Some((1920, 1080)));
        assert_eq!(jpeg_dimensions(&jpeg(640, 480)), Some((640, 480)));
        assert_eq!(webp_dimensions(&webp_lossy(320, 200)), Some((320, 200)));
    }

    #[test]
    fn inspect_enforces_size_dimensions_and_extension() {
        let valid = png(256, 256);
        let image = inspect(&valid, "logo.png").expect("valid png");
        assert_eq!(image.format, AvatarFormat::Png);
        assert_eq!((image.width, image.height), (256, 256));
        assert_eq!(image.sha256.len(), 64);

        assert_eq!(
            inspect(&valid, "logo.svg"),
            Err(AvatarRejection::ExtensionMismatch)
        );
        assert_eq!(
            inspect(&png(5000, 100), "logo.png"),
            Err(AvatarRejection::Dimensions)
        );
        assert_eq!(
            inspect(&png(100, 5000), "logo.png"),
            Err(AvatarRejection::Dimensions)
        );
        assert_eq!(
            inspect(&vec![0_u8; MAX_AVATAR_BYTES + 1], "logo.png"),
            Err(AvatarRejection::TooLarge)
        );
        assert_eq!(
            inspect(b"<html><script>alert(1)</script></html>", "logo.png"),
            Err(AvatarRejection::Unsupported)
        );
        assert_eq!(inspect(&[], "logo.png"), Err(AvatarRejection::Empty));
        assert_eq!(
            inspect(&valid, "photo.jpeg"),
            Err(AvatarRejection::ExtensionMismatch)
        );
        assert_eq!(
            inspect(&jpeg(32, 32), "photo.jpeg").map(|i| i.format),
            Ok(AvatarFormat::Jpeg)
        );
    }

    #[test]
    fn managed_names_are_scoped_to_one_account() {
        assert!(is_managed_name("avatar-7-abc.png", 7));
        assert!(!is_managed_name("avatar-70-abc.png", 7));
        assert!(!is_managed_name("avatar-7-../../secret.png", 7));
        assert!(!is_managed_name("custom-7-cover.png", 7));
        assert!(!is_managed_name("avatar.png", 7));
    }
}
