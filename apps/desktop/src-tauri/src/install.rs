//! Install pipeline: resolve the archive, download with resume, extract to a
//! staging folder (or copy a single-file game), detect the executable, move
//! into place and index the result.
//!
//! Jobs are persisted in SQLite so an interrupted download stays visible after
//! a restart and the next attempt resumes the `.part` file with HTTP Range.
//! Only one job runs at a time by default; the rest wait in the queued phase.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::db::{self, InstalledGame};
use crate::paths;
use crate::settings::Mode;
use crate::state::{AppState, JobEntry};
use crate::util::{non_empty, now_rfc3339, short_id};

const INTERRUPTED_MESSAGE: &str =
    "Ludex closed before this install finished. Install again to resume.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallPhase {
    Queued,
    Downloading,
    Extracting,
    Finalizing,
    Done,
    Cancelled,
    Error,
}

impl InstallPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallPhase::Queued => "queued",
            InstallPhase::Downloading => "downloading",
            InstallPhase::Extracting => "extracting",
            InstallPhase::Finalizing => "finalizing",
            InstallPhase::Done => "done",
            InstallPhase::Cancelled => "cancelled",
            InstallPhase::Error => "error",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "queued" => InstallPhase::Queued,
            "downloading" => InstallPhase::Downloading,
            "extracting" => InstallPhase::Extracting,
            "finalizing" => InstallPhase::Finalizing,
            "done" => InstallPhase::Done,
            "cancelled" => InstallPhase::Cancelled,
            _ => InstallPhase::Error,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            InstallPhase::Done | InstallPhase::Cancelled | InstallPhase::Error
        )
    }
}

/// Mirrors `InstallJob` from `docs/CONTRACT.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallJob {
    pub job_id: String,
    pub game_id: i64,
    pub version_id: i64,
    pub phase: InstallPhase,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    pub speed_bps: u64,
    pub message: Option<String>,
    pub install_path: Option<String>,
}

impl InstallJob {
    /// Rebuild a job from its persisted row (after an app restart).
    pub fn from_record(record: db::InstallJobRecord) -> Self {
        let phase = InstallPhase::parse(&record.phase);
        let message = record.error.clone().or_else(|| {
            Some(match phase {
                InstallPhase::Done => "Installed".to_string(),
                InstallPhase::Cancelled => "Install cancelled".to_string(),
                InstallPhase::Error => INTERRUPTED_MESSAGE.to_string(),
                _ => "Interrupted".to_string(),
            })
        });
        Self {
            job_id: record.job_id,
            game_id: record.game_id,
            version_id: record.version_id,
            phase,
            bytes_done: record.bytes_done.max(0) as u64,
            bytes_total: record.bytes_total.map(|value| value.max(0) as u64),
            speed_bps: record.speed_bps.max(0) as u64,
            message,
            install_path: record.install_path,
        }
    }
}

/// Everything `install_game` needs, captured when the job starts.
#[derive(Debug, Clone)]
pub struct InstallParams {
    pub game_id: i64,
    pub version_id: i64,
    pub server_url: String,
    pub token: Option<String>,
    pub title: String,
    pub version: Option<String>,
    pub executable: Option<String>,
    pub install_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallErrorEvent {
    job_id: String,
    message: String,
}

#[derive(Debug)]
enum Failure {
    Cancelled,
    Failed(String),
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure::Failed(message)
    }
}

#[derive(Debug, Default)]
struct JobPaths {
    staging: Option<PathBuf>,
    part: Option<PathBuf>,
}

pub fn begin(app: &AppHandle, params: InstallParams) -> Result<InstallJob, String> {
    let state = app.state::<AppState>();
    state.prune_finished_jobs();

    let settings = state.settings_snapshot();
    let mut params = params;
    // Fall back to the stored server credentials when the UI passes blanks.
    if params.server_url.trim().is_empty() {
        params.server_url = settings.server_url.clone();
    }
    if params.token.is_none() {
        params.token = settings.token.clone();
    }

    let install_root = params
        .install_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| settings.install_path());
    let slug = paths::slug_for_title(&params.title);
    let version_label = paths::version_label(params.version.as_deref());
    let planned_install = install_root.join(&slug).join(&version_label);
    let planned_part = settings
        .download_path()
        .join(format!(".{}-{}.part", params.game_id, params.version_id));

    let job_id = uuid::Uuid::new_v4().to_string();
    let game_id = params.game_id;
    let version_id = params.version_id;
    let job = InstallJob {
        job_id: job_id.clone(),
        game_id,
        version_id,
        phase: InstallPhase::Queued,
        bytes_done: 0,
        bytes_total: None,
        speed_bps: 0,
        message: Some("Queued".to_string()),
        install_path: None,
    };

    let entry = Arc::new(JobEntry {
        params,
        cancel: CancellationToken::new(),
        job: std::sync::Mutex::new(job.clone()),
        created_at: now_rfc3339(),
        part_path: std::sync::Mutex::new(Some(planned_part.to_string_lossy().to_string())),
        install_path: std::sync::Mutex::new(Some(planned_install.to_string_lossy().to_string())),
    });

    {
        // Check and insert under one lock so two concurrent installs for the
        // same version cannot both start.
        let mut jobs = state
            .jobs
            .lock()
            .map_err(|_| "The install queue is unavailable; restart Ludex.".to_string())?;
        if let Some(existing) = jobs.values().find(|entry| {
            entry.params.game_id == game_id
                && entry.params.version_id == version_id
                && !entry.snapshot().phase.is_terminal()
        }) {
            return Ok(existing.snapshot());
        }
        jobs.insert(job_id.clone(), Arc::clone(&entry));
    }

    persist(app, &entry);

    let handle = app.clone();
    let spawned_id = job_id.clone();
    tauri::async_runtime::spawn(async move {
        run(handle, spawned_id).await;
    });

    Ok(job)
}

pub fn job(app: &AppHandle, job_id: &str) -> Result<InstallJob, String> {
    let state = app.state::<AppState>();
    let entry = {
        let jobs = state
            .jobs
            .lock()
            .map_err(|_| "The install queue is unavailable; restart Ludex.".to_string())?;
        jobs.get(job_id).cloned()
    };
    entry
        .map(|entry| entry.snapshot())
        .ok_or_else(|| format!("Unknown install job: {job_id}"))
}

/// All known jobs, newest first (persisted rows, including interrupted jobs
/// from a previous run). Extension to the contract used by the UI to discover
/// resumable installs after a restart.
pub fn list(app: &AppHandle) -> Result<Vec<InstallJob>, String> {
    let state = app.state::<AppState>();
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    let rows = db::recent_install_jobs(&connection, 50)?;
    Ok(rows.into_iter().map(InstallJob::from_record).collect())
}

pub fn cancel(app: &AppHandle, job_id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let entry = {
        let jobs = state
            .jobs
            .lock()
            .map_err(|_| "The install queue is unavailable; restart Ludex.".to_string())?;
        jobs.get(job_id).cloned()
    };
    match entry {
        Some(entry) => {
            entry.cancel.cancel();
            Ok(())
        }
        None => Err(format!("Unknown install job: {job_id}")),
    }
}

fn planned_paths(state: &AppState, entry: &JobEntry) -> JobPaths {
    let settings = state.settings_snapshot();
    let install_root = entry
        .params
        .install_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| settings.install_path());
    let slug = paths::slug_for_title(&entry.params.title);
    let job_id = entry.snapshot().job_id;
    JobPaths {
        staging: Some(install_root.join(format!(".{slug}.staging-{}", short_id(&job_id)))),
        part: Some(settings.download_path().join(format!(
            ".{}-{}.part",
            entry.params.game_id, entry.params.version_id
        ))),
    }
}

async fn run(app: AppHandle, job_id: String) {
    let state = app.state::<AppState>();
    let entry = {
        match state.jobs.lock() {
            Ok(jobs) => jobs.get(&job_id).cloned(),
            Err(_) => None,
        }
    };
    let Some(entry) = entry else {
        tracing::warn!(job_id = %job_id, "install job disappeared before it started");
        return;
    };

    let mut job_paths = planned_paths(&state, &entry);
    entry.set_part_path(
        job_paths
            .part
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
    );

    // One install at a time; queued jobs wait here.
    let slots = Arc::clone(&state.install_slots);
    let permit = tokio::select! {
        biased;
        _ = entry.cancel.cancelled() => {
            finalize_cancelled(&app, &entry, &job_paths).await;
            return;
        }
        permit = slots.acquire_owned() => match permit {
            Ok(permit) => permit,
            Err(_) => {
                finalize_error(&app, &entry, "The install queue is unavailable. Restart Ludex.".to_string());
                return;
            }
        },
    };

    if entry.cancel.is_cancelled() {
        drop(permit);
        finalize_cancelled(&app, &entry, &job_paths).await;
        return;
    }

    entry.update(|job| {
        job.phase = InstallPhase::Queued;
        job.message = Some("Starting install".to_string());
    });
    persist(&app, &entry);

    let result = execute(&app, &entry, &mut job_paths).await;
    drop(permit);

    if result.is_err() {
        if let Some(staging) = job_paths.staging.as_deref() {
            let _ = tokio::fs::remove_dir_all(staging).await;
        }
        if matches!(&result, Err(Failure::Cancelled)) {
            if let Some(part) = job_paths.part.as_deref() {
                let _ = tokio::fs::remove_file(part).await;
            }
        }
    }

    match result {
        Ok(install_path) => {
            entry.update(|job| {
                job.phase = InstallPhase::Done;
                job.speed_bps = 0;
                job.bytes_done = job.bytes_total.unwrap_or(job.bytes_done);
                job.message = Some("Installed".to_string());
                job.install_path = Some(install_path.to_string_lossy().to_string());
            });
            persist(&app, &entry);
            let job = entry.snapshot();
            let _ = app.emit("install://progress", &job);
            let _ = app.emit("install://done", &job);
            tracing::info!(job_id = %job_id, game_id = job.game_id, path = ?job.install_path, "install complete");
        }
        Err(Failure::Cancelled) => {
            finalize_cancelled(&app, &entry, &job_paths).await;
        }
        Err(Failure::Failed(message)) => {
            finalize_error(&app, &entry, message);
        }
    }
}

async fn finalize_cancelled(app: &AppHandle, entry: &JobEntry, job_paths: &JobPaths) {
    if let Some(staging) = job_paths.staging.as_deref() {
        let _ = tokio::fs::remove_dir_all(staging).await;
    }
    if let Some(part) = job_paths.part.as_deref() {
        let _ = tokio::fs::remove_file(part).await;
    }
    entry.set_part_path(None);
    entry.update(|job| {
        job.phase = InstallPhase::Cancelled;
        job.speed_bps = 0;
        job.message = Some("Install cancelled".to_string());
    });
    persist(app, entry);
    let _ = app.emit("install://progress", entry.snapshot());
    tracing::info!(job_id = %entry.snapshot().job_id, "install cancelled");
}

fn finalize_error(app: &AppHandle, entry: &JobEntry, message: String) {
    entry.update(|job| {
        job.phase = InstallPhase::Error;
        job.speed_bps = 0;
        job.message = Some(message.clone());
    });
    persist(app, entry);
    let job = entry.snapshot();
    let _ = app.emit("install://progress", &job);
    let _ = app.emit(
        "install://error",
        InstallErrorEvent {
            job_id: job.job_id.clone(),
            message: message.clone(),
        },
    );
    tracing::warn!(job_id = %job.job_id, error = %message, "install failed");
}

fn persist(app: &AppHandle, entry: &JobEntry) {
    let state = app.state::<AppState>();
    let job = entry.snapshot();
    let (part_path, install_path) = entry.stored_paths();
    let record = db::InstallJobRecord {
        job_id: job.job_id.clone(),
        game_id: job.game_id,
        version_id: job.version_id,
        phase: job.phase.as_str().to_string(),
        bytes_done: job.bytes_done as i64,
        bytes_total: job.bytes_total.map(|value| value as i64),
        speed_bps: job.speed_bps as i64,
        part_path,
        install_path,
        error: if job.phase == InstallPhase::Error {
            job.message.clone()
        } else {
            None
        },
        created_at: entry.created_at.clone(),
        updated_at: now_rfc3339(),
    };

    if let Ok(connection) = state.db.lock() {
        if let Err(error) = db::upsert_install_job(&connection, &record) {
            tracing::warn!(error = %error, "could not persist the install job");
        }
    };
}

async fn execute(
    app: &AppHandle,
    entry: &Arc<JobEntry>,
    job_paths: &mut JobPaths,
) -> Result<PathBuf, Failure> {
    let state = app.state::<AppState>();
    let params = entry.params.clone();
    let settings = state.settings_snapshot();

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
    job_paths.staging = Some(staging.clone());
    if staging.exists() {
        tokio::fs::remove_dir_all(&staging).await.map_err(|e| {
            Failure::Failed(format!(
                "Could not clear the staging folder {}: {e}",
                staging.display()
            ))
        })?;
    }

    check_cancel(&entry.cancel)?;

    // 1. Local library shortcut: ask the embedded server for the archive path.
    let mut archive: Option<PathBuf> = None;
    let mut downloaded: Option<PathBuf> = None;

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
        archive = Some(local);
    }

    // 2. Otherwise download with resume.
    if archive.is_none() {
        let part = settings
            .download_path()
            .join(format!(".{}-{}.part", params.game_id, params.version_id));
        job_paths.part = Some(part.clone());
        entry.set_part_path(Some(part.to_string_lossy().to_string()));
        download(app, entry, &params, &part).await?;
        archive = Some(part.clone());
        downloaded = Some(part);
    }

    check_cancel(&entry.cancel)?;
    let archive = archive
        .ok_or_else(|| Failure::Failed("No archive was available for this install.".to_string()))?;
    let archive_size = std::fs::metadata(&archive)
        .map(|metadata| metadata.len())
        .ok();

    let mut kept_name: Option<String> = None;
    let relative_executable: Option<PathBuf> = if looks_like_single_file(&archive) {
        // 3a. Single-file game: no archive to extract, copy it into the
        // install folder and use it as the executable.
        let file_name = single_file_name(&archive, &slug);
        kept_name = Some(file_name.clone());
        set_progress(
            app,
            entry,
            InstallPhase::Finalizing,
            0,
            archive_size,
            0,
            Some("Installing game file".to_string()),
        );

        tokio::fs::create_dir_all(&staging).await.map_err(|e| {
            Failure::Failed(format!(
                "Could not create the staging folder {}: {e}",
                staging.display()
            ))
        })?;
        let destination = paths::safe_child(&staging, &file_name)?;

        let source = archive.clone();
        let target = destination.clone();
        let entry_for_copy = Arc::clone(entry);
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
                    job.message = Some("Installing game file".to_string());
                });
                let _ = app_for_copy.emit("install://progress", entry_for_copy.snapshot());
            })
        })
        .await
        .map_err(|e| Failure::Failed(format!("The file copy task failed: {e}")))?
        .map_err(|e| Failure::Failed(format!("Could not copy the game file: {e}")))?;

        Some(PathBuf::from(&file_name))
    } else {
        // 3b. Extract the archive into the staging folder.
        set_progress(
            app,
            entry,
            InstallPhase::Extracting,
            0,
            archive_size,
            0,
            Some("Extracting archive".to_string()),
        );

        let staging_for_extract = staging.clone();
        let archive_for_extract = archive.clone();
        let passwords = settings.archive_passwords.clone();
        let entry_for_extract = Arc::clone(entry);
        let app_for_extract = app.clone();
        let extraction = tokio::task::spawn_blocking(move || {
            let mut last_emit: Option<Instant> = None;
            crate::bridge::extract_archive(
                &archive_for_extract,
                &staging_for_extract,
                &passwords,
                &mut |entries_done, entries_total, bytes_done, bytes_total| {
                    let now = Instant::now();
                    let ready = last_emit
                        .map(|last| now.duration_since(last) >= Duration::from_millis(250))
                        .unwrap_or(true);
                    if !ready {
                        return;
                    }
                    last_emit = Some(now);

                    let (done, total) = match (bytes_done, bytes_total) {
                        (done, Some(total)) if total > 0 => (done, Some(total)),
                        _ => (entries_done, entries_total),
                    };
                    let message = match entries_total {
                        Some(total) => format!("Extracting {entries_done}/{total} files"),
                        None => "Extracting archive".to_string(),
                    };
                    entry_for_extract.update(|job| {
                        job.phase = InstallPhase::Extracting;
                        job.bytes_done = done;
                        job.bytes_total = total;
                        job.speed_bps = 0;
                        job.message = Some(message);
                    });
                    let _ =
                        app_for_extract.emit("install://progress", entry_for_extract.snapshot());
                },
            )
        })
        .await
        .map_err(|e| Failure::Failed(format!("The extraction task failed: {e}")))?;
        extraction.map_err(Failure::Failed)?;

        check_cancel(&entry.cancel)?;

        // 4. Locate the executable inside the staging folder. A missing
        // executable is not fatal: the install still completes with
        // `executable: null` and the UI can open the folder.
        set_progress(
            app,
            entry,
            InstallPhase::Finalizing,
            0,
            None,
            0,
            Some("Locating the game executable".to_string()),
        );

        let staging_for_exe = staging.clone();
        let title = params.title.clone();
        let requested_exe = params.executable.clone();
        tokio::task::spawn_blocking(move || {
            resolve_executable(&staging_for_exe, requested_exe.as_deref(), &title)
        })
        .await
        .map_err(|e| Failure::Failed(format!("Could not inspect the extracted files: {e}")))??
    };

    check_cancel(&entry.cancel)?;

    // 5. Move staging into place (rename, or copy across devices).
    let staging_for_move = staging.clone();
    let final_for_move = final_path.clone();
    let entry_for_move = Arc::clone(entry);
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
                job.message = Some("Installing files".to_string());
            });
            let _ = app_for_move.emit("install://progress", entry_for_move.snapshot());
        })
    })
    .await
    .map_err(|e| Failure::Failed(format!("The install could not be finalised: {e}")))?
    .map_err(Failure::Failed)?;

    let executable_path = relative_executable.map(|relative| final_path.join(relative));

    // 6. Index the install.
    let size_bytes = {
        let path = final_path.clone();
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
        install_path: final_path.to_string_lossy().to_string(),
        executable: executable_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        size_bytes: size_bytes as i64,
        installed_at: now_rfc3339(),
        last_launched_at: previous_last_launch,
        install_root: Some(install_root.to_string_lossy().to_string()),
    };

    {
        let connection = state.db.lock().map_err(|_| {
            Failure::Failed("The local database is unavailable; restart Ludex.".to_string())
        })?;
        db::upsert_installed(&connection, &installed).map_err(Failure::Failed)?;
    }

    // 7. Archive cleanup.
    if let Some(part) = downloaded {
        if settings.keep_archives {
            let name = kept_name.unwrap_or_else(|| format!("{slug}-{version_label}.archive"));
            let kept = settings.download_path().join(name);
            if let Err(error) = tokio::fs::rename(&part, &kept).await {
                tracing::warn!(error = %error, "could not keep the downloaded archive");
            }
        } else if let Err(error) = tokio::fs::remove_file(&part).await {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(error = %error, "could not delete the downloaded archive");
            }
        }
    }

    entry.set_part_path(None);
    Ok(final_path)
}

fn check_cancel(cancel: &CancellationToken) -> Result<(), Failure> {
    if cancel.is_cancelled() {
        Err(Failure::Cancelled)
    } else {
        Ok(())
    }
}

fn set_progress(
    app: &AppHandle,
    entry: &JobEntry,
    phase: InstallPhase,
    bytes_done: u64,
    bytes_total: Option<u64>,
    speed_bps: u64,
    message: Option<String>,
) {
    entry.update(|job| {
        job.phase = phase;
        job.bytes_done = bytes_done;
        job.bytes_total = bytes_total;
        job.speed_bps = speed_bps;
        job.message = message;
    });
    persist(app, entry);
    let _ = app.emit("install://progress", entry.snapshot());
}

fn absolute_path(path: PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .map_err(|e| format!("Could not resolve a relative install path: {e}"))
    }
}

async fn local_archive_path(state: &AppState, params: &InstallParams) -> Option<PathBuf> {
    let base = params.server_url.trim_end_matches('/');
    if base.is_empty() || !is_local_server(state, base) {
        return None;
    }

    #[derive(Deserialize)]
    struct LocalPath {
        path: String,
    }

    let url = format!(
        "{base}/api/games/{}/versions/{}/local-path",
        params.game_id, params.version_id
    );
    let mut request = state.http.get(url).timeout(Duration::from_secs(15));
    if let Some(token) = non_empty(params.token.as_deref()) {
        request = request.bearer_auth(token);
    }

    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let payload: LocalPath = response.json().await.ok()?;
    let path = PathBuf::from(payload.path);
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

fn is_local_server(state: &AppState, base: &str) -> bool {
    let settings = state.settings_snapshot();
    if settings.mode != Mode::Local {
        return false;
    }
    let known = settings.server_url.trim_end_matches('/');
    if !known.is_empty() && known == base {
        return true;
    }
    is_loopback_url(base)
}

fn is_loopback_url(url: &str) -> bool {
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let authority = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

async fn download(
    app: &AppHandle,
    entry: &JobEntry,
    params: &InstallParams,
    part: &Path,
) -> Result<(), Failure> {
    let state = app.state::<AppState>();
    let client = state.http.clone();
    let base = params.server_url.trim_end_matches('/');
    if base.is_empty() {
        return Err(Failure::Failed(
            "No server URL was provided for this download.".to_string(),
        ));
    }

    if let Some(parent) = part.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            Failure::Failed(format!(
                "Could not create the download folder {}: {e}",
                parent.display()
            ))
        })?;
    }

    let url = format!(
        "{base}/api/games/{}/download?versionId={}",
        params.game_id, params.version_id
    );
    let cancel = entry.cancel.clone();
    let mut existing = tokio::fs::metadata(part)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    let mut restarted = false;
    let mut response = loop {
        let mut request = client.get(&url);
        request = request.header(reqwest::header::ACCEPT_ENCODING, "identity");
        if let Some(token) = non_empty(params.token.as_deref()) {
            request = request.bearer_auth(token);
        }
        if existing > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={existing}-"));
        }

        let response = request.send().await.map_err(|e| {
            Failure::Failed(format!(
                "Could not reach the server to download \"{}\": {e}",
                params.title
            ))
        })?;
        let status = response.status();

        if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            // `bytes */total` tells us whether the .part file is already
            // complete. This is the common case after a restart once the
            // download finished but extraction failed.
            let total = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_star_total);
            if existing > 0 && total == Some(existing) {
                set_progress(
                    app,
                    entry,
                    InstallPhase::Downloading,
                    existing,
                    Some(existing),
                    0,
                    Some("Download already complete".to_string()),
                );
                return Ok(());
            }
            if existing > 0 && !restarted {
                restarted = true;
                let _ = tokio::fs::remove_file(part).await;
                existing = 0;
                continue;
            }
            return Err(Failure::Failed(
                "The server rejected the resume request (HTTP 416). Delete the partial download and retry."
                    .to_string(),
            ));
        }

        if !status.is_success() {
            let code = status.as_u16();
            let body = response.text().await.unwrap_or_default();
            let snippet = body.trim().chars().take(300).collect::<String>();
            let hint = match code {
                401 | 403 => " Check that you are signed in to the server.",
                404 => " The game version was not found on the server.",
                _ => "",
            };
            let detail = if snippet.is_empty() {
                String::new()
            } else {
                format!(": {snippet}")
            };
            return Err(Failure::Failed(format!(
                "Download failed with HTTP {code}{hint}{detail}"
            )));
        }

        break response;
    };

    let status = response.status();
    let append = status == reqwest::StatusCode::PARTIAL_CONTENT && existing > 0;
    if !append {
        existing = 0;
    }
    let mut done = existing;

    let content_length = response.content_length();
    let content_range_total = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_content_range_total);
    let total = if append {
        content_range_total.or_else(|| content_length.map(|length| length + existing))
    } else {
        content_length
    };

    set_progress(
        app,
        entry,
        InstallPhase::Downloading,
        done,
        total,
        0,
        Some(format!("Downloading \"{}\"", params.title)),
    );

    let file = if append {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(part)
            .await
            .map_err(|e| {
                Failure::Failed(format!(
                    "Could not open the partial download {}: {e}",
                    part.display()
                ))
            })?
    } else {
        tokio::fs::File::create(part).await.map_err(|e| {
            Failure::Failed(format!(
                "Could not create the download file {}: {e}",
                part.display()
            ))
        })?
    };

    let mut writer = tokio::io::BufWriter::with_capacity(1024 * 1024, file);
    let mut last_emit = Instant::now();
    let mut last_instant = Instant::now();
    let mut last_bytes = done;

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                drop(writer);
                let _ = tokio::fs::remove_file(part).await;
                return Err(Failure::Cancelled);
            }
            chunk = response.chunk() => {
                match chunk {
                    Ok(Some(bytes)) => {
                        writer.write_all(&bytes).await.map_err(|e| {
                            Failure::Failed(format!(
                                "Could not write the download to {}: {e}",
                                part.display()
                            ))
                        })?;
                        done = done.saturating_add(bytes.len() as u64);
                        if last_emit.elapsed() >= Duration::from_millis(250) {
                            let elapsed = last_instant.elapsed().as_secs_f64().max(0.001);
                            let speed = ((done.saturating_sub(last_bytes)) as f64 / elapsed) as u64;
                            set_progress(
                                app,
                                entry,
                                InstallPhase::Downloading,
                                done,
                                total,
                                speed,
                                Some(format!("Downloading \"{}\"", params.title)),
                            );
                            last_emit = Instant::now();
                            last_instant = Instant::now();
                            last_bytes = done;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = writer.flush().await;
                        return Err(Failure::Failed(format!(
                            "The download was interrupted after {done} bytes: {error}. Retry the install to resume."
                        )));
                    }
                }
            }
        }
    }

    writer.flush().await.map_err(|e| {
        Failure::Failed(format!(
            "Could not flush the download to {}: {e}",
            part.display()
        ))
    })?;
    let file = writer.into_inner();
    file.sync_all().await.map_err(|e| {
        Failure::Failed(format!(
            "Could not flush the download to disk ({}): {e}",
            part.display()
        ))
    })?;

    set_progress(
        app,
        entry,
        InstallPhase::Downloading,
        done,
        Some(total.unwrap_or(done)),
        0,
        Some("Download complete".to_string()),
    );

    Ok(())
}

fn parse_content_range_total(value: &str) -> Option<u64> {
    value.rsplit('/').next()?.trim().parse().ok()
}

/// Parse `bytes */1234` from a 416 response.
fn parse_star_total(value: &str) -> Option<u64> {
    let total = value.rsplit('/').next()?.trim();
    if total == "*" {
        None
    } else {
        total.parse().ok()
    }
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

/// Resolve the executable to launch from an extracted tree, as a path relative
/// to the install folder. `Ok(None)` means no executable was found; the
/// install still succeeds with `executable: null`.
fn resolve_executable(
    root: &Path,
    requested: Option<&str>,
    title: &str,
) -> Result<Option<PathBuf>, String> {
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
            return Err(format!(
                "The configured executable does not exist after extraction: {}",
                full.display()
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
        return Ok(Some(relative));
    }

    Ok(
        crate::bridge::pick_executable(root, title).and_then(|path| {
            let absolute = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            let canonical = absolute.canonicalize().ok()?;
            canonical
                .strip_prefix(&root_canonical)
                .ok()
                .map(Path::to_path_buf)
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range_parsing() {
        assert_eq!(parse_content_range_total("bytes 100-199/200"), Some(200));
        assert_eq!(parse_content_range_total("bytes 0-0/*"), None);
        assert_eq!(parse_star_total("bytes */4096"), Some(4096));
        assert_eq!(parse_star_total("bytes */"), None);
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_url("http://127.0.0.1:4321"));
        assert!(is_loopback_url("http://localhost:1234/api"));
        assert!(is_loopback_url("http://[::1]:9000"));
        assert!(!is_loopback_url("https://ludex.example.com"));
        assert!(!is_loopback_url("http://192.168.1.10:8080"));
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

    #[test]
    fn phase_roundtrip() {
        for phase in [
            InstallPhase::Queued,
            InstallPhase::Downloading,
            InstallPhase::Extracting,
            InstallPhase::Finalizing,
            InstallPhase::Done,
            InstallPhase::Cancelled,
            InstallPhase::Error,
        ] {
            assert_eq!(InstallPhase::parse(phase.as_str()), phase);
        }
        assert!(InstallPhase::Done.is_terminal());
        assert!(!InstallPhase::Downloading.is_terminal());
    }
}
