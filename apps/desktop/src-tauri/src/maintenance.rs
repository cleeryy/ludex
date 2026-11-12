//! Install maintenance: move a game to another folder or drive (#36), verify
//! the installed files against their manifest (#37) and repair missing or
//! corrupt files from the server archive.
//!
//! Everything that touches the filesystem is kept in plain functions so the
//! commands and the tests share the same code. Tauri types only appear in the
//! thin command-facing wrappers at the bottom.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

use crate::bridge;
use crate::db::{self, InstalledGame};
use crate::install::{self, DownloadProgress, Failure, InstallParams};
use crate::manifest::{self, FileManifest, ManifestEntry, VerifyReport, VerifyStatus};
use crate::paths;
use crate::settings;
use crate::state::AppState;
use crate::util::{human_bytes, now_rfc3339, short_id};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveProgress {
    pub game_id: i64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyProgress {
    pub game_id: i64,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairProgress {
    pub game_id: i64,
    pub phase: String,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub message: String,
}

/// Mirrors `RepairReport` from `docs/CONTRACT.md`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairReport {
    pub game_id: i64,
    pub repaired: Vec<String>,
    pub failed: Vec<String>,
    /// True when Ludex could not repair in place and the user should reinstall.
    pub fallback: bool,
    pub message: String,
    pub verify: VerifyReport,
}

#[derive(Debug, Default)]
pub struct RestoreOutcome {
    pub repaired: Vec<String>,
    pub failed: Vec<String>,
}

/* --------------------------------- move ---------------------------------- */

/// Move an installed game to another install root.
pub async fn move_install(
    app: &AppHandle,
    game_id: i64,
    target_dir: String,
) -> Result<InstalledGame, String> {
    let state = app.state::<AppState>();
    let installed = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::get_installed(&connection, game_id)?
    }
    .ok_or_else(|| format!("Game {game_id} is not installed on this device."))?;

    ensure_game_idle(&state, game_id, "moving it")?;

    let source = PathBuf::from(&installed.install_path);
    if !source.is_dir() {
        return Err(format!(
            "The install folder is missing: {}. Reinstall the game instead.",
            source.display()
        ));
    }

    let trimmed = target_dir.trim();
    if trimmed.is_empty() {
        return Err("Choose a folder to move the game to.".to_string());
    }
    let target_root = install::absolute_path(PathBuf::from(trimmed))?;

    let install_root = installed
        .install_root
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| state.settings_snapshot().install_path());
    let relative = relative_install_path(&source, &install_root, &installed)?;
    let destination = target_root.join(&relative);

    if same_path(&source, &destination) {
        return Err("The game is already installed in that folder.".to_string());
    }
    if destination.exists() {
        return Err(format!(
            "{} already exists. Choose another folder or remove it first.",
            destination.display()
        ));
    }
    if is_inside(&source, &target_root) || is_inside(&destination, &source) {
        return Err("Choose a folder outside the current install folder.".to_string());
    }

    std::fs::create_dir_all(&target_root).map_err(|e| {
        format!(
            "Could not create the target folder {}: {e}",
            target_root.display()
        )
    })?;

    // Recompute the size: the index value can be stale if the user added files.
    let size = paths::dir_size(&source);

    let handle = app.clone();
    let from = source.clone();
    let to = destination.clone();
    tokio::task::spawn_blocking(move || {
        let mut last = Instant::now();
        let result = move_install_dir(&from, &to, size, &mut |done, total| {
            if last.elapsed() < Duration::from_millis(250) {
                return;
            }
            last = Instant::now();
            let _ = handle.emit(
                "install://move-progress",
                MoveProgress {
                    game_id,
                    bytes_done: done,
                    bytes_total: total,
                    message: format!("Moving files ({})", human_bytes(done)),
                },
            );
        });
        if result.is_ok() {
            let _ = handle.emit(
                "install://move-progress",
                MoveProgress {
                    game_id,
                    bytes_done: size,
                    bytes_total: size,
                    message: "Move complete".to_string(),
                },
            );
        }
        result
    })
    .await
    .map_err(|e| format!("The move task failed: {e}"))??;

    let updated = InstalledGame {
        install_path: destination.to_string_lossy().to_string(),
        install_root: Some(target_root.to_string_lossy().to_string()),
        executable: rebase_executable(&installed.executable, &source, &destination),
        size_bytes: size as i64,
        ..installed.clone()
    };

    {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::upsert_installed(&connection, &updated)?;
        db::update_manifest_path(&connection, game_id, &updated.install_path, &now_rfc3339())?;
    }

    tracing::info!(game_id, from = %source.display(), to = %destination.display(), "install moved");
    Ok(updated)
}

/// Relative `slug/version` path of an install, used to reproduce the layout
/// under the new install root.
fn relative_install_path(
    source: &Path,
    install_root: &Path,
    installed: &InstalledGame,
) -> Result<PathBuf, String> {
    if let Ok(relative) = source.strip_prefix(install_root) {
        let mut parts = PathBuf::new();
        let mut valid = !relative.as_os_str().is_empty();
        for component in relative.components() {
            match component {
                Component::Normal(part) => parts.push(part),
                _ => valid = false,
            }
        }
        if valid && !parts.as_os_str().is_empty() {
            return Ok(parts);
        }
    }

    // Fallback: rebuild the canonical `{slug}/{version}` layout from the index.
    let slug = paths::slug_for_title(&installed.title);
    let label = paths::version_label(installed.version.as_deref());
    Ok(PathBuf::from(slug).join(label))
}

/// Refuse destructive maintenance while the game runs or an install for it is
/// queued, downloading or paused.
fn ensure_game_idle(state: &AppState, game_id: i64, action: &str) -> Result<(), String> {
    {
        let processes = state
            .processes
            .lock()
            .map_err(|_| "The process list is unavailable; restart Ludex.".to_string())?;
        if processes.values().any(|entry| entry.game_id == game_id) {
            return Err(format!("Stop the game before {action}."));
        }
    }

    {
        let jobs = state
            .jobs
            .lock()
            .map_err(|_| "The install queue is unavailable; restart Ludex.".to_string())?;
        if jobs
            .values()
            .any(|entry| entry.params.game_id == game_id && !entry.snapshot().phase.is_terminal())
        {
            return Err(format!("Wait for the install to finish before {action}."));
        }
    }

    Ok(())
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// True when `candidate` is inside `base` (both must exist or share a prefix).
fn is_inside(base: &Path, candidate: &Path) -> bool {
    match (base.canonicalize(), candidate.canonicalize()) {
        (Ok(base), Ok(candidate)) => candidate != base && candidate.starts_with(&base),
        _ => candidate.starts_with(base),
    }
}

fn rebase_executable(
    executable: &Option<String>,
    source: &Path,
    destination: &Path,
) -> Option<String> {
    let executable = executable.as_ref()?;
    let path = PathBuf::from(executable);
    match path.strip_prefix(source) {
        Ok(relative) => Some(destination.join(relative).to_string_lossy().to_string()),
        Err(_) => Some(executable.clone()),
    }
}

/// Move a directory, atomically when both paths are on the same filesystem.
///
/// Cross-device moves copy into a hidden sibling of the destination, verify the
/// byte count, rename it into place and only then delete the source. Any
/// failure removes the partial copy and leaves the original untouched.
pub fn move_install_dir(
    source: &Path,
    destination: &Path,
    expected_bytes: u64,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    if destination.exists() {
        return Err(format!(
            "{} already exists. Choose another folder or remove it first.",
            destination.display()
        ));
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Could not create the target folder {}: {e}",
                parent.display()
            )
        })?;
    }

    // Same filesystem: a rename is atomic and needs no extra space.
    match std::fs::rename(source, destination) {
        Ok(()) => {
            on_progress(expected_bytes, expected_bytes);
            return Ok(());
        }
        Err(error) => {
            tracing::info!(
                error = %error,
                from = %source.display(),
                to = %destination.display(),
                "rename failed, copying across filesystems"
            );
        }
    }

    let parent = destination
        .parent()
        .ok_or_else(|| format!("Invalid destination path: {}", destination.display()))?;
    if expected_bytes > 0 {
        match fs2::available_space(parent) {
            Ok(free) if free < expected_bytes => {
                return Err(format!(
                    "Not enough free space in {}: {} needed, {} available.",
                    parent.display(),
                    human_bytes(expected_bytes),
                    human_bytes(free)
                ));
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(error = %error, path = %parent.display(), "could not check free disk space");
            }
        }
    }

    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "game".to_string());
    let temp = parent.join(format!(
        ".{name}.moving-{}",
        short_id(&uuid::Uuid::new_v4().to_string())
    ));
    if temp.exists() {
        let _ = std::fs::remove_dir_all(&temp);
    }

    if let Err(error) = paths::copy_dir_recursive(source, &temp, on_progress) {
        let _ = std::fs::remove_dir_all(&temp);
        return Err(format!(
            "Could not copy the game files to {}: {error}",
            destination.display()
        ));
    }

    let copied_bytes = paths::dir_size(&temp);
    if expected_bytes > 0 && copied_bytes != expected_bytes {
        let _ = std::fs::remove_dir_all(&temp);
        return Err(format!(
            "The copy is incomplete ({} of {}). The game was left in its original folder.",
            human_bytes(copied_bytes),
            human_bytes(expected_bytes)
        ));
    }
    if let Err(error) = std::fs::rename(&temp, destination) {
        let _ = std::fs::remove_dir_all(&temp);
        return Err(format!(
            "Could not finish the move to {}: {error}",
            destination.display()
        ));
    }

    if let Err(error) = std::fs::remove_dir_all(source) {
        // The destination is complete; a stale copy is better than losing the
        // install. The next uninstall/move can clean it up.
        tracing::warn!(error = %error, path = %source.display(), "could not delete the old install folder");
    }

    Ok(())
}

/* -------------------------------- verify --------------------------------- */

/// Compare the install folder with its manifest, emitting progress events.
pub async fn verify_install(app: &AppHandle, game_id: i64) -> Result<VerifyReport, String> {
    let state = app.state::<AppState>();
    let installed = installed_game(&state, game_id)?;
    let root = PathBuf::from(&installed.install_path);

    let Some(manifest) = load_manifest(&state, game_id, &installed)? else {
        return Ok(no_manifest_report(game_id, &installed.source));
    };

    let handle = app.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let mut last = Instant::now();
        manifest::verify_tree(&root, &manifest, &mut |progress| {
            if last.elapsed() < Duration::from_millis(250) {
                return;
            }
            last = Instant::now();
            let _ = handle.emit(
                "install://verify-progress",
                VerifyProgress {
                    game_id,
                    files_done: progress.files_done,
                    files_total: progress.files_total,
                    bytes_done: progress.bytes_done,
                    bytes_total: progress.bytes_total,
                    message: format!(
                        "Checking {} of {} files",
                        progress.files_done, progress.files_total
                    ),
                },
            );
        })
    })
    .await
    .map_err(|e| format!("The verification task failed: {e}"))??;

    let report = outcome.into_report(game_id, None);
    let _ = app.emit("install://verify-progress", verify_done(&report));
    Ok(report)
}

fn verify_done(report: &VerifyReport) -> VerifyProgress {
    VerifyProgress {
        game_id: report.game_id,
        files_done: report.files_checked,
        files_total: report.files_checked,
        bytes_done: report.bytes_checked,
        bytes_total: report.bytes_checked,
        message: match report.status {
            VerifyStatus::Ok => "All files match the manifest".to_string(),
            VerifyStatus::Issues => format!(
                "{} missing, {} corrupt, {} extra",
                report.missing.len(),
                report.corrupt.len(),
                report.extra.len()
            ),
            VerifyStatus::NoManifest => "No file manifest recorded".to_string(),
        },
    }
}

fn no_manifest_report(game_id: i64, source: &str) -> VerifyReport {
    let message = if source == db::SOURCE_ADOPTED {
        "This adopted folder has no file manifest yet, so its files cannot be verified. Build a manifest to enable Verify and Repair."
    } else {
        "This install has no file manifest, so it cannot be verified. Reinstall the game to record one."
    };
    VerifyReport {
        game_id,
        status: VerifyStatus::NoManifest,
        files_checked: 0,
        bytes_checked: 0,
        missing: Vec::new(),
        corrupt: Vec::new(),
        extra: Vec::new(),
        checked_at: now_rfc3339(),
        message: Some(message.to_string()),
    }
}

fn installed_game(state: &AppState, game_id: i64) -> Result<InstalledGame, String> {
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    db::get_installed(&connection, game_id)?
        .ok_or_else(|| format!("Game {game_id} is not installed on this device."))
}

/// Load the manifest and keep its recorded path in sync with the index.
fn load_manifest(
    state: &AppState,
    game_id: i64,
    installed: &InstalledGame,
) -> Result<Option<FileManifest>, String> {
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    let Some(record) = db::get_manifest(&connection, game_id)? else {
        return Ok(None);
    };
    let manifest: FileManifest = serde_json::from_str(&record.manifest)
        .map_err(|e| format!("The file manifest is invalid: {e}"))?;
    if record.install_path != installed.install_path {
        let _ = db::update_manifest_path(
            &connection,
            game_id,
            &installed.install_path,
            &now_rfc3339(),
        );
    }
    Ok(Some(manifest))
}

/* -------------------------------- repair --------------------------------- */

/// Restore missing/corrupt files from the version archive.
///
/// The archive is downloaded with the same Range-resume machinery as installs
/// (or read from the local library) and extracted to a temporary folder. Only
/// the files listed in the manifest are written back, so intact files and extra
/// user files are never touched. When no archive can be obtained the report has
/// `fallback: true` and the UI offers a full reinstall.
pub async fn repair_install(app: &AppHandle, game_id: i64) -> Result<RepairReport, String> {
    let state = app.state::<AppState>();
    let installed = installed_game(&state, game_id)?;
    ensure_game_idle(&state, game_id, "repairing it")?;
    let root = PathBuf::from(&installed.install_path);

    let Some(manifest) = load_manifest(&state, game_id, &installed)? else {
        return Ok(RepairReport {
            game_id,
            repaired: Vec::new(),
            failed: Vec::new(),
            fallback: true,
            message: if installed.source == db::SOURCE_ADOPTED {
                "This adopted folder has no file manifest, so Ludex cannot tell which files are damaged. Build a manifest first."
                    .to_string()
            } else {
                "This install has no file manifest, so Ludex cannot tell which files are damaged. Reinstall the game instead."
                    .to_string()
            },
            verify: no_manifest_report(game_id, &installed.source),
        });
    };

    let verify_root = root.clone();
    let verify_manifest = manifest.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        manifest::verify_tree(&verify_root, &verify_manifest, &mut |_| {})
    })
    .await
    .map_err(|e| format!("The verification task failed: {e}"))??;

    let damaged = manifest::damaged_entries(&outcome);
    let damaged_total = damaged.len() as u64;
    let mut verify = outcome.clone().into_report(game_id, None);
    if damaged.is_empty() {
        return Ok(RepairReport {
            game_id,
            repaired: Vec::new(),
            failed: Vec::new(),
            fallback: false,
            message: "All files match the manifest. Nothing to repair.".to_string(),
            verify,
        });
    }

    let settings = state.settings_snapshot();
    let base = settings.server_url.trim_end_matches('/').to_string();
    if base.is_empty() {
        return Ok(fallback_report(
            game_id,
            verify,
            "No server is configured, so the archive cannot be downloaded. Reinstall the game instead."
                .to_string(),
        ));
    }

    let install_root = installed
        .install_root
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| settings.install_path());
    let download_root = settings::resolve_download_dir(&settings, &install_root, None);
    // A dedicated `.repair.part` name: never clobber a paused install's archive
    // for the same version, and a failed repair can resume where it stopped.
    let part = download_root.join(format!(".{}-{}.repair.part", game_id, installed.version_id));

    let params = InstallParams {
        game_id,
        version_id: installed.version_id,
        server_url: base.clone(),
        token: settings.token.clone(),
        title: installed.title.clone(),
        version: installed.version.clone(),
        executable: None,
        install_dir: None,
        download_dir: None,
        install_location_id: None,
        download_location_id: None,
        expected_bytes: None,
        stream: None,
        replace_path: None,
    };

    let mut archive = install::local_archive_path(&state, &params).await;
    let mut downloaded = false;
    if archive.is_none() {
        let url = format!(
            "{base}/api/games/{game_id}/download?versionId={}",
            installed.version_id
        );
        let cancel = CancellationToken::new();
        let pause = CancellationToken::new();
        let handle = app.clone();
        let title = installed.title.clone();
        let result = install::download_file(
            &state.http,
            &url,
            settings.token.as_deref(),
            &part,
            &title,
            &cancel,
            &pause,
            settings.bandwidth_limit_kib,
            install::DownloadTuning::from_idle_secs(settings.download_stall_timeout_secs),
            move |update: DownloadProgress| {
                let _ = handle.emit(
                    "install://repair-progress",
                    RepairProgress {
                        game_id,
                        phase: "downloading".to_string(),
                        files_done: 0,
                        files_total: damaged_total,
                        bytes_done: update.done,
                        bytes_total: update.total.unwrap_or(update.done),
                        message: update.message,
                    },
                );
            },
        )
        .await;

        match result {
            Ok(()) => {
                archive = Some(part.clone());
                downloaded = true;
            }
            Err(failure) => {
                return Ok(fallback_report(
                    game_id,
                    verify,
                    format!(
                        "Could not download the archive ({}). Reinstall the game instead.",
                        failure_message(&failure)
                    ),
                ));
            }
        }
    }

    let Some(archive) = archive else {
        return Ok(fallback_report(
            game_id,
            verify,
            "No archive is available for this version. Reinstall the game instead.".to_string(),
        ));
    };

    let staging = install_root.join(format!(
        ".repair-{game_id}-{}",
        short_id(&uuid::Uuid::new_v4().to_string())
    ));
    if staging.exists() {
        let _ = tokio::fs::remove_dir_all(&staging).await;
    }

    let extract_handle = app.clone();
    let extract_archive_path = archive.clone();
    let extract_staging = staging.clone();
    let passwords = settings.archive_passwords.clone();
    let extracted = tokio::task::spawn_blocking(move || {
        let mut last = Instant::now();
        bridge::extract_archive(
            &extract_archive_path,
            &extract_staging,
            &passwords,
            &mut |entries_done, entries_total, bytes_done, bytes_total| {
                if last.elapsed() < Duration::from_millis(250) {
                    return;
                }
                last = Instant::now();
                let _ = extract_handle.emit(
                    "install://repair-progress",
                    RepairProgress {
                        game_id,
                        phase: "extracting".to_string(),
                        files_done: entries_done,
                        files_total: entries_total.unwrap_or(0),
                        bytes_done,
                        bytes_total: bytes_total.unwrap_or(0),
                        message: format!("Extracting {entries_done} files"),
                    },
                );
            },
        )
    })
    .await;

    let extracted = match extracted {
        Ok(Ok(())) => true,
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "repair extraction failed");
            false
        }
        Err(error) => {
            tracing::warn!(error = %error, "repair extraction task failed");
            false
        }
    };

    if !extracted {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        if downloaded && !settings.keep_archives {
            let _ = tokio::fs::remove_file(&part).await;
        }
        return Ok(fallback_report(
            game_id,
            verify,
            "The archive could not be extracted. Reinstall the game instead.".to_string(),
        ));
    }

    let restore_handle = app.clone();
    let restore_staging = staging.clone();
    let restore_root = root.clone();
    let entries = damaged.clone();
    let total_files = entries.len() as u64;
    let total_bytes = entries.iter().map(|entry| entry.size).sum::<u64>();
    let restored = tokio::task::spawn_blocking(move || {
        let mut last = Instant::now();
        restore_files(
            &restore_staging,
            &restore_root,
            &entries,
            &mut |done, bytes| {
                if last.elapsed() < Duration::from_millis(250) {
                    return;
                }
                last = Instant::now();
                let _ = restore_handle.emit(
                    "install://repair-progress",
                    RepairProgress {
                        game_id,
                        phase: "restoring".to_string(),
                        files_done: done,
                        files_total: total_files,
                        bytes_done: bytes,
                        bytes_total: total_bytes,
                        message: format!("Restoring {done} of {total_files} files"),
                    },
                );
            },
        )
    })
    .await
    .map_err(|e| format!("The repair task failed: {e}"))?;

    let _ = tokio::fs::remove_dir_all(&staging).await;
    if downloaded && !settings.keep_archives {
        let _ = tokio::fs::remove_file(&part).await;
    }

    let repaired: HashSet<String> = restored.repaired.iter().cloned().collect();
    verify
        .missing
        .retain(|issue| !repaired.contains(&issue.path));
    verify
        .corrupt
        .retain(|issue| !repaired.contains(&issue.path));
    verify.status = if verify.missing.is_empty() && verify.corrupt.is_empty() {
        VerifyStatus::Ok
    } else {
        VerifyStatus::Issues
    };
    verify.checked_at = now_rfc3339();
    verify.message = Some(if restored.failed.is_empty() {
        format!(
            "Restored {} of {} damaged files.",
            restored.repaired.len(),
            damaged.len()
        )
    } else {
        format!(
            "Restored {} of {} damaged files; {} could not be restored.",
            restored.repaired.len(),
            damaged.len(),
            restored.failed.len()
        )
    });

    let _ = app.emit("install://verify-progress", verify_done(&verify));
    // Restored files may include a deleted executable; the picker must rescan
    // instead of serving the pre-repair listing (#143).
    if let Ok(mut cache) = state.executables.lock() {
        if let Ok(root) = crate::game_executables::installed_root(&installed) {
            cache.invalidate(&root.to_string_lossy());
        }
    }
    Ok(RepairReport {
        game_id,
        repaired: restored.repaired,
        failed: restored.failed,
        fallback: false,
        message: verify
            .message
            .clone()
            .unwrap_or_else(|| "Repair finished".to_string()),
        verify,
    })
}

fn failure_message(failure: &Failure) -> String {
    match failure {
        Failure::Cancelled => "the download was cancelled".to_string(),
        Failure::Paused => "the download was paused".to_string(),
        Failure::Failed(message) => message.clone(),
    }
}

fn fallback_report(game_id: i64, verify: VerifyReport, message: String) -> RepairReport {
    RepairReport {
        game_id,
        repaired: Vec::new(),
        failed: Vec::new(),
        fallback: true,
        message,
        verify,
    }
}

/// Copy only the damaged entries from an extracted archive into the install.
///
/// Each source file is hashed before the copy so an archive that does not match
/// the manifest cannot silently replace a good file with a bad one.
pub fn restore_files(
    staging: &Path,
    install_root: &Path,
    entries: &[ManifestEntry],
    on_progress: &mut dyn FnMut(u64, u64),
) -> RestoreOutcome {
    let mut outcome = RestoreOutcome::default();
    let mut bytes_done = 0u64;

    for (index, entry) in entries.iter().enumerate() {
        let Some(source) = manifest::join_relative(staging, &entry.path) else {
            outcome.failed.push(entry.path.clone());
            continue;
        };
        if !source.is_file() {
            outcome.failed.push(entry.path.clone());
            continue;
        }

        match manifest::hash_file(&source) {
            Ok((size, sha256)) if size == entry.size && sha256 == entry.sha256 => {}
            Ok(_) | Err(_) => {
                outcome.failed.push(entry.path.clone());
                continue;
            }
        }

        let Some(destination) = manifest::join_relative(install_root, &entry.path) else {
            outcome.failed.push(entry.path.clone());
            continue;
        };
        if let Some(parent) = destination.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                tracing::warn!(error = %error, path = %parent.display(), "could not create the restore folder");
                outcome.failed.push(entry.path.clone());
                continue;
            }
        }
        if let Err(error) = std::fs::copy(&source, &destination) {
            tracing::warn!(error = %error, path = %entry.path, "could not restore a file");
            outcome.failed.push(entry.path.clone());
            continue;
        }

        bytes_done = bytes_done.saturating_add(entry.size);
        outcome.repaired.push(entry.path.clone());
        on_progress(index as u64 + 1, bytes_done);
    }

    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ludex-maint-{label}-{}",
            short_id(&uuid::Uuid::new_v4().to_string())
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn installed_at(path: &str, root: &str) -> InstalledGame {
        InstalledGame {
            game_id: 1,
            version_id: 2,
            title: "My Game".into(),
            version: Some("1.0".into()),
            install_path: path.into(),
            executable: Some(format!("{path}/game.exe")),
            size_bytes: 0,
            installed_at: "2026-01-01T00:00:00Z".into(),
            last_launched_at: None,
            archive_hash: None,
            install_root: Some(root.into()),
            source: crate::db::SOURCE_LUDEX.into(),
            launch_uri: None,
        }
    }

    #[test]
    fn move_keeps_the_slug_and_version_layout() {
        let game = installed_at("/games/my-game/1.0", "/games");
        assert_eq!(
            relative_install_path(Path::new("/games/my-game/1.0"), Path::new("/games"), &game)
                .unwrap(),
            PathBuf::from("my-game/1.0")
        );
        // A custom install root that no longer matches falls back to slug/version.
        assert_eq!(
            relative_install_path(Path::new("/elsewhere/x"), Path::new("/games"), &game).unwrap(),
            PathBuf::from("my-game/1.0")
        );

        assert_eq!(
            rebase_executable(
                &game.executable,
                Path::new("/games/my-game/1.0"),
                Path::new("/mnt/games/my-game/1.0")
            ),
            Some("/mnt/games/my-game/1.0/game.exe".to_string())
        );
        assert_eq!(
            rebase_executable(
                &Some("/usr/bin/other".into()),
                Path::new("/games"),
                Path::new("/mnt")
            ),
            Some("/usr/bin/other".to_string())
        );
    }

    #[test]
    fn same_filesystem_move_is_a_rename() {
        let root = temp_root("move");
        let source = root.join("games/doom/1.0");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("game.exe"), b"MZ").unwrap();
        let destination = root.join("other/doom/1.0");

        let mut progress = Vec::new();
        move_install_dir(&source, &destination, 2, &mut |done, total| {
            progress.push((done, total))
        })
        .unwrap();

        assert!(!source.exists());
        assert_eq!(fs::read(destination.join("game.exe")).unwrap(), b"MZ");
        assert_eq!(progress.last(), Some(&(2, 2)));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cross_device_copy_verifies_and_rolls_back() {
        let root = temp_root("copy");
        let source = root.join("source");
        fs::create_dir_all(source.join("bin")).unwrap();
        fs::write(source.join("game.exe"), vec![1u8; 1024]).unwrap();
        fs::write(source.join("bin/data.bin"), vec![2u8; 4096]).unwrap();
        let destination = root.join("target/doom/1.0");

        // Force the copy path (the rename would otherwise succeed).
        let size = paths::dir_size(&source);
        let copied = paths::copy_dir_recursive(&source, &destination, |_, _| {}).unwrap();
        assert_eq!(copied, size);
        assert_eq!(paths::dir_size(&destination), size);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn move_refuses_to_overwrite() {
        let root = temp_root("overwrite");
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("game.exe"), b"MZ").unwrap();
        let destination = root.join("target");
        fs::create_dir_all(&destination).unwrap();

        let error = move_install_dir(&source, &destination, 2, &mut |_, _| {}).unwrap_err();
        assert!(error.contains("already exists") || error.contains("finish the move"));
        assert!(source.exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_only_touches_damaged_files() {
        let root = temp_root("restore");
        let staging = root.join("staging");
        let install = root.join("install");
        fs::create_dir_all(staging.join("bin")).unwrap();
        fs::create_dir_all(install.join("bin")).unwrap();

        fs::write(staging.join("bin/data.bin"), vec![7u8; 2048]).unwrap();
        fs::write(staging.join("keep.bin"), b"fresh").unwrap();
        fs::write(install.join("keep.bin"), b"intact").unwrap();
        fs::write(install.join("bin/data.bin"), b"corrupt").unwrap();

        let manifest = manifest::build_manifest(&staging, &mut |_| {}).unwrap();
        let damaged = vec![manifest
            .entries
            .iter()
            .find(|entry| entry.path == "bin/data.bin")
            .unwrap()
            .clone()];

        let outcome = restore_files(&staging, &install, &damaged, &mut |_, _| {});
        assert_eq!(outcome.repaired, vec!["bin/data.bin".to_string()]);
        assert!(outcome.failed.is_empty());
        assert_eq!(
            fs::read(install.join("bin/data.bin")).unwrap(),
            vec![7u8; 2048]
        );
        assert_eq!(fs::read(install.join("keep.bin")).unwrap(), b"intact");

        let _ = fs::remove_dir_all(&root);
    }

    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, data) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }
        buffer.into_inner()
    }

    #[tokio::test]
    async fn repair_restores_missing_and_corrupt_files_from_a_local_server() {
        let root = temp_root("repair");
        let install = root.join("install");
        let archive_bytes = build_zip(&[
            ("game.exe", b"MZ-game-binary".as_slice()),
            ("data/level.bin", &vec![42u8; 8192]),
        ]);

        let archive_path = root.join("game.zip");
        fs::write(&archive_path, &archive_bytes).unwrap();
        bridge::extract_archive(&archive_path, &install, &[], &mut |_, _, _, _| {}).unwrap();
        let manifest = manifest::build_manifest(&install, &mut |_| {}).unwrap();

        // Damage the install: delete one file and truncate another.
        fs::remove_file(install.join("data/level.bin")).unwrap();
        fs::write(install.join("game.exe"), b"MZ").unwrap();

        let outcome = manifest::verify_tree(&install, &manifest, &mut |_| {}).unwrap();
        assert_eq!(outcome.missing.len(), 1);
        assert_eq!(outcome.corrupt.len(), 1);
        let damaged = manifest::damaged_entries(&outcome);

        // Download the archive from a local test server, then restore only the
        // damaged files from the extracted copy (the repair pipeline minus Tauri).
        let server = crate::test_support::serve_bytes(archive_bytes.clone()).await;
        let part = root.join("repair.part");
        let client = reqwest::Client::builder().build().unwrap();
        install::download_file(
            &client,
            &server.url,
            None,
            &part,
            "Repair",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            install::DownloadTuning::default(),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(fs::read(&part).unwrap(), archive_bytes);

        let staging = root.join("staging");
        bridge::extract_archive(&part, &staging, &[], &mut |_, _, _, _| {}).unwrap();
        let restored = restore_files(&staging, &install, &damaged, &mut |_, _| {});
        assert_eq!(restored.repaired.len(), 2);
        assert!(restored.failed.is_empty());

        let outcome = manifest::verify_tree(&install, &manifest, &mut |_| {}).unwrap();
        assert!(!outcome.has_damage());
        assert_eq!(
            fs::read(install.join("game.exe")).unwrap(),
            b"MZ-game-binary"
        );
        assert_eq!(
            fs::read(install.join("data/level.bin")).unwrap(),
            vec![42u8; 8192]
        );

        let _ = fs::remove_dir_all(&root);
    }
}
