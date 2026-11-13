//! Full archive SHA-256 verification before install (#216).
//!
//! The server publishes the SHA-256 of every byte of an archive: as
//! `contentSha256` in the version payload and as the `X-Ludex-Archive-Sha256`
//! header on the download response. This module turns those values into a
//! yes/no gate that runs before extraction, so a truncated or corrupted body
//! can never be installed as a successful game.
//!
//! ## Hashing a resumed transfer
//!
//! The classic transport writes a `.part` file and resumes with HTTP `Range`.
//! A streaming hasher cannot be persisted across a process restart, so the rule
//! is: a digest produced while writing is only trusted when the file was
//! written **once from byte 0** in the same run. A resumed (or restarted)
//! transfer is rehashed from byte 0 after the last byte arrives. That is one
//! extra sequential read of the archive, which is why the streaming path covers
//! the common case; the fallback keeps the guarantee without a partial-hash
//! state file.

use std::path::Path;

use crate::manifest::hash_file;

/// Response header carrying the server's full archive SHA-256 (lowercase hex).
pub const ARCHIVE_SHA256_HEADER: &str = "x-ludex-archive-sha256";

/// Outcome of verifying one downloaded archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArchiveVerdict {
    /// The received bytes match the digest the server published.
    Verified(String),
    /// The server has no full hash for this version (legacy row or a job that
    /// has not run yet): warn, do not block.
    Unverified,
    /// The bytes do not match the published digest.
    Mismatch(ArchiveMismatch),
}

/// A failed verification, with the information needed to explain it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveMismatch {
    /// Digest published by the server.
    pub expected: String,
    /// Digest of the bytes on disk.
    pub actual: String,
    /// The server advertised `actual` for the bytes it just sent, but the
    /// version metadata said `expected`: the archive was replaced between the
    /// metadata read and the transfer, not corrupted in flight.
    pub changed_on_server: bool,
}

impl ArchiveMismatch {
    /// User-facing failure message. Both digests are included: they are not
    /// secrets and are what a support report needs.
    pub(crate) fn message(&self, title: &str) -> String {
        if self.changed_on_server {
            format!(
                "The archive for \"{title}\" changed on the server after the download started. \
                 Expected SHA-256 {}, received {}. Nothing was installed; install again to fetch the new archive.",
                self.expected, self.actual
            )
        } else {
            format!(
                "The downloaded archive for \"{title}\" is corrupted: expected SHA-256 {}, \
                 received {}. Nothing was installed; the partial file was deleted.",
                self.expected, self.actual
            )
        }
    }
}

/// Verifies one archive before extraction.
///
/// `known` is a digest computed while writing the file from byte 0 (skips the
/// extra read). `served` is the digest the server put on the response: when it
/// matches the received bytes but not `expected`, the archive changed on the
/// server rather than being corrupted in transit.
pub(crate) fn verify_download(
    path: &Path,
    expected: Option<&str>,
    known: Option<&str>,
    served: Option<&str>,
) -> Result<ArchiveVerdict, String> {
    let Some(expected) = expected.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(ArchiveVerdict::Unverified);
    };
    let actual = match known.map(str::trim).filter(|value| !value.is_empty()) {
        Some(digest) => digest.to_string(),
        // A resumed transfer has no trusted streaming digest: hash the whole
        // file from byte 0, never just the new bytes.
        None => hash_file(path)?.1,
    };
    if actual.eq_ignore_ascii_case(expected) {
        return Ok(ArchiveVerdict::Verified(actual.to_ascii_lowercase()));
    }
    let changed_on_server = served.is_some_and(|served| {
        served.eq_ignore_ascii_case(&actual) && !served.eq_ignore_ascii_case(expected)
    });
    Ok(ArchiveVerdict::Mismatch(ArchiveMismatch {
        expected: expected.to_ascii_lowercase(),
        actual: actual.to_ascii_lowercase(),
        changed_on_server,
    }))
}

/// Message logged when a version has no full hash yet.
pub(crate) fn legacy_warning(game_id: i64, version_id: i64) -> String {
    format!(
        "VERSION {version_id} of game {game_id} has no full archive hash yet; \
         installing without strong verification (legacy row or pending server backfill)"
    )
}

/// Message logged when a stream install cannot verify the archive.
pub(crate) fn stream_note(expected: Option<&str>) -> String {
    match expected {
        Some(_) => "per-entry stream install: entries are CRC-32 and size verified, the full archive hash is not applicable because no archive is retained".to_string(),
        None => "per-entry stream install on a legacy version without a full archive hash; entries are CRC-32 and size verified".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludex-verify-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("tempdir");
        dir
    }

    fn temp_file(label: &str, bytes: &[u8]) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = temp_dir(label);
        let path = dir.join("archive.part");
        std::fs::write(&path, bytes).expect("write");
        (dir, path)
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    #[test]
    fn a_corrupt_archive_is_rejected_with_both_digests() {
        let payload: Vec<u8> = (0..256 * 1024).map(|index| (index % 251) as u8).collect();
        let expected = sha256_hex(&payload);
        let mut corrupt = payload.clone();
        corrupt[128 * 1024] ^= 0xFF;
        let (dir, path) = temp_file("corrupt", &corrupt);

        let verdict = verify_download(&path, Some(&expected), None, None).expect("verify");
        let ArchiveVerdict::Mismatch(mismatch) = verdict else {
            panic!("a corrupted archive must be rejected: {verdict:?}");
        };
        assert!(!mismatch.changed_on_server);
        let message = mismatch.message("Hades");
        assert!(message.contains(&expected), "{message}");
        assert!(message.contains(&mismatch.actual), "{message}");
        assert!(message.contains("corrupted"), "{message}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_truncated_archive_is_rejected() {
        let payload: Vec<u8> = vec![7_u8; 200 * 1024];
        let expected = sha256_hex(&payload);
        let (dir, path) = temp_file("truncated", &payload[..100 * 1024]);

        let verdict = verify_download(&path, Some(&expected), None, None).expect("verify");
        assert!(
            matches!(verdict, ArchiveVerdict::Mismatch(_)),
            "{verdict:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_resumed_file_is_hashed_from_byte_zero() {
        // Simulate the resume writer: first attempt wrote the first half, a
        // new process appended the second half. No streaming digest survives,
        // so verification must read the whole file, not only the new bytes.
        let payload: Vec<u8> = (0..300 * 1024).map(|index| (index % 243) as u8).collect();
        let split = payload.len() / 2;
        let dir = temp_dir("resume");
        let path = dir.join("archive.part");
        std::fs::write(&path, &payload[..split]).expect("partial write");
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("append");
            file.write_all(&payload[split..]).expect("resume write");
        }

        let verdict = verify_download(&path, Some(&sha256_hex(&payload)), None, None)
            .expect("verify resumed file");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            verdict,
            ArchiveVerdict::Verified(sha256_hex(&payload)),
            "a file assembled by a resume must verify against the whole archive"
        );
    }

    #[test]
    fn a_changed_archive_is_distinguished_from_transport_corruption() {
        let new_bytes = vec![2_u8; 64 * 1024];
        let old_bytes = vec![1_u8; 64 * 1024];
        let (dir, path) = temp_file("changed", &new_bytes);
        let served = sha256_hex(&new_bytes);

        // The server sent a different archive than the metadata described.
        let verdict = verify_download(&path, Some(&sha256_hex(&old_bytes)), None, Some(&served))
            .expect("verify");
        let ArchiveVerdict::Mismatch(mismatch) = verdict else {
            panic!("expected a mismatch");
        };
        assert!(mismatch.changed_on_server);
        assert!(mismatch.message("Hades").contains("changed on the server"));

        // Same metadata, corrupted body: the server's own digest disagrees
        // with what reached the disk, which is transport corruption.
        let mut flipped = new_bytes.clone();
        flipped[0] ^= 0xFF;
        let (dir2, corrupt_path) = temp_file("flipped", &flipped);
        let verdict =
            verify_download(&corrupt_path, Some(&served), None, Some(&served)).expect("verify");
        let ArchiveVerdict::Mismatch(mismatch) = verdict else {
            panic!("expected a mismatch");
        };
        assert!(!mismatch.changed_on_server);
        assert!(mismatch.message("Hades").contains("corrupted"));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn a_fresh_streaming_digest_skips_the_second_read() {
        let payload = vec![5_u8; 128 * 1024];
        let digest = sha256_hex(&payload);
        let dir = temp_dir("known");
        let missing = dir.join("does-not-exist.part");

        // `known` is trusted even though the path does not exist, proving the
        // verifier did not re-read the file.
        let verdict =
            verify_download(&missing, Some(&digest), Some(&digest), None).expect("verify");
        assert_eq!(verdict, ArchiveVerdict::Verified(digest));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_rows_without_a_hash_are_not_blocked() {
        let payload = vec![9_u8; 4096];
        let (dir, path) = temp_file("legacy", &payload);
        let verdict = verify_download(&path, None, None, None).expect("verify");
        assert_eq!(verdict, ArchiveVerdict::Unverified);
        let warning = legacy_warning(3, 7);
        assert!(warning.contains("VERSION 7"), "{warning}");
        assert!(warning.contains("without strong verification"), "{warning}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stream_installs_document_their_verification_model() {
        assert!(stream_note(Some("abc")).contains("CRC-32"));
        assert!(stream_note(Some("abc")).contains("no archive is retained"));
        assert!(stream_note(None).contains("legacy"));
    }
}
