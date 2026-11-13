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

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

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

/* ------------------------- verifying phase (#223) ------------------------- */

/// Why a hash pass stopped before producing a digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HashFailure {
    Cancelled,
    Paused,
    Failed(String),
}

/// Bytes hashed per read. Large enough that a 100 GB archive costs one pass,
/// small enough that cancel/pause and progress stay responsive.
const HASH_CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// Hash `path` from byte 0, reporting read progress.
///
/// A resumed or pre-existing `.part` has no trusted streaming digest, so it is
/// re-read entirely (#216). That read is what looked like a frozen 100 %
/// download before #223: it now reports `progress(done, total)` and honours
/// cancel/pause between chunks, and it never deletes or modifies the file.
pub(crate) fn hash_file_with_progress(
    path: &Path,
    cancel: &CancellationToken,
    pause: &CancellationToken,
    mut progress: impl FnMut(u64, u64),
) -> Result<String, HashFailure> {
    let mut file = std::fs::File::open(path).map_err(|error| {
        HashFailure::Failed(format!("Could not read {}: {error}", path.display()))
    })?;
    let total = file
        .metadata()
        .map(|metadata| metadata.len())
        .unwrap_or_default();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; HASH_CHUNK_BYTES];
    let mut done = 0u64;
    progress(0, total);

    loop {
        if cancel.is_cancelled() {
            return Err(HashFailure::Cancelled);
        }
        if pause.is_cancelled() {
            return Err(HashFailure::Paused);
        }
        let read = file.read(&mut buffer).map_err(|error| {
            HashFailure::Failed(format!("Could not read {}: {error}", path.display()))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        done = done.saturating_add(read as u64);
        progress(done, total);
    }

    Ok(hex::encode(hasher.finalize()))
}

/// A verified digest with the fingerprint of the bytes it was computed from.
///
/// Stored next to the `.part` so a later attempt short-circuits the re-read: an
/// unchanged 96 GB archive must not be hashed again on every retry (#223).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DigestRecord {
    pub size: u64,
    /// Modification time in milliseconds since the Unix epoch.
    pub mtime_ms: i64,
    /// ETag of the response that produced the bytes, when the server sent one.
    pub etag: Option<String>,
    pub sha256: String,
}

/// `{part}.verified.json`, next to the archive it describes.
pub(crate) fn cache_path(part: &Path) -> PathBuf {
    let mut name = part
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".verified.json");
    part.with_file_name(name)
}

/// `(size, mtime_ms)` of a file, the fingerprint the cache compares.
pub(crate) fn file_stamp(path: &Path) -> Option<(u64, i64)> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let since_epoch = modified.duration_since(UNIX_EPOCH).ok()?;
    Some((metadata.len(), since_epoch.as_millis() as i64))
}

pub(crate) fn load_digest(part: &Path) -> Option<DigestRecord> {
    let raw = std::fs::read_to_string(cache_path(part)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Best effort: a missing sidecar only costs one extra read.
pub(crate) fn store_digest(part: &Path, record: &DigestRecord) {
    let path = cache_path(part);
    match serde_json::to_string(record) {
        Ok(encoded) => {
            if let Err(error) = std::fs::write(&path, encoded) {
                tracing::debug!(error = %error, path = %path.display(), "could not write the verified digest");
            }
        }
        Err(error) => tracing::debug!(error = %error, "could not encode the verified digest"),
    }
}

pub(crate) fn clear_digest(part: &Path) {
    if let Err(error) = std::fs::remove_file(cache_path(part)) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::debug!(error = %error, "could not delete the verified digest");
        }
    }
}

/// The cached digest, when it still describes these bytes and this version.
///
/// A resume appends to the file, which changes the size and the mtime, so the
/// cache is invalidated by construction. A different expected digest (the
/// server replaced the archive) or a different ETag invalidates it too.
pub(crate) fn cached_digest(
    part: &Path,
    expected: &str,
    etag: Option<&str>,
) -> Option<DigestRecord> {
    let record = load_digest(part)?;
    let (size, mtime_ms) = file_stamp(part)?;
    if record.size != size || record.mtime_ms != mtime_ms {
        return None;
    }
    if !record.sha256.eq_ignore_ascii_case(expected) {
        return None;
    }
    if let (Some(known), Some(current)) = (record.etag.as_deref(), etag) {
        if known != current {
            return None;
        }
    }
    Some(record)
}

/// True when the archive has to be read from byte 0 (#223).
pub(crate) fn needs_rehash(
    part: &Path,
    expected: Option<&str>,
    known: Option<&str>,
    etag: Option<&str>,
) -> bool {
    let Some(expected) = expected.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    if known.map(str::trim).is_some_and(|value| !value.is_empty()) {
        return false;
    }
    cached_digest(part, expected, etag).is_none()
}

/// Verification with progress and the digest cache, for the install pipeline.
///
/// `known` is a digest computed while writing the file from byte 0 and wins
/// over both the cache and a re-read. `served` is the digest the server put on
/// the response, used to tell a replaced archive from a corrupted transfer.
#[allow(clippy::too_many_arguments)]
pub(crate) fn verify_archive(
    part: &Path,
    expected: Option<&str>,
    known: Option<&str>,
    served: Option<&str>,
    etag: Option<&str>,
    cancel: &CancellationToken,
    pause: &CancellationToken,
    progress: impl FnMut(u64, u64),
) -> Result<ArchiveVerdict, HashFailure> {
    let Some(expected) = expected.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(ArchiveVerdict::Unverified);
    };

    let actual = match known.map(str::trim).filter(|value| !value.is_empty()) {
        Some(digest) => digest.to_string(),
        None => match cached_digest(part, expected, etag) {
            Some(record) => {
                tracing::info!(
                    path = %part.display(),
                    bytes = record.size,
                    sha256 = %record.sha256,
                    "archive digest reused from the verification sidecar"
                );
                record.sha256
            }
            None => {
                let digest = hash_file_with_progress(part, cancel, pause, progress)?;
                if let Some((size, mtime_ms)) = file_stamp(part) {
                    store_digest(
                        part,
                        &DigestRecord {
                            size,
                            mtime_ms,
                            etag: etag.map(str::to_string),
                            sha256: digest.clone(),
                        },
                    );
                }
                digest
            }
        },
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

    /* --------------------- verifying phase (#223) --------------------- */

    #[test]
    fn a_hash_pass_reports_progress_and_finishes_with_the_digest() {
        let payload: Vec<u8> = (0..9 * 1024 * 1024)
            .map(|index| (index % 251) as u8)
            .collect();
        let (dir, path) = temp_file("progress", &payload);
        let seen: std::sync::Mutex<Vec<(u64, u64)>> = std::sync::Mutex::new(Vec::new());

        let digest = hash_file_with_progress(
            &path,
            &CancellationToken::new(),
            &CancellationToken::new(),
            |done, total| seen.lock().unwrap().push((done, total)),
        )
        .expect("hash");

        assert_eq!(digest, sha256_hex(&payload));
        let seen = seen.lock().unwrap();
        assert!(
            seen.len() >= 3,
            "progress must be reported per chunk: {seen:?}"
        );
        assert_eq!(seen.first().copied(), Some((0, payload.len() as u64)));
        assert_eq!(
            seen.last().copied(),
            Some((payload.len() as u64, payload.len() as u64))
        );
        // Monotonic: a progress bar must never jump backwards.
        assert!(seen.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_stops_a_hash_pass_and_keeps_the_file() {
        let payload: Vec<u8> = vec![3_u8; 16 * 1024 * 1024];
        let (dir, path) = temp_file("cancel-hash", &payload);
        let cancel = CancellationToken::new();
        let seen = std::sync::Mutex::new(0_u64);

        let result =
            hash_file_with_progress(&path, &cancel, &CancellationToken::new(), |done, _| {
                *seen.lock().unwrap() = done;
                if done > 0 {
                    cancel.cancel();
                }
            });

        assert_eq!(result, Err(HashFailure::Cancelled));
        assert!(
            *seen.lock().unwrap() < payload.len() as u64,
            "cancelling must stop before reading the whole archive"
        );
        // The verifier never deletes or truncates what it reads.
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            payload.len() as u64
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_verified_digest_is_reused_instead_of_rehashing_the_archive() {
        let payload = vec![1_u8; 4 * 1024 * 1024];
        let (dir, path) = temp_file("cache-hit", &payload);
        let expected = sha256_hex(&payload);

        let first = verify_archive(
            &path,
            Some(&expected),
            None,
            None,
            None,
            &CancellationToken::new(),
            &CancellationToken::new(),
            |_, _| {},
        )
        .expect("first verify");
        assert_eq!(first, ArchiveVerdict::Verified(expected.clone()));
        assert!(cache_path(&path).is_file(), "the sidecar must be written");
        assert!(!needs_rehash(&path, Some(&expected), None, None));

        // Claim the file was verified against a different archive while keeping
        // the stamp of the bytes on disk. A cache hit returns `expected` without
        // reading, so the panicking progress closure proves there is no re-read.
        let (size, mtime_ms) = file_stamp(&path).expect("stamp");
        let other = sha256_hex(&vec![0_u8; payload.len()]);
        store_digest(
            &path,
            &DigestRecord {
                size,
                mtime_ms,
                etag: None,
                sha256: other.clone(),
            },
        );
        let cached = verify_archive(
            &path,
            Some(&other),
            None,
            None,
            None,
            &CancellationToken::new(),
            &CancellationToken::new(),
            |_, _| panic!("a cached digest must not re-read the archive"),
        )
        .expect("cached verify");
        assert_eq!(cached, ArchiveVerdict::Verified(other));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_resumed_archive_invalidates_the_cached_digest() {
        let payload = vec![2_u8; 1024 * 1024];
        let (dir, path) = temp_file("cache-resume", &payload);
        let expected = sha256_hex(&payload);
        verify_archive(
            &path,
            Some(&expected),
            None,
            None,
            None,
            &CancellationToken::new(),
            &CancellationToken::new(),
            |_, _| {},
        )
        .expect("verify");
        assert!(!needs_rehash(&path, Some(&expected), None, None));

        // A resume appends bytes: size and mtime change with it.
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            file.write_all(b"more").unwrap();
        }
        assert!(
            needs_rehash(&path, Some(&expected), None, None),
            "an appended .part must be hashed from byte 0"
        );
        let verdict = verify_archive(
            &path,
            Some(&expected),
            None,
            None,
            None,
            &CancellationToken::new(),
            &CancellationToken::new(),
            |_, _| {},
        )
        .expect("verify");
        assert!(
            matches!(verdict, ArchiveVerdict::Mismatch(_)),
            "{verdict:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_different_expected_digest_or_etag_bypasses_the_cache() {
        let payload = vec![4_u8; 512 * 1024];
        let (dir, path) = temp_file("cache-etag", &payload);
        let good = sha256_hex(&payload);
        verify_archive(
            &path,
            Some(&good),
            None,
            None,
            Some("\"v1\""),
            &CancellationToken::new(),
            &CancellationToken::new(),
            |_, _| {},
        )
        .expect("verify");

        // The server replaced the archive: a new expected digest is a new job.
        let other = sha256_hex(&[9_u8; 128]);
        assert!(needs_rehash(&path, Some(&other), None, Some("\"v1\"")));
        // Same bytes but a new ETag means the server re-uploaded them.
        assert!(needs_rehash(&path, Some(&good), None, Some("\"v2\"")));
        // A streaming digest always wins: no read at all.
        assert!(!needs_rehash(&path, Some(&other), Some(&other), None));

        clear_digest(&path);
        assert!(load_digest(&path).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
