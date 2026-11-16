//! Archive verification and the disk preflight before extraction.
//!
//! The server publishes a full-archive SHA-256 (#216); this module reads it,
//! proves the bytes on disk against it once (#223/#240) and re-checks the
//! install volume before unpacking (#222). Verification is deliberately a
//! separate phase: the network slot is already free while the archive is
//! re-read, so the next game can download.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;

use crate::bridge::models::{GameDetail, GameVersion};
use crate::space;
use crate::state::{AppState, JobEntry};
use crate::util::{human_bytes, non_empty};

use super::job::{Failure, InstallParams, InstallPhase};
use super::plan::free_space;
use super::scheduler::{persist, set_progress};
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct ArchiveMetadata {
    /// Full archive SHA-256, when the server published one.
    pub(super) sha256: Option<String>,
    /// `zip`, `7z`, `rar`… `None` on a legacy row or an older server.
    pub(super) format: Option<String>,
}

/// Digest and format the server published for one version (#216/#226).
///
/// `versions` is the shared contract payload (`crate::bridge::models`),
/// re-exported through [`crate::bridge`]; the desktop keeps no shadow DTO
/// (#237).
fn archive_metadata(versions: &[GameVersion], version_id: i64) -> ArchiveMetadata {
    let Some(version) = versions.iter().find(|version| version.id == version_id) else {
        return ArchiveMetadata::default();
    };
    ArchiveMetadata {
        sha256: version
            .content_sha256
            .as_deref()
            .map(|digest| digest.trim().to_ascii_lowercase())
            .filter(|digest| digest.len() == 64),
        format: version
            .archive_format
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase()),
    }
}

/// Full archive SHA-256 and format the server published for this version
/// (#216/#226).
///
/// Read from the game detail payload (players can load it, unlike the editor
/// version list). A missing digest means a legacy row or a backfill still
/// running: the install proceeds with a warning instead of blocking. A failed
/// request is logged and treated the same way, so an offline/local install is
/// never held hostage by metadata.
pub(super) async fn expected_archive_metadata(
    state: &AppState,
    params: &InstallParams,
) -> ArchiveMetadata {
    let base = params.server_url.trim_end_matches('/');
    if base.is_empty() {
        return ArchiveMetadata::default();
    }
    let url = format!("{base}/api/games/{}", params.game_id);
    let mut request = state.http.get(url).timeout(Duration::from_secs(20));
    if let Some(token) = non_empty(params.token.as_deref()) {
        request = request.bearer_auth(token);
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(
                game_id = params.game_id,
                version_id = params.version_id,
                %error,
                "could not read the archive metadata; installing without strong verification"
            );
            return ArchiveMetadata::default();
        }
    };
    if !response.status().is_success() {
        tracing::warn!(
            game_id = params.game_id,
            version_id = params.version_id,
            status = response.status().as_u16(),
            "the server did not return the game detail; installing without strong verification"
        );
        return ArchiveMetadata::default();
    }
    let Ok(detail) = response.json::<GameDetail>().await else {
        return ArchiveMetadata::default();
    };
    let metadata = archive_metadata(&detail.versions, params.version_id);
    match metadata.sha256.as_deref() {
        Some(digest) => tracing::info!(
            game_id = params.game_id,
            version_id = params.version_id,
            sha256 = digest,
            archive_format = ?metadata.format,
            "server published the archive SHA-256; it will be verified before install"
        ),
        None => tracing::warn!(
            "{}",
            crate::archive_verify::legacy_warning(params.game_id, params.version_id)
        ),
    }
    metadata
}

pub(crate) fn verifying_message(done: u64, total: u64) -> String {
    if total > 0 {
        format!(
            "Verifying archive (one pass, {} of {})",
            human_bytes(done),
            human_bytes(total)
        )
    } else {
        "Verifying archive".to_string()
    }
}

/// Verify the archive before extraction with a real `verifying` phase (#223).
///
/// The rehash of a resumed `.part` reads the whole file, which used to look
/// like a frozen 100 % download. It now reports read progress, is skipped when
/// the digest sidecar still matches, and honours cancel/pause between chunks.
#[allow(clippy::too_many_arguments)]
pub(super) async fn verify_archive_on_disk(
    app: &AppHandle,
    entry: &Arc<JobEntry>,
    params: &InstallParams,
    path: &Path,
    expected: Option<&str>,
    served: Option<&str>,
    known: Option<&str>,
    etag: Option<&str>,
) -> Result<crate::archive_verify::ArchiveVerdict, Failure> {
    use crate::archive_verify::{verify_archive, ArchiveVerdict};

    let expected = expected.map(str::to_string);
    let served = served.map(str::to_string);
    let known = known.map(str::to_string);
    let etag = etag.map(str::to_string);
    // Always run through `verify_archive` so the cache is consulted and a
    // verified digest is stored; a from-zero transfer needs no extra read.
    if !crate::archive_verify::needs_rehash(
        path,
        expected.as_deref(),
        known.as_deref(),
        etag.as_deref(),
    ) {
        return verify_archive(
            path,
            expected.as_deref(),
            known.as_deref(),
            served.as_deref(),
            etag.as_deref(),
            &CancellationToken::new(),
            &CancellationToken::new(),
            |_, _| {},
        )
        .map_err(hash_failure);
    }

    let bytes = std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    set_progress(
        app,
        entry,
        InstallPhase::Verifying,
        0,
        Some(bytes),
        0,
        Some(verifying_message(0, bytes)),
    );
    tracing::info!(
        job_id = %entry.snapshot().job_id,
        game_id = params.game_id,
        version_id = params.version_id,
        bytes,
        expected = expected.as_deref().unwrap_or_default(),
        "verifying the downloaded archive before extraction"
    );

    let started = Instant::now();
    let cancel = entry.cancel.clone();
    let pause = entry.pause_token();
    let handle = app.clone();
    let job_entry = Arc::clone(entry);
    let path_buf = path.to_path_buf();
    let blocking = {
        let expected = expected.clone();
        let served = served.clone();
        let known = known.clone();
        let etag = etag.clone();
        let handle = handle.clone();
        let job_entry = Arc::clone(&job_entry);
        move || {
            let mut last_emit = Instant::now();
            verify_archive(
                &path_buf,
                expected.as_deref(),
                known.as_deref(),
                served.as_deref(),
                etag.as_deref(),
                &cancel,
                &pause,
                |done, total| {
                    // The bar must move during a 96 GB read, but a SQLite write
                    // every 4 MiB is wasteful: 500 ms is fast enough for the UI.
                    if last_emit.elapsed() < Duration::from_millis(500) {
                        return;
                    }
                    last_emit = Instant::now();
                    job_entry.update(|job| {
                        job.phase = InstallPhase::Verifying;
                        job.bytes_done = done;
                        job.bytes_total = Some(total.max(1));
                        job.speed_bps = 0;
                        job.clear_reconnect();
                        job.message = Some(verifying_message(done, total));
                    });
                    persist(&handle, &job_entry);
                    let _ = handle.emit("install://progress", job_entry.snapshot());
                },
            )
        }
    };
    let joined = tokio::task::spawn_blocking(blocking)
        .await
        .map_err(|error| Failure::Failed(format!("The verification task failed: {error}")))?;

    let verdict = joined.map_err(hash_failure)?;
    match &verdict {
        ArchiveVerdict::Verified(digest) => {
            tracing::info!(
                job_id = %entry.snapshot().job_id,
                game_id = params.game_id,
                version_id = params.version_id,
                bytes,
                elapsed_ms = started.elapsed().as_millis() as u64,
                sha256 = %digest,
                "archive verification finished"
            );
        }
        ArchiveVerdict::Unverified => tracing::warn!(
            game_id = params.game_id,
            version_id = params.version_id,
            "no archive SHA-256 for this version; extracting without strong verification"
        ),
        ArchiveVerdict::Mismatch(mismatch) => tracing::warn!(
            game_id = params.game_id,
            version_id = params.version_id,
            bytes,
            elapsed_ms = started.elapsed().as_millis() as u64,
            expected = %mismatch.expected,
            actual = %mismatch.actual,
            changed_on_server = mismatch.changed_on_server,
            "archive verification failed"
        ),
    }
    Ok(verdict)
}

pub(super) fn hash_failure(failure: crate::archive_verify::HashFailure) -> Failure {
    match failure {
        crate::archive_verify::HashFailure::Cancelled => Failure::Cancelled,
        crate::archive_verify::HashFailure::Paused => Failure::Paused,
        crate::archive_verify::HashFailure::Failed(message) => Failure::Failed(message),
    }
}

pub(super) fn ensure_extraction_space(
    install_root: &Path,
    archive_bytes: Option<u64>,
) -> Result<(), Failure> {
    let Some(archive) = archive_bytes.filter(|bytes| *bytes > 0) else {
        return Ok(());
    };
    let needed = archive.saturating_mul(space::UNKNOWN_EXTRACTION_PERCENT) / 100;
    let Some(free) = free_space(install_root) else {
        return Ok(());
    };
    let required = space::with_margin(needed);
    if free < required {
        return Err(Failure::Failed(format!(
            "Not enough free space to unpack into {}: {} needed for the extracted files (archive {} × 1.5 plus a safety margin), {} available. The archive was kept; free space on that volume and resume the install.",
            install_root.display(),
            human_bytes(required),
            human_bytes(archive),
            human_bytes(free)
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verifying_message_names_the_bytes_and_never_a_percent() {
        // #223/#240: the row must say what is happening during the rehash, and
        // that it is the single archive pass, never a percent.
        let message = verifying_message(42 * 1024 * 1024 * 1024, 96 * 1024 * 1024 * 1024);
        assert!(message.starts_with("Verifying archive ("), "{message}");
        assert!(message.contains("one pass"), "{message}");
        assert!(message.contains("42.0 GiB"), "{message}");
        assert!(message.contains("96.0 GiB"), "{message}");
        // An unknown size still says "Verifying archive".
        assert_eq!(verifying_message(0, 0), "Verifying archive");
    }

    #[test]
    fn archive_metadata_reads_the_format_defensively() {
        let versions: Vec<GameVersion> = serde_json::from_value(serde_json::json!([
            {
                "id": 1,
                "fileName": "game.7z",
                "size": 10,
                "earlyAccess": false,
                "downloadUrl": "/api/games/1/download?versionId=1",
                "contentSha256": "  AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA  ",
                "archiveFormat": " 7Z ",
            },
            {
                "id": 2,
                "fileName": "legacy.7z",
                "size": 10,
                "earlyAccess": false,
                "downloadUrl": "/api/games/1/download?versionId=2",
            }
        ]))
        .expect("game versions");

        let metadata = archive_metadata(&versions, 1);
        assert_eq!(
            metadata.sha256.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(metadata.format.as_deref(), Some("7z"));

        // A legacy row (or an older server) has neither key.
        let legacy = archive_metadata(&versions, 2);
        assert!(legacy.sha256.is_none());
        assert!(legacy.format.is_none());

        // An unknown version is not an error either.
        assert_eq!(archive_metadata(&versions, 9), ArchiveMetadata::default());
    }
}
