//! Cheap, stable fingerprints for library archives.
//!
//! Update detection needs to know whether the archive behind a game version was
//! replaced, without hashing tens of gigabytes on every scan. The fingerprint is
//! a SHA-256 over the file size plus a bounded sample of the head and the tail:
//! it is stable for a given file, changes whenever the archive is replaced, and
//! costs at most two read windows regardless of the archive size.
//!
//! The value is prefixed with the algorithm name so a future, stronger
//! fingerprint can coexist with stored values:
//!
//! ```text
//! sha256-sample:<64 hex chars>
//! ```
//!
//! It is **not** a full content hash and must not be used to prove integrity;
//! [`crate::archive::verify_archive`] and the per-file install manifest do that.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use sha2::{Digest, Sha256};

/// Bytes read from the head and the tail of the file.
pub const SAMPLE_BYTES: u64 = 1024 * 1024;

/// Algorithm prefix stored with every fingerprint.
pub const FINGERPRINT_PREFIX: &str = "sha256-sample:";

/// Fingerprint an archive file.
///
/// The digest covers the size (8 bytes, little endian), the first
/// [`SAMPLE_BYTES`] bytes and the last [`SAMPLE_BYTES`] bytes. Files smaller
/// than two sample windows are read once, without overlapping ranges.
pub fn archive_fingerprint(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();

    let mut hasher = Sha256::new();
    hasher.update(size.to_le_bytes());

    let head_len = size.min(SAMPLE_BYTES);
    if head_len > 0 {
        let mut buffer = vec![0u8; head_len as usize];
        file.read_exact(&mut buffer)?;
        hasher.update(&buffer);
    }

    if size > SAMPLE_BYTES {
        let tail_start = size.saturating_sub(SAMPLE_BYTES);
        if tail_start > head_len {
            file.seek(SeekFrom::Start(tail_start))?;
            let mut buffer = vec![0u8; SAMPLE_BYTES as usize];
            file.read_exact(&mut buffer)?;
            hasher.update(&buffer);
        }
    }

    Ok(format!(
        "{FINGERPRINT_PREFIX}{}",
        hex::encode(hasher.finalize())
    ))
}

/// True when `value` looks like a fingerprint produced by this module.
#[must_use]
pub fn is_fingerprint(value: &str) -> bool {
    value
        .strip_prefix(FINGERPRINT_PREFIX)
        .is_some_and(|digest| digest.len() == 64 && digest.chars().all(|c| c.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ludex-fingerprint-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut file = File::create(&path).unwrap();
        file.write_all(bytes).unwrap();
        path
    }

    #[test]
    fn fingerprint_is_stable_and_prefixed() {
        let path = write_file("a.zip", b"hello world");
        let first = archive_fingerprint(&path).unwrap();
        let second = archive_fingerprint(&path).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with(FINGERPRINT_PREFIX));
        assert!(is_fingerprint(&first));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn fingerprint_changes_when_content_or_size_changes() {
        let first = write_file("a.zip", b"hello world");
        let same_size = write_file("b.zip", b"hello worle");
        let longer = write_file("c.zip", b"hello world!");
        let a = archive_fingerprint(&first).unwrap();
        let b = archive_fingerprint(&same_size).unwrap();
        let c = archive_fingerprint(&longer).unwrap();
        assert_ne!(a, b, "same-size edits must change the fingerprint");
        assert_ne!(a, c, "size changes must change the fingerprint");
        for path in [first, same_size, longer] {
            let _ = std::fs::remove_dir_all(path.parent().unwrap());
        }
    }

    #[test]
    fn fingerprint_samples_large_files_without_reading_everything() {
        // 3 MiB: head + tail windows with a gap in the middle.
        let mut bytes = vec![0u8; (SAMPLE_BYTES * 3) as usize];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = (index % 251) as u8;
        }
        let path = write_file("large.zip", &bytes);
        let first = archive_fingerprint(&path).unwrap();

        // Change a byte in the middle, outside both sample windows.
        bytes[(SAMPLE_BYTES + SAMPLE_BYTES / 2) as usize] ^= 0xFF;
        std::fs::write(&path, &bytes).unwrap();
        let second = archive_fingerprint(&path).unwrap();
        assert_eq!(first, second, "the middle of the file is not sampled");

        // Change a byte in the tail window.
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        std::fs::write(&path, &bytes).unwrap();
        let third = archive_fingerprint(&path).unwrap();
        assert_ne!(second, third);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn rejects_malformed_fingerprints() {
        assert!(!is_fingerprint(""));
        assert!(!is_fingerprint("sha256-sample:abc"));
        assert!(!is_fingerprint("md5:0000"));
        assert!(is_fingerprint(&format!(
            "{FINGERPRINT_PREFIX}{}",
            "a".repeat(64)
        )));
    }
}
