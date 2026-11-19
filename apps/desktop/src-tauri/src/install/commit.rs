//! The commit half of an install: turn a source into an indexed game folder.
//!
//! It used to be one 828-line `execute`. It is now a small orchestrator over
//! named stages, each of which owns one disk transition:
//!
//! 1. [`InstallLayout::prepare`] resolves and validates the folders (no writes
//!    beyond creating the install root).
//! 2. [`install_from_files`] / [`install_from_stream`] /
//!    [`install_from_archive`] run the transports in priority order and record
//!    what they produced in [`PreparedSource`].
//! 3. [`prepare_tree`] copies a single-file game or extracts the archive into
//!    staging.
//! 4. [`move_into_place`] moves staging to the final folder.
//! 5. [`record_manifest`] and [`index_install`] persist the manifest and the
//!    installed row, then [`remove_previous`] deletes the old update target.
//!
//! Every stage keeps the pre-split ordering, so the installed bytes, the
//! database writes and the emitted events are unchanged; the pipeline gate test
//! at the bottom pins the installed-tree digest of a representative archive.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};

use crate::db::{self, InstalledGame};
use crate::manifest;
use crate::net::CancelSet;
use crate::paths;
use crate::settings::LocalSettings;
use crate::space;
use crate::state::{AppState, JobEntry, SlotGuard};
use crate::util::{now_rfc3339, short_id};

use super::download::{cleanup_archive, download, download_write_failure, extraction_failure};
use super::files::{manifest_install, ManifestAttempt};
use super::job::{Failure, InstallParams, InstallPhase, JobPaths};
use super::plan::{absolute_path, effective_staging};
use super::scheduler::{dispatch, persist, set_progress};
use super::source::{
    local_archive_path, stream_requested, stream_with_retry, InstallStreamAttempt, StreamOutcome,
};
use super::verify::{
    ensure_extraction_space, expected_archive_metadata, verify_archive_on_disk, ArchiveMetadata,
};

/// Wait for a slot without losing cancel/pause responsiveness (#221).
async fn acquire_slot(
    entry: &JobEntry,
    limiter: &std::sync::Arc<crate::state::SlotLimiter>,
) -> Result<SlotGuard, Failure> {
    let stops = CancelSet::new(&entry.cancel, &entry.pause_token());
    Ok(stops.select(limiter.acquire()).await?)
}

/// Take (or keep) the network slot before a transfer.
async fn hold_download(
    app: &AppHandle,
    entry: &JobEntry,
    slot: &mut Option<SlotGuard>,
) -> Result<(), Failure> {
    if slot.is_some() {
        return Ok(());
    }
    let limiter = std::sync::Arc::clone(&app.state::<AppState>().download_slots);
    *slot = Some(acquire_slot(entry, &limiter).await?);
    Ok(())
}

/// Give the network slot back as soon as the bytes are on disk (#221). The
/// next queued download starts immediately, while this job verifies/extracts.
fn release_download(app: &AppHandle, slot: &mut Option<SlotGuard>) {
    if slot.take().is_some() {
        dispatch(app);
    }
}

/// Take (or keep) the single extraction/commit slot.
async fn hold_extract(
    app: &AppHandle,
    entry: &JobEntry,
    slot: &mut Option<SlotGuard>,
) -> Result<(), Failure> {
    if slot.is_some() {
        return Ok(());
    }
    let limiter = std::sync::Arc::clone(&app.state::<AppState>().extract_slots);
    *slot = Some(acquire_slot(entry, &limiter).await?);
    Ok(())
}

/// Absolute folders and names of one install attempt.
struct InstallLayout {
    install_root: PathBuf,
    slug: String,
    version_label: String,
    final_path: PathBuf,
    staging: PathBuf,
}

impl InstallLayout {
    /// Resolve the install root, create it, and pin the final/staging folders
    /// on the job. Fails before any transfer if the root is not writable.
    fn prepare(
        entry: &JobEntry,
        params: &InstallParams,
        settings: &LocalSettings,
    ) -> Result<Self, Failure> {
        let install_root = params
            .install_dir
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| settings.install_path());
        let install_root = absolute_path(install_root)?;
        std::fs::create_dir_all(&install_root).map_err(|e| {
            Failure::Failed(format!(
                "Could not create the install folder {}: {e}",
                install_root.display()
            ))
        })?;

        let slug = paths::slug_for_title(&params.title);
        let version_label = paths::version_label(params.version.as_deref());
        let game_dir = paths::safe_child(&install_root, &slug)?;
        let final_path = paths::safe_child(&game_dir, &version_label)?;
        entry.set_install_path(Some(final_path.to_string_lossy().to_string()));

        let job_id = entry.snapshot().job_id;
        let staging = install_root.join(format!(".{slug}.staging-{}", short_id(&job_id)));
        entry.set_staging_path(Some(staging.to_string_lossy().to_string()));
        // The staging folder is NOT cleared here: a resumable stream install
        // adopts the folder of its previous attempt and needs the completed
        // files (#115). Every non-resumable branch resets it before writing.

        Ok(Self {
            install_root,
            slug,
            version_label,
            final_path,
            staging,
        })
    }
}

/// What the transports produced for one attempt.
#[derive(Default)]
struct PreparedSource {
    /// Archive to extract or copy (classic download or local library file).
    archive: Option<PathBuf>,
    /// Temporary archive this attempt created and may clean up afterwards.
    downloaded: Option<PathBuf>,
    /// Digest the server published for this archive (#216).
    expected_sha256: Option<String>,
    /// Digest proved before extraction (#216/#223).
    verified_sha256: Option<String>,
    /// Archive format the server published (#226).
    archive_format: Option<String>,
    /// Per-file digests collected while the transport wrote the files (#240).
    collected_manifest: Option<manifest::FileManifest>,
    streamed: bool,
    files_installed: bool,
    manifest_fallback: bool,
}

impl PreparedSource {
    fn new(metadata: ArchiveMetadata) -> Self {
        Self {
            expected_sha256: metadata.sha256,
            archive_format: metadata.format,
            ..Self::default()
        }
    }

    fn reset_checkpoints(&self, entry: &JobEntry) {
        entry.set_stream_completed(None);
        entry.set_staging_path(None);
    }
}

/// What [`commit`] produced: the final folder, the setup payload found when no
/// game executable could be recorded (#214), and the executable warning of a
/// completed install whose configured path was stale (#311).
pub(crate) struct ExecutedInstall {
    pub(crate) install_path: PathBuf,
    pub(crate) pending_setup: Option<crate::installer::RepackLayout>,
    /// Set when the configured executable was not in the extracted tree: the
    /// install is still complete, and this text explains what was chosen (or
    /// how to pick one). The job stays `done`.
    pub(crate) warning: Option<String>,
}

impl ExecutedInstall {
    /// Message of the finished job.
    ///
    /// A repack archive wins (#214: the user must run its setup), then the
    /// executable warning of #311, then the plain "Installed" text. Every case
    /// is a `done` job: none of them is an error and the extracted tree is
    /// always kept.
    pub(crate) fn done_message(&self) -> String {
        if let Some(repack) = &self.pending_setup {
            return format!(
                "Installed. This archive contains a setup program: run {} to finish installing.",
                repack.setup
            );
        }
        if let Some(warning) = &self.warning {
            return warning.clone();
        }
        "Installed".to_string()
    }
}

/// Run one job from its source to an indexed install folder.
///
/// The steps are deliberately linear: each stage receives only what the next
/// needs, and the two slots are released as early as the old code did.
pub(crate) async fn commit(
    app: &AppHandle,
    entry: &std::sync::Arc<JobEntry>,
    job_paths: &mut JobPaths,
    download_slot: &mut Option<SlotGuard>,
    extract_slot: &mut Option<SlotGuard>,
) -> Result<ExecutedInstall, Failure> {
    let state = app.state::<AppState>();
    let params = entry.params.clone();
    let settings = state.settings_snapshot();

    let layout = InstallLayout::prepare(entry, &params, &settings)?;
    job_paths.staging = Some(layout.staging.clone());
    let stops = CancelSet::new(&entry.cancel, &entry.pause_token());
    stops.check()?;

    let mut source = PreparedSource::new(expected_archive_metadata(&state, &params).await);

    // #231/#235: the file-manifest transport comes first, before the local
    // archive shortcut, because its whole point is to never read or write an
    // archive.
    let download_root = download_root(&params, &settings, &layout.install_root);

    if settings.download_policy.wants_files() {
        install_from_files(
            app,
            entry,
            &params,
            &layout,
            &download_root,
            &mut source,
            job_paths,
        )
        .await?;
    }

    if !source.files_installed {
        if let Some(local) = local_archive_path(&state, &params).await {
            tracing::info!(path = %local.display(), "using local library archive");
            set_progress(
                app,
                entry,
                InstallPhase::Downloading,
                0,
                None,
                0,
                Some("Using the local library file".to_string()),
            );
            source.archive = Some(local);
        }
    }

    // 2. Stream install (remote ZIP only): write the entries straight into
    // staging. Non-ZIP archives and older servers fall back to the classic
    // resumable download before any data is written (#203).
    if source.archive.is_none()
        && !source.files_installed
        && !source.manifest_fallback
        && stream_requested(params.stream, &settings, &params.server_url)
    {
        install_from_stream(
            app,
            entry,
            &params,
            &layout,
            &mut source,
            job_paths,
            extract_slot,
        )
        .await?;
    }

    // 3. Otherwise download with resume.
    if source.archive.is_none() && !source.streamed && !source.files_installed {
        install_from_archive(
            app,
            entry,
            &params,
            &layout,
            &download_root,
            &mut source,
            job_paths,
            download_slot,
            extract_slot,
        )
        .await?;
    }

    stops.check_cancelled()?;

    // #225: `stream_install` can adopt the staging folder of a previous job
    // and records it in `job_paths.staging`. Everything that inspects or
    // commits staged files must use the folder the transport actually wrote
    // to; the locally computed path is only a fallback.
    let staging = effective_staging(job_paths, &layout.staging);

    // Verification and extraction are disk work: one at a time (#221). The
    // stream path already took the extractor; the classic path took it in
    // `install_from_archive`; the local shortcut takes it here.
    hold_extract(app, entry, extract_slot).await?;

    // The local shortcut reads the library file directly: verify it too, once,
    // before extraction. The classic path already proved its digest.
    if !source.streamed && !source.files_installed && source.verified_sha256.is_none() {
        if let Some(path) = source.archive.clone() {
            let verdict = verify_archive_on_disk(
                app,
                entry,
                &params,
                &path,
                source.expected_sha256.as_deref(),
                None,
                None,
                None,
            )
            .await?;
            match &verdict {
                crate::archive_verify::ArchiveVerdict::Verified(digest) => {
                    tracing::info!(
                        game_id = params.game_id,
                        version_id = params.version_id,
                        sha256 = %digest,
                        "local archive verified against the server SHA-256"
                    );
                    source.verified_sha256 = crate::archive_verify::trusted_digest(&verdict);
                }
                crate::archive_verify::ArchiveVerdict::Unverified => {}
                crate::archive_verify::ArchiveVerdict::Mismatch(mismatch) => {
                    // A library file is never deleted: report and stop.
                    return Err(Failure::Failed(mismatch.message(&params.title)));
                }
            }
        }
    }

    if let Some(digest) = source.verified_sha256.clone() {
        entry.update(|job| job.verified_sha256 = Some(digest));
        persist(app, entry);
    }

    // Stream installs retain no archive: the per-entry CRC-32 and size checks
    // are the verification model, recorded in the stream manifest (#216).
    if source.streamed {
        tracing::info!(
            game_id = params.game_id,
            version_id = params.version_id,
            "{}",
            crate::archive_verify::stream_note(source.expected_sha256.as_deref())
        );
    }
    // File-manifest installs retain no archive either: every file was proven
    // against the manifest's SHA-256 before it was renamed into place (#231).
    if source.files_installed {
        tracing::info!(
            game_id = params.game_id,
            version_id = params.version_id,
            "file manifest verified: every file matched its SHA-256; no archive was written"
        );
    }

    // Fingerprint the archive before it is deleted: it is how update detection
    // knows whether the installed files came from this exact archive.
    let archive_hash = fingerprint_archive(source.archive.clone()).await;

    let relative_executable = prepare_tree(
        app,
        entry,
        &params,
        &settings,
        &layout,
        &staging,
        &mut source,
        &stops,
    )
    .await?;

    stops.check_cancelled()?;

    // 5. Move staging into place (rename, or copy across devices). For a stream
    // install this is the folder it adopted from the previous attempt (#225).
    move_into_place(app, entry, &staging, &layout.final_path).await?;

    if let Some(warning) = relative_executable.warning.as_deref() {
        let detected = relative_executable
            .relative
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_string());
        tracing::warn!(
            game_id = params.game_id,
            version_id = params.version_id,
            configured = params.executable.as_deref().unwrap_or("none"),
            detected = %detected,
            warning,
            "the configured executable was not in the extracted tree; the install still completed"
        );
    }
    let executable_path = relative_executable
        .relative
        .map(|relative| layout.final_path.join(relative));

    // 6. Record the file manifest so the install can be verified and repaired
    // later. #240: the digests come from the transport; only a legacy or repair
    // install still hashes the folder here.
    record_manifest(
        app,
        entry,
        &layout.final_path,
        &params,
        source.collected_manifest,
        source.verified_sha256.clone(),
    )
    .await;

    // 7. Index the install.
    let installed = index_install(
        &state,
        &params,
        &layout,
        executable_path.as_deref(),
        archive_hash,
    )
    .await?;

    entry.set_part_path(None);

    // Update installs: the previous folder is only removed once the new one is
    // indexed, so a failed update never destroys a playable install. Saves live
    // outside the install folder and are never touched.
    remove_previous(&params, &layout).await;

    // #214: a repack archive finishes with no executable. Detect the setup
    // program now so the job can point the user at the guided flow; nothing is
    // executed here.
    let pending_setup = if installed.executable.is_none() {
        let root = layout.final_path.clone();
        tokio::task::spawn_blocking(move || crate::installer::detect_repack(&root))
            .await
            .unwrap_or(None)
    } else {
        None
    };

    Ok(ExecutedInstall {
        install_path: layout.final_path,
        pending_setup,
        warning: relative_executable.warning,
    })
}

/// Download folder of the attempt: the job's pinned folder, then the staging
/// policy (#226).
fn download_root(params: &InstallParams, settings: &LocalSettings, install_root: &Path) -> PathBuf {
    params
        .download_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| settings.effective_download_dir(install_root))
}

/// Try the file-manifest transport (#231/#235).
async fn install_from_files(
    app: &AppHandle,
    entry: &std::sync::Arc<JobEntry>,
    params: &InstallParams,
    layout: &InstallLayout,
    download_root: &Path,
    source: &mut PreparedSource,
    job_paths: &mut JobPaths,
) -> Result<(), Failure> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    match manifest_install(app, entry, params, &layout.staging, download_root).await {
        Ok(ManifestAttempt::Installed(report)) => {
            source.files_installed = true;
            source.verified_sha256 = Some(report.archive_sha256.clone());
            // #240: the server's manifest is exactly the per-file digest set
            // this install wrote and verified, so it becomes the local
            // manifest without a single read of the installed files.
            source.collected_manifest = Some(manifest::FileManifest::from_entries(report.entries));
            entry.update(|job| {
                job.verified_sha256 = Some(report.archive_sha256.clone());
                job.bytes_done = report.bytes;
                job.bytes_total = Some(report.bytes);
                job.entries_done = Some(report.files as u32);
                job.entries_total = Some(report.files as u32);
                job.current_entry = None;
                job.speed_bps = 0;
                job.resumable = false;
                job.clear_reconnect();
                job.message = Some(format!("Downloaded {} files", report.files));
            });
            // No archive exists on this device: nothing to keep and nothing to
            // clean up on the classic path.
            job_paths.part = None;
            entry.set_part_path(None);
            job_paths.staging = Some(layout.staging.clone());
            persist(app, entry);
        }
        Ok(ManifestAttempt::NotAvailable) => {
            // `auto` found no ready cache and nothing was written: the job
            // continues on its normal pipeline.
        }
        Ok(ManifestAttempt::Classic) => {
            // Only an explicit `files` choice is documented as "fall back to
            // the classic archive once".
            source.manifest_fallback = true;
            let classic_peak = space::SpacePlan::build(
                params.expected_bytes.or(entry.snapshot().bytes_total),
                None,
                &layout.install_root,
                download_root,
                false,
                settings.keep_archives,
            );
            let job_id = entry.snapshot().job_id;
            if let space::SpaceReservation::Waiting(status) =
                state
                    .space
                    .try_reserve(&job_id, &classic_peak, &space::SystemCapacity)
            {
                return Err(Failure::Failed(status.message()));
            }
        }
        Err(failure) => return Err(failure),
    }
    Ok(())
}

/// Try the per-entry ZIP stream, falling back to the archive on the documented
/// conditions (#203/#204).
async fn install_from_stream(
    app: &AppHandle,
    entry: &std::sync::Arc<JobEntry>,
    params: &InstallParams,
    layout: &InstallLayout,
    source: &mut PreparedSource,
    job_paths: &mut JobPaths,
    extract_slot: &mut Option<SlotGuard>,
) -> Result<(), Failure> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    // #221: a stream install is network *and* extraction, and it writes entry
    // by entry while it downloads. It holds **both** slots for its whole
    // duration, so it can never interleave with another unpacker.
    hold_extract(app, entry, extract_slot).await?;
    // #212: a stream job retries itself after its per-entry budget is
    // exhausted, with the same checkpoint, instead of failing outright.
    let tuning = crate::net::RetryPolicy {
        network: Some(&state.network),
        retry_now: Some(entry.retry_signal()),
        ..crate::net::RetryPolicy::from_settings(
            settings.download_stall_timeout_secs,
            settings.download_retry_window_secs,
        )
    };
    let pause = entry.pause_token();
    let outcome = {
        let mut attempt = InstallStreamAttempt {
            app,
            entry,
            params,
            staging: &layout.staging,
            job_paths,
        };
        stream_with_retry(
            &mut attempt,
            tuning,
            &entry.cancel,
            &pause,
            |message, attempt, delay| {
                let at = crate::net::retry::retry_at(delay);
                entry.update(|job| {
                    job.phase = InstallPhase::Downloading;
                    job.reconnecting = true;
                    job.attempt = attempt;
                    job.last_error = Some(message.to_string());
                    job.next_retry_at = Some(at);
                    job.waiting_for_network = false;
                    job.message = Some(format!(
                        "Retrying the stream in {} s, attempt {attempt}",
                        delay.as_secs().max(1)
                    ));
                });
                persist(app, entry);
                let _ = app.emit("install://progress", entry.snapshot());
            },
        )
        .await
    };
    match outcome {
        Ok(StreamOutcome::Done { files }) => {
            source.streamed = true;
            // #240: the entries were hashed while they were written (and the
            // adopted ones when this attempt started), so the manifest needs
            // no walk of the staging folder.
            source.collected_manifest = Some(manifest::FileManifest::from_entries(
                files
                    .into_iter()
                    .map(|file| manifest::ManifestEntry {
                        path: file.path,
                        size: file.size,
                        sha256: file.sha256,
                    })
                    .collect(),
            ));
            job_paths.part = None;
            entry.set_part_path(None);
        }
        Ok(StreamOutcome::Unsupported(status)) => {
            tracing::info!(
                status = status.as_u16(),
                game_id = params.game_id,
                version_id = params.version_id,
                archive_format = ?source.archive_format,
                "per-entry stream unsupported; using the resumable download instead"
            );
            let stream_staging = job_paths
                .staging
                .clone()
                .unwrap_or_else(|| layout.staging.clone());
            reset_staging(&stream_staging).await?;
            // #226: name the format when the server published one, so the
            // fallback is not a generic "not available" message.
            let message = super::source::stream_fallback_message(source.archive_format.as_deref());
            // The job is now a normal, resumable download: it pauses and
            // resumes with HTTP Range and a `.part` file (#203/#208).
            entry.update(|job| {
                job.stream = false;
                job.resumable = false;
                job.entries_done = None;
                job.entries_total = None;
                job.current_entry = None;
                job.message = Some(message);
            });
            source.reset_checkpoints(entry);
            persist(app, entry);
            // The job is a normal download now: give the extractor back so
            // another install can prefetch/unpack while it transfers (#221).
            drop(extract_slot.take());
        }
        Err(Failure::StreamUnavailable(message)) => {
            // Every entry attempt failed before receiving a byte: the per-entry
            // path is unusable behind this proxy. Fall back once to the classic
            // resumable download (#204).
            tracing::warn!(
                error = %message,
                game_id = params.game_id,
                version_id = params.version_id,
                "per-entry stream unavailable; falling back to the resumable download"
            );
            let stream_staging = job_paths
                .staging
                .clone()
                .unwrap_or_else(|| layout.staging.clone());
            reset_staging(&stream_staging).await?;
            entry.update(|job| {
                job.stream = false;
                job.resumable = false;
                job.entries_done = None;
                job.entries_total = None;
                job.current_entry = None;
                job.message = Some(format!(
                    "The server could not stream this archive ({message}); downloading instead"
                ));
            });
            source.reset_checkpoints(entry);
            persist(app, entry);
            drop(extract_slot.take());
        }
        Err(failure) => return Err(failure),
    }
    Ok(())
}

/// Download the archive with resume, verifying it once before extraction
/// (#216/#221/#223). A digest mismatch re-downloads once from zero.
#[allow(clippy::too_many_arguments)]
async fn install_from_archive(
    app: &AppHandle,
    entry: &std::sync::Arc<JobEntry>,
    params: &InstallParams,
    layout: &InstallLayout,
    download_root: &Path,
    source: &mut PreparedSource,
    job_paths: &mut JobPaths,
    download_slot: &mut Option<SlotGuard>,
    extract_slot: &mut Option<SlotGuard>,
) -> Result<(), Failure> {
    let state = app.state::<AppState>();
    // A stream attempt may have adopted the staging folder of an earlier job:
    // the classic path starts from a clean folder, so drop it.
    if let Some(previous) = job_paths
        .staging
        .clone()
        .filter(|previous| previous != &layout.staging)
    {
        let _ = tokio::fs::remove_dir_all(&previous).await;
    }
    reset_staging(&layout.staging).await?;
    // The classic path writes to the current job's own folder: make that the
    // effective staging before the commit reads it (#225).
    job_paths.staging = Some(layout.staging.clone());
    let part = job_paths.part.clone().unwrap_or_else(|| {
        download_root.join(format!(".{}-{}.part", params.game_id, params.version_id))
    });
    job_paths.part = Some(part.clone());
    entry.set_part_path(Some(part.to_string_lossy().to_string()));

    // Transfer (network slot) → verify (disk, #223) → extract (the single
    // extractor). The network slot is released as soon as the bytes are on
    // disk, so the next queued game downloads while this one hashes and
    // unpacks (#221). A mismatch re-downloads once from zero (#216): the
    // extractor is then given back so the pipeline cannot deadlock with a
    // stream install waiting for it.
    let mut retried = false;
    loop {
        if download_slot.is_none() {
            hold_download(app, entry, download_slot).await?;
        }
        let report = download(app, entry, params, &part).await?;
        CancelSet::new(&entry.cancel, &entry.pause_token()).check_cancelled()?;
        release_download(app, download_slot);
        entry.update(|job| job.prefetching = false);
        hold_extract(app, entry, extract_slot).await?;

        let verdict = verify_archive_on_disk(
            app,
            entry,
            params,
            &part,
            source.expected_sha256.as_deref(),
            report.served_sha256.as_deref(),
            report.sha256.as_deref(),
            report.etag.as_deref(),
        )
        .await?;
        match &verdict {
            crate::archive_verify::ArchiveVerdict::Verified(digest) => {
                tracing::info!(
                    game_id = params.game_id,
                    version_id = params.version_id,
                    bytes = report.bytes,
                    resumed = report.resumed,
                    sha256 = %digest,
                    "archive verified against the server SHA-256"
                );
                // #240: the proved digest is stored with the install manifest,
                // so nothing re-hashes the tree to prove it again.
                source.verified_sha256 = crate::archive_verify::trusted_digest(&verdict);
                break;
            }
            crate::archive_verify::ArchiveVerdict::Unverified => break,
            crate::archive_verify::ArchiveVerdict::Mismatch(mismatch) => {
                // Never extract these bytes: remove them before anything else
                // can read the `.part`.
                let _ = tokio::fs::remove_file(&part).await;
                crate::archive_verify::clear_digest(&part);
                if !retried {
                    retried = true;
                    tracing::warn!(
                        game_id = params.game_id,
                        version_id = params.version_id,
                        expected = %mismatch.expected,
                        actual = %mismatch.actual,
                        changed_on_server = mismatch.changed_on_server,
                        "archive failed SHA-256 verification; retrying once from zero"
                    );
                    set_progress(
                        app,
                        entry,
                        InstallPhase::Downloading,
                        0,
                        None,
                        0,
                        Some(if mismatch.changed_on_server {
                            "The archive changed on the server; downloading it again".to_string()
                        } else {
                            "The download failed verification; retrying from 0%".to_string()
                        }),
                    );
                    // The archive may have been replaced since the first
                    // metadata read: refresh before re-downloading.
                    let fresh = expected_archive_metadata(&state, params).await;
                    if fresh.sha256.is_some() {
                        source.expected_sha256 = fresh.sha256;
                    }
                    if fresh.format.is_some() {
                        source.archive_format = fresh.format;
                    }
                    drop(extract_slot.take());
                    continue;
                }
                return Err(Failure::Failed(mismatch.message(&params.title)));
            }
        }
    }
    source.archive = Some(part.clone());
    source.downloaded = Some(part);
    Ok(())
}

/// Fingerprint the archive of this attempt (#30 update detection).
async fn fingerprint_archive(archive: Option<PathBuf>) -> Option<String> {
    let path = archive?;
    match tokio::task::spawn_blocking(move || crate::bridge::archive_fingerprint(&path)).await {
        Ok(Ok(value)) => Some(value),
        Ok(Err(error)) => {
            tracing::debug!(error = %error, "could not fingerprint the archive");
            None
        }
        Err(error) => {
            tracing::debug!(error = %error, "archive fingerprint task failed");
            None
        }
    }
}

/// Copy a single-file game or extract an archive into staging, then locate the
/// executable relative to the tree.
#[allow(clippy::too_many_arguments)]
async fn prepare_tree(
    app: &AppHandle,
    entry: &std::sync::Arc<JobEntry>,
    params: &InstallParams,
    settings: &LocalSettings,
    layout: &InstallLayout,
    staging: &Path,
    source: &mut PreparedSource,
    stops: &CancelSet,
) -> Result<ExecutableChoice, Failure> {
    if source.streamed || source.files_installed {
        let snapshot = entry.snapshot();
        set_progress(
            app,
            entry,
            InstallPhase::Finalizing,
            snapshot.bytes_done,
            snapshot.bytes_total,
            0,
            Some("Locating the game executable".to_string()),
        );
        let staging_for_exe = staging.to_path_buf();
        let title = params.title.clone();
        let requested_exe = params.executable.clone();
        return tokio::task::spawn_blocking(move || {
            resolve_executable(&staging_for_exe, requested_exe.as_deref(), &title)
        })
        .await
        .map_err(|e| Failure::Failed(format!("Could not inspect the downloaded files: {e}")))?
        .map_err(Failure::Failed);
    }

    let archive = source
        .archive
        .clone()
        .ok_or_else(|| Failure::Failed("No archive was available for this install.".to_string()))?;
    let archive_size = std::fs::metadata(&archive)
        .map(|metadata| metadata.len())
        .ok();

    if looks_like_single_file(&archive) {
        let file_name = single_file_name(&archive, &layout.slug);
        set_progress(
            app,
            entry,
            InstallPhase::Finalizing,
            0,
            archive_size,
            0,
            Some("Installing game file".to_string()),
        );
        copy_single_file(app, entry, &archive, staging, &file_name).await?;
        if let Some(part) = source.downloaded.as_deref() {
            cleanup_archive(part, settings.keep_archives, &file_name).await;
        }
        return Ok(ExecutableChoice {
            relative: Some(PathBuf::from(file_name)),
            warning: None,
        });
    }

    let archive_name = format!("{}-{}.archive", layout.slug, layout.version_label);
    extract_archive(
        app,
        entry,
        params,
        settings,
        &layout.install_root,
        staging,
        &archive,
        &archive_name,
        source,
        stops,
    )
    .await
}

/// 3a. Single-file game: no archive to extract, copy it into the install
/// folder and use it as the executable.
async fn copy_single_file(
    app: &AppHandle,
    entry: &std::sync::Arc<JobEntry>,
    archive: &Path,
    staging: &Path,
    file_name: &str,
) -> Result<(), Failure> {
    tokio::fs::create_dir_all(staging).await.map_err(|e| {
        Failure::Failed(format!(
            "Could not create the staging folder {}: {e}",
            staging.display()
        ))
    })?;
    let destination = paths::safe_child(staging, file_name)?;
    let source = archive.to_path_buf();
    let target = destination.clone();
    let entry_for_copy = std::sync::Arc::clone(entry);
    let app_for_copy = app.clone();
    tokio::task::spawn_blocking(move || {
        let mut last_emit: Option<Instant> = None;
        paths::copy_file(&source, &target, |done, total| {
            let now = Instant::now();
            let ready = last_emit
                .map(|last| now.duration_since(last) >= Duration::from_millis(250))
                .unwrap_or(true);
            if !ready {
                return;
            }
            last_emit = Some(now);
            entry_for_copy.update(|job| {
                job.phase = InstallPhase::Finalizing;
                job.bytes_done = done;
                job.bytes_total = Some(total);
                job.speed_bps = 0;
                job.clear_reconnect();
                job.message = Some("Installing game file".to_string());
            });
            let _ = app_for_copy.emit("install://progress", entry_for_copy.snapshot());
        })
    })
    .await
    .map_err(|e| Failure::Failed(format!("The file copy task failed: {e}")))?
    .map_err(|e| {
        Failure::Failed(download_write_failure(
            &destination,
            &std::io::Error::other(e),
        ))
    })?;
    Ok(())
}

/// 3b. Extract the archive into staging, collecting the per-file digests.
#[allow(clippy::too_many_arguments)]
async fn extract_archive(
    app: &AppHandle,
    entry: &std::sync::Arc<JobEntry>,
    params: &InstallParams,
    settings: &LocalSettings,
    install_root: &Path,
    staging: &Path,
    archive: &Path,
    archive_name: &str,
    source: &mut PreparedSource,
    stops: &CancelSet,
) -> Result<ExecutableChoice, Failure> {
    let archive_size = std::fs::metadata(archive)
        .map(|metadata| metadata.len())
        .ok();
    // #222: the ledger reserved the peak at admission, but the disk may have
    // been filled by something else since: fail before unpacking and keep the
    // archive so the job resumes after freeing space.
    ensure_extraction_space(install_root, archive_size)?;
    set_progress(
        app,
        entry,
        InstallPhase::Extracting,
        0,
        archive_size,
        0,
        Some("Extracting archive".to_string()),
    );

    let staging_for_extract = staging.to_path_buf();
    let archive_for_extract = archive.to_path_buf();
    let passwords = settings.archive_passwords.clone();
    let entry_for_extract = std::sync::Arc::clone(entry);
    let app_for_extract = app.clone();
    let cancel_for_extract = entry.cancel.clone();
    let extraction = tokio::task::spawn_blocking(move || {
        let mut last_emit: Option<Instant> = None;
        crate::bridge::extract_archive(
            &archive_for_extract,
            &staging_for_extract,
            &passwords,
            Some(cancel_for_extract),
            &mut |update| {
                let now = Instant::now();
                let ready = last_emit
                    .map(|last| now.duration_since(last) >= Duration::from_millis(250))
                    .unwrap_or(true);
                if !ready {
                    return;
                }
                last_emit = Some(now);

                let (done, total) = if update.bytes_total > 0 {
                    (update.bytes_done, Some(update.bytes_total))
                } else {
                    (update.entries_done, Some(update.entries_total))
                };
                // #232: the message carries the rate and a real ETA while the
                // archive is unpacked, instead of a bar that only moves with the
                // file count.
                let mut message = format!(
                    "Extracting {}/{} files",
                    update.entries_done, update.entries_total
                );
                if update.bytes_per_second > 0 {
                    message.push_str(&format!(
                        " · {}",
                        super::download::format_speed(update.bytes_per_second)
                    ));
                }
                if let Some(eta) = update.eta_seconds {
                    message.push_str(&format!(" · {} left", super::download::format_eta(eta)));
                }
                let speed = update.bytes_per_second;
                entry_for_extract.update(|job| {
                    job.phase = InstallPhase::Extracting;
                    job.bytes_done = done;
                    job.bytes_total = total;
                    job.speed_bps = speed;
                    job.clear_reconnect();
                    job.message = Some(message);
                });
                let _ = app_for_extract.emit("install://progress", entry_for_extract.snapshot());
            },
        )
    })
    .await
    .map_err(|e| Failure::Failed(format!("The extraction task failed: {e}")))?;
    let extracted = match extraction {
        Ok(files) => files,
        Err(error) => {
            // #232: a cancel during extraction is a cancellation, not a corrupt
            // archive. It wins over the error message so the job reports
            // "cancelled" and keeps the staging.
            stops.check_cancelled()?;
            return Err(Failure::Failed(extraction_failure(staging, error)));
        }
    };
    tracing::info!(
        files = extracted.len(),
        game_id = params.game_id,
        version_id = params.version_id,
        // #240 counters: the archive is read once (extraction), the tree is
        // written once, and nothing is read back to hash it.
        archive_read_bytes = archive_size.unwrap_or(0),
        tree_written_bytes = extracted.iter().map(|file| file.size).sum::<u64>(),
        tree_read_bytes = 0_u64,
        "extraction produced the per-file digests; no second pass over the installed tree"
    );
    source.collected_manifest = Some(manifest::FileManifest::from_entries(
        extracted
            .into_iter()
            .map(|file| manifest::ManifestEntry {
                path: file.path,
                size: file.size,
                sha256: file.sha256,
            })
            .collect(),
    ));

    // The archive is no longer needed once extraction succeeded.
    if let Some(part) = source.downloaded.as_deref() {
        cleanup_archive(part, settings.keep_archives, archive_name).await;
    }

    stops.check_cancelled()?;

    // 4. Locate the executable inside the staging folder. A missing executable
    // is not fatal: a stale configured path falls back to detection and, when
    // that finds nothing, the install completes with `executable: null` (#311).
    set_progress(
        app,
        entry,
        InstallPhase::Finalizing,
        0,
        None,
        0,
        Some("Locating the game executable".to_string()),
    );
    let staging_for_exe = staging.to_path_buf();
    let title = params.title.clone();
    let requested_exe = params.executable.clone();
    tokio::task::spawn_blocking(move || {
        resolve_executable(&staging_for_exe, requested_exe.as_deref(), &title)
    })
    .await
    .map_err(|e| Failure::Failed(format!("Could not inspect the extracted files: {e}")))?
    .map_err(Failure::Failed)
}

/// 5. Move staging into place with progress events.
async fn move_into_place(
    app: &AppHandle,
    entry: &std::sync::Arc<JobEntry>,
    staging: &Path,
    final_path: &Path,
) -> Result<(), Failure> {
    let staging_for_move = staging.to_path_buf();
    let final_for_move = final_path.to_path_buf();
    let entry_for_move = std::sync::Arc::clone(entry);
    let app_for_move = app.clone();
    tokio::task::spawn_blocking(move || {
        let mut last_emit: Option<Instant> = None;
        paths::move_dir(&staging_for_move, &final_for_move, |done, total| {
            let now = Instant::now();
            let ready = last_emit
                .map(|last| now.duration_since(last) >= Duration::from_millis(250))
                .unwrap_or(true);
            if !ready {
                return;
            }
            last_emit = Some(now);
            entry_for_move.update(|job| {
                job.phase = InstallPhase::Finalizing;
                job.bytes_done = done;
                job.bytes_total = Some(total);
                job.speed_bps = 0;
                job.clear_reconnect();
                job.message = Some("Installing files".to_string());
            });
            let _ = app_for_move.emit("install://progress", entry_for_move.snapshot());
        })
    })
    .await
    .map_err(|e| Failure::Failed(format!("The install could not be finalised: {e}")))?
    .map_err(Failure::Failed)
}

/// 7. Write the installed row and return what the job needs afterwards.
async fn index_install(
    state: &AppState,
    params: &InstallParams,
    layout: &InstallLayout,
    executable: Option<&Path>,
    archive_hash: Option<String>,
) -> Result<InstalledGame, Failure> {
    let size_bytes = {
        let path = layout.final_path.clone();
        tokio::task::spawn_blocking(move || paths::dir_size(&path))
            .await
            .unwrap_or(0)
    };

    let previous_last_launch = {
        let connection = state.db.lock().map_err(|_| {
            Failure::Failed("The local database is unavailable; restart Ludex.".to_string())
        })?;
        db::get_installed(&connection, params.game_id)
            .map_err(Failure::Failed)?
            .and_then(|game| game.last_launched_at)
    };

    let installed = InstalledGame {
        game_id: params.game_id,
        version_id: params.version_id,
        title: params.title.clone(),
        version: params.version.clone(),
        install_path: layout.final_path.to_string_lossy().to_string(),
        executable: executable.map(|path| path.to_string_lossy().to_string()),
        size_bytes: size_bytes as i64,
        installed_at: now_rfc3339(),
        last_launched_at: previous_last_launch,
        archive_hash,
        install_root: Some(layout.install_root.to_string_lossy().to_string()),
        source: db::SOURCE_LUDEX.to_string(),
        launch_uri: None,
    };

    {
        let connection = state.db.lock().map_err(|_| {
            Failure::Failed("The local database is unavailable; restart Ludex.".to_string())
        })?;
        db::upsert_installed(&connection, &installed).map_err(Failure::Failed)?;
    }
    Ok(installed)
}

/// Delete the previous install folder of an update, after the new one is
/// indexed.
async fn remove_previous(params: &InstallParams, layout: &InstallLayout) {
    let Some(previous) = params
        .replace_path
        .as_deref()
        .map(PathBuf::from)
        .filter(|previous| previous != &layout.final_path)
    else {
        return;
    };
    let base = layout.install_root.clone();
    let previous_for_log = previous.clone();
    let removed =
        tokio::task::spawn_blocking(move || paths::remove_installed_dir(&base, &previous))
            .await
            .unwrap_or_else(|error| Err(format!("cleanup task failed: {error}")));
    match removed {
        Ok(()) => {
            tracing::info!(path = %previous_for_log.display(), "removed the previous install after the update")
        }
        Err(error) => tracing::warn!(error = %error, "could not remove the previous install"),
    }
}

/// Persist the manifest of an installed folder (#240).
///
/// `collected` is what the transport produced while writing: the extraction
/// result, the per-entry stream or the server's file manifest. When it is set
/// this function only writes a row — the installed files are not opened again.
/// `None` is the fallback for a legacy install, a repair or a single-file copy:
/// the folder is walked and hashed exactly as before.
///
/// `trusted_archive_sha256` is the digest proved before extraction (#216/#223)
/// or the file manifest's `archiveSha256` (#231). A failure here never fails the
/// install: the game is playable, it just cannot be verified until it is
/// reinstalled.
async fn record_manifest(
    app: &AppHandle,
    entry: &std::sync::Arc<JobEntry>,
    root: &Path,
    params: &InstallParams,
    collected: Option<manifest::FileManifest>,
    trusted_archive_sha256: Option<String>,
) {
    let from_extraction = collected.is_some();
    set_progress(
        app,
        entry,
        InstallPhase::Finalizing,
        0,
        None,
        0,
        Some(
            if from_extraction {
                "Recording file hashes from extraction"
            } else {
                "Recording file manifest"
            }
            .to_string(),
        ),
    );

    let handle = app.clone();
    let job_entry = std::sync::Arc::clone(entry);
    let path = root.to_path_buf();
    let built = tokio::task::spawn_blocking(move || {
        let mut last = Instant::now();
        manifest::manifest_for_install(collected, &path, &mut |progress| {
            if last.elapsed() < Duration::from_millis(250) {
                return;
            }
            last = Instant::now();
            job_entry.update(|job| {
                job.phase = InstallPhase::Finalizing;
                job.bytes_done = progress.bytes_done;
                job.bytes_total = Some(progress.bytes_total.max(1));
                job.speed_bps = 0;
                job.clear_reconnect();
                job.message = Some(format!(
                    "Hashing {} of {} files",
                    progress.files_done, progress.files_total
                ));
            });
            let _ = handle.emit("install://progress", job_entry.snapshot());
        })
    })
    .await;

    let manifest = match built {
        Ok(Ok(manifest)) => manifest,
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "could not build the file manifest");
            return;
        }
        Err(error) => {
            tracing::warn!(error = %error, "the manifest task failed");
            return;
        }
    };

    tracing::info!(
        game_id = params.game_id,
        version_id = params.version_id,
        files = manifest.entries.len(),
        bytes = manifest.total_bytes,
        from_extraction,
        // #240: zero when the digests came from the transport. A non-zero value
        // is the fallback walk (legacy install, repair, single-file copy).
        hashed_bytes = if from_extraction {
            0
        } else {
            manifest.total_bytes
        },
        trusted_archive_sha256 = trusted_archive_sha256.as_deref().unwrap_or("none"),
        "install manifest recorded"
    );

    let encoded = match serde_json::to_string(&manifest) {
        Ok(encoded) => encoded,
        Err(error) => {
            tracing::warn!(error = %error, "could not encode the file manifest");
            return;
        }
    };

    let state = app.state::<AppState>();
    let record = db::InstallManifestRecord {
        game_id: params.game_id,
        version_id: params.version_id,
        install_path: root.to_string_lossy().to_string(),
        manifest: encoded,
        files: manifest.entries.len() as i64,
        bytes: manifest.total_bytes as i64,
        trusted_archive_sha256,
        created_at: manifest.created_at.clone(),
        updated_at: now_rfc3339(),
    };
    match state.db.lock() {
        Ok(connection) => {
            if let Err(error) = db::upsert_manifest(&connection, &record) {
                tracing::warn!(error = %error, "could not persist the file manifest");
            }
        }
        Err(_) => tracing::warn!("could not persist the file manifest: database unavailable"),
    };
}

/// Deletes a staging folder so a non-resumable branch starts from scratch.
pub(super) async fn reset_staging(staging: &Path) -> Result<(), Failure> {
    if tokio::fs::try_exists(staging).await.unwrap_or(false) {
        tokio::fs::remove_dir_all(staging).await.map_err(|e| {
            Failure::Failed(format!(
                "Could not clear the staging folder {}: {e}",
                staging.display()
            ))
        })?;
    }
    Ok(())
}

/// True for standalone game files that must not go through the archive
/// extractor: `.exe`, `.sh`, `.AppImage`, `.bat`, `.cmd`, or anything with an
/// executable magic header (PE, ELF, shebang).
fn looks_like_single_file(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        extension.as_str(),
        "exe" | "sh" | "appimage" | "bat" | "cmd"
    ) {
        return true;
    }

    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut header = [0u8; 4];
    let read = file.read(&mut header).unwrap_or(0);
    if read >= 2 && &header[..2] == b"MZ" {
        return true;
    }
    if read == 4 && &header == b"\x7fELF" {
        return true;
    }
    read >= 2 && &header[..2] == b"#!"
}

fn detect_single_file_extension(path: &Path) -> &'static str {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return "bin";
    };
    let mut header = [0u8; 4];
    let read = file.read(&mut header).unwrap_or(0);
    if read >= 2 && &header[..2] == b"MZ" {
        return "exe";
    }
    if read == 4 && &header == b"\x7fELF" {
        return "elf";
    }
    if read >= 2 && &header[..2] == b"#!" {
        return "sh";
    }
    "bin"
}

fn single_file_name(source: &Path, slug: &str) -> String {
    if let Some(name) = source.file_name().and_then(|value| value.to_str()) {
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if matches!(
            extension.as_str(),
            "exe" | "sh" | "appimage" | "bat" | "cmd"
        ) {
            return paths::sanitize_component(name, slug);
        }
    }
    format!("{slug}.{}", detect_single_file_extension(source))
}

/// Outcome of locating the game executable in a staged tree (#311).
#[derive(Debug)]
struct ExecutableChoice {
    /// Path relative to the install folder; `None` when nothing was found.
    relative: Option<PathBuf>,
    /// Set when the configured path was missing: the install still completes
    /// and the job shows this text instead of failing.
    warning: Option<String>,
}

/// Warning of a completed install whose configured executable was not in the
/// extracted tree and whose detection found nothing either (#311).
const NO_EXECUTABLE_WARNING: &str =
    "The game installed but Ludex could not find an executable; pick one from the game's Launch options.";

/// Resolve the executable to launch from an extracted tree, as a path relative
/// to the install folder. `relative: None` means no executable was found; the
/// install still succeeds with `executable: null`.
///
/// #311: a configured path that is not in the extracted tree is a warning, not
/// a failure. A completed extraction is never discarded because the metadata
/// named a file the archive does not contain (GameVault-style
/// `Versions/<label>/Installation/<game>/<exe>` paths, for instance). The
/// existing detector runs over the extracted files instead; when it finds
/// nothing the install still completes with no executable and the user picks
/// one from the game's Launch options. Only a path that **exists** but escapes
/// the game folder is still an error: that is a broken manifest, not a stale
/// one.
fn resolve_executable(
    root: &Path,
    requested: Option<&str>,
    title: &str,
) -> Result<ExecutableChoice, String> {
    let root_canonical = root
        .canonicalize()
        .map_err(|e| format!("Could not inspect the extracted files: {e}"))?;

    if let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) {
        let candidate = Path::new(requested);
        let full = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            root.join(candidate)
        };
        if !full.exists() {
            return Ok(missing_configured_executable(
                root,
                &root_canonical,
                &full,
                requested,
                title,
            ));
        }
        let canonical = full
            .canonicalize()
            .map_err(|e| format!("Could not resolve {}: {e}", full.display()))?;
        if !canonical.starts_with(&root_canonical) {
            return Err(format!(
                "The configured executable is outside the game folder: {}",
                full.display()
            ));
        }
        let relative = canonical
            .strip_prefix(&root_canonical)
            .map_err(|_| {
                format!(
                    "The configured executable is outside the game folder: {}",
                    full.display()
                )
            })?
            .to_path_buf();
        return Ok(ExecutableChoice {
            relative: Some(relative),
            warning: None,
        });
    }

    Ok(ExecutableChoice {
        relative: detect_executable(root, &root_canonical, title),
        warning: None,
    })
}

/// The configured executable is gone: detect one, or finish without one.
fn missing_configured_executable(
    root: &Path,
    root_canonical: &Path,
    full: &Path,
    requested: &str,
    title: &str,
) -> ExecutableChoice {
    match detect_executable(root, root_canonical, title) {
        Some(relative) => ExecutableChoice {
            warning: Some(format!(
                "The configured executable does not exist after extraction ({}); using the detected {} instead. You can change it in the game's Launch options.",
                requested,
                relative.display()
            )),
            relative: Some(relative),
        },
        None => ExecutableChoice {
            relative: None,
            warning: Some(format!(
                "{} The configured executable was {}; it is not in the extracted files.",
                NO_EXECUTABLE_WARNING,
                full.display()
            )),
        },
    }
}

/// Run the existing detector over the extracted tree and return its choice as a
/// path relative to the install folder.
///
/// `crate::bridge::pick_executable` is the same ranking the install already used
/// when no path was configured: it prefers a file whose name matches the game
/// title (or the game folder that wraps it), prefers shallow and larger
/// binaries, and skips installers, uninstallers, redistributables and crash
/// handlers (`setup*`, `unins*`, `vcredist*`, `UnityCrashHandler*`, `Redist/`,
/// `_CommonRedist/`, `Support/`, …). The result is canonicalised so a link that
/// escapes the game folder is never recorded.
fn detect_executable(root: &Path, root_canonical: &Path, title: &str) -> Option<PathBuf> {
    crate::bridge::pick_executable(root, title).and_then(|path| {
        let absolute = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        let canonical = absolute.canonicalize().ok()?;
        canonical
            .strip_prefix(root_canonical)
            .ok()
            .map(Path::to_path_buf)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest;
    use sha2::{Digest, Sha256};

    /// #240: the manifest the pipeline now persists comes from the extraction,
    /// and it must be byte-identical to the one the old second pass produced.
    ///
    /// The extraction walks every backend's tee (sequential ZIP, parallel ZIP,
    /// 7z through the crate, RAR through its scratch copy); the reference is
    /// `manifest::build_manifest`, exactly what `record_manifest` used to call
    /// on the installed folder.
    #[test]
    fn extracted_digests_match_the_manifest_the_second_pass_built() {
        use std::io::Write as _;

        let root = std::env::temp_dir().join(format!(
            "ludex-extract-manifest-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&root).unwrap();

        let archive_path = root.join("library.zip");
        {
            let file = std::fs::File::create(&archive_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            let big: Vec<u8> = (0..300_000_u32).map(|index| (index % 251) as u8).collect();
            for (name, data) in [
                ("Game/game.exe", b"MZ-ludex".as_slice()),
                ("Game/empty.txt", b"".as_slice()),
                ("Game/data/big.bin", big.as_slice()),
                ("Game/unicode-é.txt", "café".as_bytes()),
            ] {
                writer.start_file(name, options).unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }

        let base = root.join("extraction");
        let digests = crate::bridge::extract_archive(&archive_path, &base, &[], None, &mut |_| {})
            .expect("extract");
        assert_eq!(digests.len(), 4, "one digest per written file");

        // Exactly what the install now stores.
        let collected = manifest::FileManifest::from_entries(
            digests
                .into_iter()
                .map(|file| manifest::ManifestEntry {
                    path: file.path,
                    size: file.size,
                    sha256: file.sha256,
                })
                .collect(),
        );
        // Exactly what it used to store, from a full walk of the folder.
        let rebuilt = manifest::build_manifest(&base, &mut |_| {}).expect("build");

        assert_eq!(
            collected.entries, rebuilt.entries,
            "path, size and sha256 must be byte-identical to the old manifest"
        );
        assert_eq!(collected.total_bytes, rebuilt.total_bytes);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// #240: a job whose transport has no digest still records a manifest, so
    /// legacy installs, repairs and single-file copies keep working.
    #[test]
    fn an_install_without_collected_digests_falls_back_to_the_walk() {
        let root = std::env::temp_dir().join(format!(
            "ludex-manifest-fallback-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin/game.bin"), b"payload").unwrap();

        let manifest =
            manifest::manifest_for_install(None, &root, &mut |_| {}).expect("fallback manifest");
        assert_eq!(manifest.entries.len(), 1);
        assert_eq!(manifest.entries[0].path, "bin/game.bin");
        assert_eq!(manifest.total_bytes, 7);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn single_file_detection() {
        let root = std::env::temp_dir().join(format!(
            "ludex-single-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&root).unwrap();

        let exe = root.join("game.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        assert!(looks_like_single_file(&exe));
        assert_eq!(single_file_name(&exe, "slug"), "game.exe");

        let part = root.join(".1-2.part");
        std::fs::write(&part, b"\x7fELFrest").unwrap();
        assert!(looks_like_single_file(&part));
        assert_eq!(single_file_name(&part, "slug"), "slug.elf");

        let zip = root.join(".1-2.part.zip");
        std::fs::write(&zip, b"PK\x03\x04").unwrap();
        assert!(!looks_like_single_file(&zip));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// SHA-256 over the canonical `path\0size\0sha256\n` list of a manifest.
    ///
    /// Order is the manifest order, which `build_manifest` sorts and every
    /// transport produces deterministically, so this is a stable fingerprint of
    /// the installed tree.
    fn tree_digest(manifest: &manifest::FileManifest) -> String {
        let mut canonical = String::new();
        for entry in &manifest.entries {
            canonical.push_str(&format!(
                "{}\u{0}{}\u{0}{}\n",
                entry.path, entry.size, entry.sha256
            ));
        }
        hex::encode(Sha256::digest(canonical.as_bytes()))
    }

    /// Representative archive for the pipeline gate: an executable, an empty
    /// file, a multi-chunk binary and a non-ASCII name.
    fn representative_archive(root: &Path) -> PathBuf {
        use std::io::Write as _;

        let archive_path = root.join("library.zip");
        let file = std::fs::File::create(&archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let big: Vec<u8> = (0..300_000_u32).map(|index| (index % 251) as u8).collect();
        for (name, data) in [
            ("Game/game.exe", b"MZ-ludex".as_slice()),
            ("Game/empty.txt", b"".as_slice()),
            ("Game/data/big.bin", big.as_slice()),
            ("Game/unicode-é.txt", "café".as_bytes()),
        ] {
            writer.start_file(name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
        archive_path
    }

    /// Wave 3 gate (#237): the installed tree is byte-identical after the
    /// `install.rs` split. The digest below was recorded from the pre-split
    /// pipeline on the same archive; `extracted_digests_...` already proves the
    /// transport-collected manifest equals the old full walk, so pinning the
    /// walk digest pins the install bytes.
    #[test]
    fn the_installed_tree_digest_is_unchanged_by_the_pipeline_split() {
        let root = std::env::temp_dir().join(format!(
            "ludex-tree-gate-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&root).unwrap();

        let archive_path = representative_archive(&root);
        let installed = root.join("installed");
        crate::bridge::extract_archive(&archive_path, &installed, &[], None, &mut |_| {})
            .expect("extract");
        let walked = manifest::build_manifest(&installed, &mut |_| {}).expect("manifest");

        assert_eq!(
            tree_digest(&walked),
            "5751f1925d569a9df85fe9ef16b401e078ae9310e9618f1e69863ad0da2beaaf",
            "the installed tree changed: paths, sizes or digests"
        );
        assert_eq!(walked.entries.len(), 4);
        assert_eq!(walked.total_bytes, 300_013);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Build a throwaway tree for the executable-resolution tests (#311) and
    /// return `(base, staging, final_path)`.
    fn executable_fixture(label: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "ludex-exe-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        let staging = base.join(".game.staging-1");
        std::fs::create_dir_all(&staging).unwrap();
        let final_path = base.join("game/1");
        (base, staging, final_path)
    }

    /// #311: the UNCHARTED case. A GameVault-style configured path points at a
    /// folder the archive never creates; the completed extraction must survive.
    /// Detection stores the game binary it finds and the warning names it.
    #[test]
    fn a_stale_configured_executable_falls_back_to_detection_and_keeps_the_extraction() {
        let (base, staging, final_path) = executable_fixture("stale");
        let game = staging.join("Uncharted Legacy of Thieves Collection");
        std::fs::create_dir_all(game.join("crs")).unwrap();
        std::fs::write(game.join("u4.exe"), b"MZ-u4").unwrap();
        std::fs::write(game.join("u4-l.exe"), b"MZ-u4-l").unwrap();
        std::fs::write(game.join("crs/crs-handler.exe"), b"MZ-crs").unwrap();
        // A helper the existing detector already ignores: it must never win.
        std::fs::write(game.join("unins000.exe"), b"MZ-unins").unwrap();

        let choice = resolve_executable(
            &staging,
            Some("Versions/Unspecified/Installation/Uncharted Legacy of Thieves Collection/u4.exe"),
            "UNCHARTED Legacy of Thieves Collection",
        )
        .expect("a stale configured path is a warning, not an install failure");

        let relative = choice
            .relative
            .expect("the detector still finds the game binary");
        let stored = relative.to_string_lossy().replace('\\', "/");
        assert!(
            stored.starts_with("Uncharted Legacy of Thieves Collection/"),
            "detection must stay inside the game folder: {stored}"
        );
        assert!(stored.ends_with(".exe"), "{stored}");
        assert!(
            !stored.to_ascii_lowercase().contains("unins"),
            "an uninstaller must never be recorded: {stored}"
        );
        let warning = choice.warning.expect("the job must carry a warning");
        assert!(
            warning.contains("does not exist after extraction"),
            "{warning}"
        );
        assert!(
            warning.contains(&stored),
            "the warning must name what was chosen: {warning}"
        );

        // Nothing is discarded: the commit moves this tree into place.
        crate::paths::move_dir(&staging, &final_path, |_, _| {}).expect("move into place");
        assert!(
            !staging.exists(),
            "the staging folder must be moved, not left"
        );
        assert!(
            final_path.join(&relative).is_file(),
            "the game binary moved"
        );
        assert!(
            final_path
                .join("Uncharted Legacy of Thieves Collection/unins000.exe")
                .is_file(),
            "every extracted file moved"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// #311: when detection finds nothing either, the install still completes
    /// with no executable. The job message carries the warning (a `done` job,
    /// never `error`) and the extracted files are kept.
    #[test]
    fn a_stale_configured_executable_with_no_candidate_still_completes() {
        let (base, staging, final_path) = executable_fixture("no-candidate");
        std::fs::write(staging.join("ReadMe.txt"), b"readme").unwrap();
        std::fs::create_dir_all(staging.join("_CommonRedist")).unwrap();
        std::fs::write(staging.join("_CommonRedist/vcredist_x64.exe"), b"MZ").unwrap();
        std::fs::write(staging.join("unins000.exe"), b"MZ").unwrap();

        let choice = resolve_executable(&staging, Some("Game/Game.exe"), "Some Game")
            .expect("no executable is not an install failure");
        assert!(
            choice.relative.is_none(),
            "a redistributable or uninstaller is not a game binary"
        );
        let warning = choice.warning.expect("the job must carry a warning");
        assert!(warning.contains(NO_EXECUTABLE_WARNING), "{warning}");
        assert!(
            warning.contains("Launch options"),
            "the warning names the manual fix: {warning}"
        );

        // The scheduler maps this warning onto `done`: the install completed.
        let outcome = ExecutedInstall {
            install_path: final_path.clone(),
            pending_setup: None,
            warning: Some(warning.clone()),
        };
        assert_eq!(outcome.done_message(), warning);

        crate::paths::move_dir(&staging, &final_path, |_, _| {}).expect("move into place");
        assert!(!staging.exists());
        assert!(final_path.join("ReadMe.txt").is_file());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A configured executable that is in the extracted tree is stored exactly
    /// as before (#311 touches only the missing case).
    #[test]
    fn a_configured_executable_that_exists_is_used_unchanged() {
        let (base, staging, _final_path) = executable_fixture("present");
        std::fs::create_dir_all(staging.join("bin")).unwrap();
        std::fs::write(staging.join("bin/game.exe"), b"MZ").unwrap();

        let choice = resolve_executable(&staging, Some("bin/game.exe"), "Game").unwrap();
        assert_eq!(choice.relative, Some(PathBuf::from("bin/game.exe")));
        assert!(choice.warning.is_none());

        // The absolute form of the same file resolves to the same relative path.
        let absolute = staging.join("bin/game.exe");
        let choice =
            resolve_executable(&staging, Some(&absolute.to_string_lossy()), "Game").unwrap();
        assert_eq!(choice.relative, Some(PathBuf::from("bin/game.exe")));
        assert!(choice.warning.is_none());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Only a path that exists but escapes the game folder is still a hard
    /// failure: that is a broken manifest, not a stale one (#311).
    #[test]
    fn an_executable_that_exists_outside_the_extracted_tree_is_still_an_error() {
        let (base, staging, _final_path) = executable_fixture("outside");
        let outside = base.join("outside.exe");
        std::fs::write(&outside, b"MZ").unwrap();

        let error = resolve_executable(&staging, Some(&outside.to_string_lossy()), "Game")
            .expect_err("an existing path outside the folder must not be recorded");
        assert!(error.contains("outside the game folder"), "{error}");

        let _ = std::fs::remove_dir_all(&base);
    }
}
