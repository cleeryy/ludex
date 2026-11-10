//! Install pipeline: resolve the archive, download with resume, extract to a
//! staging folder (or copy a single-file game), detect the executable, move
//! into place and index the result.
//!
//! Jobs are persisted in SQLite so an interrupted download stays visible after
//! a restart and the next attempt resumes the `.part` file with HTTP Range.
//! Only one job runs at a time by default; the rest wait in the queued phase.

use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};
use tokio_stream::StreamExt;
use tokio_util::io::StreamReader;
use tokio_util::sync::CancellationToken;

use crate::db::{self, InstallManifestRecord, InstalledGame};
use crate::manifest;
use crate::paths;
use crate::settings::{self, LocalSettings, Mode};
use crate::state::{AppState, JobEntry};
use crate::util::{human_bytes, non_empty, now_rfc3339, short_id};

const INTERRUPTED_MESSAGE: &str =
    "Ludex closed before this install finished. Install again to resume.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallPhase {
    Queued,
    Downloading,
    Extracting,
    Finalizing,
    Paused,
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
            InstallPhase::Paused => "paused",
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
            "paused" => InstallPhase::Paused,
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
    /// True when this job used the non-resumable tar stream (#114). The UI
    /// offers "Retry as download" instead of "Resume" for a failed stream job.
    pub stream: bool,
}

impl InstallJob {
    /// Rebuild a job from its persisted row (after an app restart).
    pub fn from_record(record: db::InstallJobRecord) -> Self {
        let phase = InstallPhase::parse(&record.phase);
        let message = record.error.clone().or_else(|| {
            Some(match phase {
                InstallPhase::Done => "Installed".to_string(),
                InstallPhase::Cancelled => "Install cancelled".to_string(),
                InstallPhase::Paused => "Paused".to_string(),
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
            stream: record.stream,
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
    pub download_dir: Option<String>,
    pub stream: Option<bool>,
    /// Previous install folder to delete once an update is verified. The
    /// previous files stay untouched until the new install is in place.
    pub replace_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallErrorEvent {
    job_id: String,
    message: String,
}

#[derive(Debug)]
pub(crate) enum Failure {
    Cancelled,
    /// The user paused the job: the `.part` file is kept for a Range resume.
    Paused,
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
    let download_root =
        settings::resolve_download_dir(&settings, &install_root, params.download_dir.as_deref());

    let game_id = params.game_id;
    let version_id = params.version_id;

    // Resume the previous `.part` even if the download folder changed since
    // that attempt; fall back to the configured/derived download dir.
    let stored_part = {
        let connection = state.db.lock().ok();
        connection
            .and_then(|connection| {
                db::latest_install_job_for_version(&connection, game_id, version_id)
                    .ok()
                    .flatten()
            })
            .and_then(|record| record.part_path)
    };
    let planned_part = match stored_part.map(PathBuf::from) {
        Some(path) if path.is_file() => path,
        _ => download_root.join(format!(".{game_id}-{version_id}.part")),
    };

    let job_id = uuid::Uuid::new_v4().to_string();
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
        stream: stream_requested(params.stream, &settings, &params.server_url),
    };

    let entry = Arc::new(JobEntry {
        params,
        cancel: CancellationToken::new(),
        pause: std::sync::Mutex::new(CancellationToken::new()),
        running: AtomicBool::new(false),
        job: std::sync::Mutex::new(job.clone()),
        created_at: now_rfc3339(),
        part_path: std::sync::Mutex::new(Some(planned_part.to_string_lossy().to_string())),
        install_path: std::sync::Mutex::new(Some(planned_install.to_string_lossy().to_string())),
    });

    {
        // Check and insert under one lock so two concurrent installs for the
        // same version cannot both start.
        let existing = {
            let mut jobs = state
                .jobs
                .lock()
                .map_err(|_| "The install queue is unavailable; restart Ludex.".to_string())?;
            match jobs.values().find(|entry| {
                entry.params.game_id == game_id
                    && entry.params.version_id == version_id
                    && !entry.snapshot().phase.is_terminal()
            }) {
                Some(existing) => Some(Arc::clone(existing)),
                None => {
                    jobs.insert(job_id.clone(), Arc::clone(&entry));
                    None
                }
            }
        };

        if let Some(existing) = existing {
            let snapshot = existing.snapshot();
            if snapshot.phase == InstallPhase::Paused {
                // `install_game` doubles as "resume": keep the same job (and its
                // `.part` path) so the download continues with HTTP Range.
                existing.clear_pause();
                existing.update(|job| {
                    job.phase = InstallPhase::Queued;
                    job.speed_bps = 0;
                    job.message = Some("Queued".to_string());
                });
                persist(app, &existing);
                emit_jobs(app);
                start_run(app, &existing);
                return Ok(existing.snapshot());
            }
            return Ok(snapshot);
        }
    }

    // A new attempt replaces any paused/active row for the same version, so the
    // job list does not keep a stale paused entry after a restart.
    if let Ok(connection) = state.db.lock() {
        if let Err(error) =
            db::supersede_install_jobs(&connection, game_id, version_id, &now_rfc3339())
        {
            tracing::warn!(error = %error, "could not supersede the previous install jobs");
        }
    }

    persist(app, &entry);
    emit_jobs(app);
    start_run(app, &entry);

    Ok(job)
}

/// Spawn the worker for a job, unless one is already running. Pausing stops the
/// worker; resuming calls this again with the same entry.
fn start_run(app: &AppHandle, entry: &Arc<JobEntry>) {
    if entry.running.swap(true, Ordering::SeqCst) {
        return;
    }
    let handle = app.clone();
    let job_id = entry.snapshot().job_id;
    tauri::async_runtime::spawn(async move {
        run(handle, job_id).await;
    });
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

/// All known jobs, newest first: active in-memory jobs (fresh snapshots) plus
/// persisted rows from this or a previous run, so the UI can discover
/// resumable installs after a restart.
pub fn list(app: &AppHandle) -> Result<Vec<InstallJob>, String> {
    let state = app.state::<AppState>();

    let in_memory: Vec<Arc<JobEntry>> = {
        let jobs = state
            .jobs
            .lock()
            .map_err(|_| "The install queue is unavailable; restart Ludex.".to_string())?;
        jobs.values().cloned().collect()
    };

    let mut merged: Vec<(String, String, InstallJob)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in in_memory {
        let snapshot = entry.snapshot();
        seen.insert(snapshot.job_id.clone());
        merged.push((entry.created_at.clone(), snapshot.job_id.clone(), snapshot));
    }

    {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        for record in db::all_install_jobs(&connection)? {
            if seen.contains(&record.job_id) {
                continue;
            }
            let created_at = record.created_at.clone();
            let job_id = record.job_id.clone();
            merged.push((created_at, job_id, InstallJob::from_record(record)));
        }
    }

    // Newest first; job id breaks ties deterministically.
    merged.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    Ok(merged.into_iter().map(|(_, _, job)| job).collect())
}

/// Tell the UI that the job list changed (queued, finished, cancelled).
fn emit_jobs(app: &AppHandle) {
    match list(app) {
        Ok(jobs) => {
            let _ = app.emit("install://jobs", jobs);
        }
        Err(error) => tracing::warn!(error = %error, "could not emit install job list"),
    }
    // The tray tooltip reports active downloads; keep it in sync.
    crate::tray::refresh_tooltip(app);
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
            // A paused job has no worker waiting on the token: finish it here so
            // its partial archive is cleaned up.
            if entry.snapshot().phase == InstallPhase::Paused {
                let paths = planned_paths(&state, &entry);
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    finalize_cancelled(&handle, &entry, &paths).await;
                });
            }
            Ok(())
        }
        None => Err(format!("Unknown install job: {job_id}")),
    }
}

/// Pause a queued or downloading job.
///
/// The download loop stops at the next chunk boundary, flushes the partial
/// archive to disk and the job moves to the `paused` phase. Resuming continues
/// from the same byte offset with an HTTP Range request. Stream installs cannot
/// be paused: nothing is written to the download folder, so there is no offset
/// to resume from.
pub fn pause(app: &AppHandle, job_id: &str) -> Result<InstallJob, String> {
    let state = app.state::<AppState>();
    let entry = {
        let jobs = state
            .jobs
            .lock()
            .map_err(|_| "The install queue is unavailable; restart Ludex.".to_string())?;
        jobs.get(job_id).cloned()
    }
    .ok_or_else(|| {
        "This install job is from a previous session. Start the install again from the game page."
            .to_string()
    })?;

    let snapshot = entry.snapshot();
    match snapshot.phase {
        InstallPhase::Paused => Ok(snapshot),
        InstallPhase::Queued => {
            entry.request_pause();
            entry.update(|job| {
                job.phase = InstallPhase::Paused;
                job.speed_bps = 0;
                job.message = Some("Paused".to_string());
            });
            persist(app, &entry);
            emit_jobs(app);
            Ok(entry.snapshot())
        }
        InstallPhase::Downloading => {
            if stream_requested(
                entry.params.stream,
                &state.settings_snapshot(),
                &entry.params.server_url,
            ) {
                return Err(
                    "Stream installs cannot be paused. Turn off stream installs in Settings, then retry."
                        .to_string(),
                );
            }
            entry.request_pause();
            // The worker flushes the `.part` file and emits the paused state.
            Ok(entry.snapshot())
        }
        _ => Err(
            "This install is not downloading, so it cannot be paused. Wait for it to finish or cancel it."
                .to_string(),
        ),
    }
}

/// Resume a paused job from its byte offset. Returns the running job.
pub fn resume(app: &AppHandle, job_id: &str) -> Result<InstallJob, String> {
    let state = app.state::<AppState>();
    let entry = {
        let jobs = state
            .jobs
            .lock()
            .map_err(|_| "The install queue is unavailable; restart Ludex.".to_string())?;
        jobs.get(job_id).cloned()
    }
    .ok_or_else(|| {
        "This install job is from a previous session. Start the install again from the game page to resume it."
            .to_string()
    })?;

    let snapshot = entry.snapshot();
    if snapshot.phase != InstallPhase::Paused {
        return Ok(snapshot);
    }

    entry.clear_pause();
    entry.update(|job| {
        job.phase = InstallPhase::Queued;
        job.speed_bps = 0;
        job.message = Some("Resuming".to_string());
    });
    persist(app, &entry);
    emit_jobs(app);
    start_run(app, &entry);
    Ok(entry.snapshot())
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
    let download_root = settings::resolve_download_dir(
        &settings,
        &install_root,
        entry.params.download_dir.as_deref(),
    );
    let part = entry
        .stored_paths()
        .0
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            download_root.join(format!(
                ".{}-{}.part",
                entry.params.game_id, entry.params.version_id
            ))
        });
    JobPaths {
        staging: Some(install_root.join(format!(".{slug}.staging-{}", short_id(&job_id)))),
        part: Some(part),
    }
}

/// Clears the `running` flag when the worker exits, however it exits.
struct RunGuard<'a>(&'a AtomicBool);

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

async fn run(app: AppHandle, job_id: String) {
    run_job(app.clone(), job_id.clone()).await;

    // A resume can land while the worker is finishing a pause: `start_run` sees
    // `running = true` and does nothing. Pick the job up again once the flag is
    // clear so it never stays queued without a worker.
    let state = app.state::<AppState>();
    let entry = {
        match state.jobs.lock() {
            Ok(jobs) => jobs.get(&job_id).cloned(),
            Err(_) => None,
        }
    };
    if let Some(entry) = entry {
        if entry.snapshot().phase == InstallPhase::Queued && !entry.cancel.is_cancelled() {
            start_run(&app, &entry);
        }
    }
}

async fn run_job(app: AppHandle, job_id: String) {
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
    let _guard = RunGuard(&entry.running);

    let mut job_paths = planned_paths(&state, &entry);
    entry.set_part_path(
        job_paths
            .part
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
    );

    // One install at a time; queued jobs wait here. A pause while queued moves
    // the job out of the queue without ever touching the disk.
    let slots = Arc::clone(&state.install_slots);
    let pause = entry.pause_token();
    let permit = tokio::select! {
        biased;
        _ = entry.cancel.cancelled() => {
            finalize_cancelled(&app, &entry, &job_paths).await;
            return;
        }
        _ = pause.cancelled() => {
            finalize_paused(&app, &entry, &job_paths).await;
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
    if entry.pause_token().is_cancelled() {
        drop(permit);
        finalize_paused(&app, &entry, &job_paths).await;
        return;
    }

    entry.update(|job| {
        job.phase = InstallPhase::Queued;
        job.message = Some("Starting install".to_string());
    });
    persist(&app, &entry);
    emit_jobs(&app);

    // Heartbeat: the UI must see progress at least every 500 ms even when the
    // network or the extractor is quiet. `set_progress` also emits on change.
    let heartbeat = {
        let app = app.clone();
        let entry = Arc::clone(&entry);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(400)).await;
                let phase = entry.snapshot().phase;
                if phase.is_terminal() {
                    break;
                }
                if matches!(phase, InstallPhase::Downloading | InstallPhase::Extracting) {
                    let _ = app.emit("install://progress", entry.snapshot());
                }
            }
        })
    };

    let result = execute(&app, &entry, &mut job_paths).await;
    heartbeat.abort();
    drop(permit);

    if result.is_err() {
        if let Some(staging) = job_paths.staging.as_deref() {
            let _ = tokio::fs::remove_dir_all(staging).await;
        }
        // Cancelling discards the partial archive; pausing keeps it so the
        // next attempt resumes with HTTP Range.
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
            emit_jobs(&app);
            crate::tray::refresh_menu(&app);
            tracing::info!(job_id = %job_id, game_id = job.game_id, path = ?job.install_path, "install complete");
        }
        Err(Failure::Cancelled) => {
            finalize_cancelled(&app, &entry, &job_paths).await;
        }
        Err(Failure::Paused) => {
            finalize_paused(&app, &entry, &job_paths).await;
        }
        Err(Failure::Failed(message)) => {
            finalize_error(&app, &entry, message);
        }
    }
}

/// Stop a job while keeping its partial archive. The next `install_game` or
/// `resume_install` continues from `bytesDone` with a Range request.
async fn finalize_paused(app: &AppHandle, entry: &JobEntry, job_paths: &JobPaths) {
    // A cancel that raced with the pause must win: the user asked for the
    // partial archive to be discarded.
    if entry.cancel.is_cancelled() {
        finalize_cancelled(app, entry, job_paths).await;
        return;
    }
    if let Some(staging) = job_paths.staging.as_deref() {
        let _ = tokio::fs::remove_dir_all(staging).await;
    }
    entry.update(|job| {
        job.phase = InstallPhase::Paused;
        job.speed_bps = 0;
        job.message = Some("Paused".to_string());
    });
    persist(app, entry);
    let _ = app.emit("install://progress", entry.snapshot());
    emit_jobs(app);
    tracing::info!(job_id = %entry.snapshot().job_id, "install paused");
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
    emit_jobs(app);
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
    emit_jobs(app);
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
        stream: job.stream,
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
    let pause = entry.pause_token();
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

    check_stop(&entry.cancel, &pause)?;

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

    let download_root =
        settings::resolve_download_dir(&settings, &install_root, params.download_dir.as_deref());

    // 2. Stream install (remote ZIP only): unpack the tar while downloading.
    let mut streamed = false;
    if archive.is_none() && stream_requested(params.stream, &settings, &params.server_url) {
        match stream_install(app, entry, &params, &staging).await {
            Ok(StreamOutcome::Done) => {
                streamed = true;
                job_paths.part = None;
                entry.set_part_path(None);
            }
            Ok(StreamOutcome::Unsupported(status)) => {
                tracing::info!(
                    status = status.as_u16(),
                    "stream install unsupported, falling back to download"
                );
                // The job is now a normal, resumable download: a failure must
                // offer "Resume", not the stream-specific retry (#114).
                entry.update(|job| job.stream = false);
                persist(app, entry);
                let _ = tokio::fs::remove_dir_all(&staging).await;
            }
            Err(failure) => return Err(failure),
        }
    }

    // 3. Otherwise download with resume.
    if archive.is_none() && !streamed {
        let part = job_paths.part.clone().unwrap_or_else(|| {
            download_root.join(format!(".{}-{}.part", params.game_id, params.version_id))
        });
        job_paths.part = Some(part.clone());
        entry.set_part_path(Some(part.to_string_lossy().to_string()));
        download(app, entry, &params, &part).await?;
        archive = Some(part.clone());
        downloaded = Some(part);
    }

    check_cancel(&entry.cancel)?;

    // Fingerprint the archive before it is deleted: it is how update detection
    // knows whether the installed files came from this exact archive.
    let archive_hash = match archive.as_deref() {
        Some(path) => {
            let path = path.to_path_buf();
            match tokio::task::spawn_blocking(move || ludex_core::archive_fingerprint(&path)).await
            {
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
        None => None,
    };

    let relative_executable: Option<PathBuf> = if streamed {
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
        let staging_for_exe = staging.clone();
        let title = params.title.clone();
        let requested_exe = params.executable.clone();
        tokio::task::spawn_blocking(move || {
            resolve_executable(&staging_for_exe, requested_exe.as_deref(), &title)
        })
        .await
        .map_err(|e| Failure::Failed(format!("Could not inspect the streamed files: {e}")))??
    } else {
        let archive = archive.ok_or_else(|| {
            Failure::Failed("No archive was available for this install.".to_string())
        })?;
        let archive_size = std::fs::metadata(&archive)
            .map(|metadata| metadata.len())
            .ok();

        if looks_like_single_file(&archive) {
            // 3a. Single-file game: no archive to extract, copy it into the
            // install folder and use it as the executable.
            let file_name = single_file_name(&archive, &slug);
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

            if let Some(part) = downloaded.as_deref() {
                cleanup_archive(part, settings.keep_archives, &file_name).await;
            }

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
                        let _ = app_for_extract
                            .emit("install://progress", entry_for_extract.snapshot());
                    },
                )
            })
            .await
            .map_err(|e| Failure::Failed(format!("The extraction task failed: {e}")))?;
            extraction.map_err(Failure::Failed)?;

            // The archive is no longer needed once extraction succeeded.
            if let Some(part) = downloaded.as_deref() {
                cleanup_archive(
                    part,
                    settings.keep_archives,
                    &format!("{slug}-{version_label}.archive"),
                )
                .await;
            }

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
        }
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

    // 6. Record the file manifest (relative path, size, SHA-256) so the install
    // can be verified and repaired later. Hashing a large game takes a while, so
    // progress is reported with the finalizing phase.
    record_manifest(app, entry, &final_path, &params).await;

    // 7. Index the install.
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
        archive_hash,
        install_root: Some(install_root.to_string_lossy().to_string()),
        source: db::SOURCE_LUDEX.to_string(),
        launch_uri: None,
    };

    {
        let connection = state.db.lock().map_err(|_| {
            Failure::Failed("The local database is unavailable; restart Ludex.".to_string())
        })?;
        db::upsert_installed(&connection, &installed).map_err(Failure::Failed)?;
    }

    entry.set_part_path(None);

    // Update installs: the previous folder is only removed once the new one is
    // indexed, so a failed update never destroys a playable install. Saves live
    // outside the install folder and are never touched.
    if let Some(previous) = params
        .replace_path
        .as_deref()
        .map(PathBuf::from)
        .filter(|previous| previous != &final_path)
    {
        let base = install_root.clone();
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

    Ok(final_path)
}

/// Build and persist the manifest after the files are in their final folder.
/// A failure here never fails the install: the game is playable, it just cannot
/// be verified until it is reinstalled.
async fn record_manifest(
    app: &AppHandle,
    entry: &Arc<JobEntry>,
    root: &Path,
    params: &InstallParams,
) {
    set_progress(
        app,
        entry,
        InstallPhase::Finalizing,
        0,
        None,
        0,
        Some("Recording file manifest".to_string()),
    );

    let handle = app.clone();
    let job_entry = Arc::clone(entry);
    let path = root.to_path_buf();
    let built = tokio::task::spawn_blocking(move || {
        let mut last = Instant::now();
        manifest::build_manifest(&path, &mut |progress| {
            if last.elapsed() < Duration::from_millis(250) {
                return;
            }
            last = Instant::now();
            job_entry.update(|job| {
                job.phase = InstallPhase::Finalizing;
                job.bytes_done = progress.bytes_done;
                job.bytes_total = Some(progress.bytes_total.max(1));
                job.speed_bps = 0;
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

    let encoded = match serde_json::to_string(&manifest) {
        Ok(encoded) => encoded,
        Err(error) => {
            tracing::warn!(error = %error, "could not encode the file manifest");
            return;
        }
    };

    let state = app.state::<AppState>();
    let record = InstallManifestRecord {
        game_id: params.game_id,
        version_id: params.version_id,
        install_path: root.to_string_lossy().to_string(),
        manifest: encoded,
        files: manifest.entries.len() as i64,
        bytes: manifest.total_bytes as i64,
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

fn check_cancel(cancel: &CancellationToken) -> Result<(), Failure> {
    if cancel.is_cancelled() {
        Err(Failure::Cancelled)
    } else {
        Ok(())
    }
}

/// Cancel wins over pause when both are requested.
fn check_stop(cancel: &CancellationToken, pause: &CancellationToken) -> Result<(), Failure> {
    if cancel.is_cancelled() {
        Err(Failure::Cancelled)
    } else if pause.is_cancelled() {
        Err(Failure::Paused)
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

pub(crate) fn absolute_path(path: PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .map_err(|e| format!("Could not resolve a relative install path: {e}"))
    }
}

pub(crate) async fn local_archive_path(
    state: &AppState,
    params: &InstallParams,
) -> Option<PathBuf> {
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
    matches!(
        host.to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "::1"
    )
}

/// Snapshot passed to the download progress callback.
pub(crate) struct DownloadProgress {
    pub done: u64,
    pub total: Option<u64>,
    pub speed_bps: u64,
    pub message: String,
}

/// Paces a copy loop to a KiB/s cap. `0` disables throttling.
#[derive(Debug)]
pub(crate) struct Throttle {
    bytes_per_sec: f64,
    started: Instant,
    sent: u64,
}

impl Throttle {
    pub(crate) fn new(limit_kib: u64) -> Option<Self> {
        (limit_kib > 0).then(|| Self {
            bytes_per_sec: limit_kib.saturating_mul(1024) as f64,
            started: Instant::now(),
            sent: 0,
        })
    }

    /// Record `bytes` and return how long the caller must sleep to stay on
    /// schedule. The average rate converges on the configured cap.
    pub(crate) fn pace(&mut self, bytes: usize) -> Option<Duration> {
        self.sent = self.sent.saturating_add(bytes as u64);
        let expected = Duration::from_secs_f64(self.sent as f64 / self.bytes_per_sec);
        expected.checked_sub(self.started.elapsed())
    }
}

/// Download `url` into `part`, resuming an existing file with HTTP Range.
///
/// The `.part` file is kept when the user pauses; cancelling is handled by the
/// caller (which deletes it). `progress` is called at most every 250 ms.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn download_file(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
    part: &Path,
    title: &str,
    cancel: &CancellationToken,
    pause: &CancellationToken,
    bandwidth_kib: u64,
    mut progress: impl FnMut(DownloadProgress) + Send,
) -> Result<(), Failure> {
    if let Some(parent) = part.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            Failure::Failed(format!(
                "Could not create the download folder {}: {e}",
                parent.display()
            ))
        })?;
    }

    let mut existing = tokio::fs::metadata(part)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    let mut restarted = false;
    let mut response = loop {
        let mut request = client
            .get(url)
            .header(reqwest::header::ACCEPT_ENCODING, "identity");
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if existing > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={existing}-"));
        }

        let response = request.send().await.map_err(|e| {
            Failure::Failed(format!(
                "Could not reach the server to download \"{title}\": {e}"
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
                progress(DownloadProgress {
                    done: existing,
                    total: Some(existing),
                    speed_bps: 0,
                    message: "Download already complete".to_string(),
                });
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

    // Disk preflight: never start a download that cannot fit. Extraction size
    // is unknown, so we only require the archive itself; the job message warns
    // that peak usage is archive + extracted.
    let needed = total.unwrap_or(0).saturating_sub(done);
    if needed > 0 {
        if let Some(directory) = part.parent() {
            match fs2::available_space(directory) {
                Ok(free) => {
                    if let Err(message) = ensure_download_space(needed, free, directory) {
                        return Err(Failure::Failed(message));
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %error, path = %directory.display(), "could not check free disk space");
                }
            }
        }
    }

    progress(DownloadProgress {
        done,
        total,
        speed_bps: 0,
        message: download_message(title),
    });

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
    let mut throttle = Throttle::new(bandwidth_kib);
    let mut last_emit = Instant::now();
    let mut last_instant = Instant::now();
    let mut last_bytes = done;

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                let _ = writer.flush().await;
                return Err(Failure::Cancelled);
            }
            _ = pause.cancelled() => {
                let _ = writer.flush().await;
                return Err(Failure::Paused);
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

                        if let Some(throttle) = throttle.as_mut() {
                            if let Some(delay) = throttle.pace(bytes.len()) {
                                if delay > Duration::from_millis(2) {
                                    tokio::select! {
                                        biased;
                                        _ = cancel.cancelled() => {
                                            let _ = writer.flush().await;
                                            return Err(Failure::Cancelled);
                                        }
                                        _ = pause.cancelled() => {
                                            let _ = writer.flush().await;
                                            return Err(Failure::Paused);
                                        }
                                        _ = tokio::time::sleep(delay) => {}
                                    }
                                }
                            }
                        }

                        if last_emit.elapsed() >= Duration::from_millis(250) {
                            let elapsed = last_instant.elapsed().as_secs_f64().max(0.001);
                            let speed = ((done.saturating_sub(last_bytes)) as f64 / elapsed) as u64;
                            progress(DownloadProgress {
                                done,
                                total,
                                speed_bps: speed,
                                message: download_message(title),
                            });
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

    progress(DownloadProgress {
        done,
        total: Some(total.unwrap_or(done)),
        speed_bps: 0,
        message: "Download complete".to_string(),
    });

    Ok(())
}

async fn download(
    app: &AppHandle,
    entry: &Arc<JobEntry>,
    params: &InstallParams,
    part: &Path,
) -> Result<(), Failure> {
    let state = app.state::<AppState>();
    let client = state.http.clone();
    let bandwidth_kib = state.settings_snapshot().bandwidth_limit_kib;
    let base = params.server_url.trim_end_matches('/');
    if base.is_empty() {
        return Err(Failure::Failed(
            "No server URL was provided for this download.".to_string(),
        ));
    }

    let url = format!(
        "{base}/api/games/{}/download?versionId={}",
        params.game_id, params.version_id
    );
    let cancel = entry.cancel.clone();
    let pause = entry.pause_token();
    let handle = app.clone();
    let job_entry = Arc::clone(entry);

    download_file(
        &client,
        &url,
        non_empty(params.token.as_deref()),
        part,
        &params.title,
        &cancel,
        &pause,
        bandwidth_kib,
        move |update| {
            job_entry.update(|job| {
                job.phase = InstallPhase::Downloading;
                job.bytes_done = update.done;
                job.bytes_total = update.total;
                job.speed_bps = update.speed_bps;
                job.message = Some(update.message);
            });
            persist(&handle, &job_entry);
            let _ = handle.emit("install://progress", job_entry.snapshot());
        },
    )
    .await
}

/// Delete the temp archive as soon as extraction/copy succeeded, or keep it
/// under a friendly name when `keepArchives` is enabled.
async fn cleanup_archive(part: &Path, keep: bool, name: &str) {
    if keep {
        let kept = part
            .parent()
            .map(|directory| directory.join(name))
            .unwrap_or_else(|| PathBuf::from(name));
        if let Err(error) = tokio::fs::rename(part, &kept).await {
            tracing::warn!(error = %error, "could not keep the downloaded archive");
        }
    } else if let Err(error) = tokio::fs::remove_file(part).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(error = %error, "could not delete the downloaded archive");
        }
    }
}

fn ensure_download_space(needed: u64, free: u64, directory: &Path) -> Result<(), String> {
    if needed > 0 && free < needed {
        Err(format!(
            "Not enough free space in {}: {} needed, {} available. Free space or change the download folder.",
            directory.display(),
            human_bytes(needed),
            human_bytes(free)
        ))
    } else {
        Ok(())
    }
}

fn download_message(title: &str) -> String {
    format!(
        "Downloading \"{title}\" (peak disk use: archive + extracted; archive deleted after extraction)"
    )
}

/// Should this install use the ZIP tar stream instead of download+extract?
///
/// The per-call `stream` flag wins over `settings.streamInstall`. Loopback
/// servers never stream: the local-path shortcut is cheaper and lossless.
fn stream_requested(requested: Option<bool>, settings: &LocalSettings, server_url: &str) -> bool {
    let enabled = requested.unwrap_or(settings.stream_install);
    enabled && !server_url.trim().is_empty() && !is_loopback_url(server_url)
}

#[derive(Debug)]
enum StreamOutcome {
    Done,
    /// The server cannot stream this version (non-ZIP). Fall back to the
    /// normal download + extract path.
    Unsupported(reqwest::StatusCode),
}

/// Counts bytes pulled from the HTTP stream so progress reflects the download,
/// not the extracted file sizes.
struct ProgressReader<R> {
    inner: R,
    bytes: Arc<AtomicU64>,
}

impl<R> ProgressReader<R> {
    fn new(inner: R, bytes: Arc<AtomicU64>) -> Self {
        Self { inner, bytes }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for ProgressReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buffer.filled().len();
        match Pin::new(&mut this.inner).poll_read(context, buffer) {
            Poll::Ready(Ok(())) => {
                let read = buffer.filled().len().saturating_sub(before);
                this.bytes.fetch_add(read as u64, Ordering::Relaxed);
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

/// Stream a remote ZIP as tar and unpack it directly into `staging`.
///
/// Nothing is written to the download folder: the tar is consumed as it
/// arrives. Non-ZIP versions answer 415/404 and the caller falls back to the
/// regular download. Stream installs are not resumable.
async fn stream_install(
    app: &AppHandle,
    entry: &Arc<JobEntry>,
    params: &InstallParams,
    staging: &Path,
) -> Result<StreamOutcome, Failure> {
    let state = app.state::<AppState>();
    let bandwidth_kib = state.settings_snapshot().bandwidth_limit_kib;
    let base = params.server_url.trim_end_matches('/');
    if base.is_empty() {
        return Ok(StreamOutcome::Unsupported(reqwest::StatusCode::NOT_FOUND));
    }

    let url = format!(
        "{base}/api/games/{}/versions/{}/stream",
        params.game_id, params.version_id
    );
    let mut request = state
        .http
        .get(url)
        .header(reqwest::header::ACCEPT_ENCODING, "identity");
    if let Some(token) = non_empty(params.token.as_deref()) {
        request = request.bearer_auth(token);
    }

    let response = tokio::select! {
        biased;
        _ = entry.cancel.cancelled() => return Err(Failure::Cancelled),
        response = request.send() => response.map_err(|e| {
            Failure::Failed(format!(
                "Could not reach the server to stream \"{}\": {e}",
                params.title
            ))
        })?,
    };

    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND
        || status == reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE
    {
        return Ok(StreamOutcome::Unsupported(status));
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let snippet = body.trim().chars().take(300).collect::<String>();
        let detail = if snippet.is_empty() {
            String::new()
        } else {
            format!(": {snippet}")
        };
        return Err(Failure::Failed(format!(
            "Stream install failed with HTTP {status}{detail}"
        )));
    }
    if let Some(content_type) = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    {
        if !content_type
            .to_ascii_lowercase()
            .starts_with("application/x-tar")
        {
            return Ok(StreamOutcome::Unsupported(status));
        }
    }

    // Exact length when the server sets it; otherwise an estimate that must
    // never make the UI show 100% before the stream actually ends.
    let exact_total = response.content_length();
    let estimated_total = response
        .headers()
        .get("x-ludex-estimated-bytes")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0);
    let counter = Arc::new(AtomicU64::new(0));

    tokio::fs::create_dir_all(staging).await.map_err(|e| {
        Failure::Failed(format!(
            "Could not create the staging folder {}: {e}",
            staging.display()
        ))
    })?;

    set_progress(
        app,
        entry,
        InstallPhase::Downloading,
        0,
        exact_total.or(estimated_total),
        0,
        Some("Streaming install".to_string()),
    );

    let stream = response.bytes_stream().map_err(std::io::Error::other);
    let reader = ProgressReader::new(StreamReader::new(Box::pin(stream)), Arc::clone(&counter));

    let progress_task = {
        let app = app.clone();
        let entry = Arc::clone(entry);
        let counter = Arc::clone(&counter);
        tokio::spawn(async move {
            let mut last_bytes = 0u64;
            let mut last_at = Instant::now();
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let done = counter.load(Ordering::Relaxed);
                let elapsed = last_at.elapsed().as_secs_f64().max(0.001);
                let speed = ((done.saturating_sub(last_bytes)) as f64 / elapsed) as u64;
                let total = match (exact_total, estimated_total) {
                    (Some(total), _) => Some(total.max(done.saturating_add(1))),
                    (None, Some(estimate)) => Some(estimate.max(done.saturating_add(1))),
                    (None, None) => None,
                };
                entry.update(|job| {
                    job.phase = InstallPhase::Downloading;
                    job.bytes_done = done;
                    job.bytes_total = total;
                    job.speed_bps = speed;
                    job.message = Some("Streaming install".to_string());
                });
                persist(&app, &entry);
                let _ = app.emit("install://progress", entry.snapshot());
                last_bytes = done;
                last_at = Instant::now();
            }
        })
    };

    let unpacked = tokio::select! {
        biased;
        _ = entry.cancel.cancelled() => Err(Failure::Cancelled),
        result = unpack_tar_stream(reader, staging, bandwidth_kib) => result,
    };
    progress_task.abort();
    let files = unpacked?;
    let done = counter.load(Ordering::Relaxed);

    // The stream ended: only now is it safe to show a complete percentage.
    set_progress(
        app,
        entry,
        InstallPhase::Downloading,
        done,
        Some(done.max(1)),
        0,
        Some(format!("Stream complete ({files} files)")),
    );

    Ok(StreamOutcome::Done)
}

/// Unpack an uncompressed tar stream into `staging`.
///
/// Only regular files and directories are accepted; absolute paths, `..`
/// components, symlinks, hard links and special files abort the install.
async fn unpack_tar_stream<R>(reader: R, staging: &Path, bandwidth_kib: u64) -> Result<u64, Failure>
where
    R: AsyncRead + Unpin,
{
    let mut archive = tokio_tar::Archive::new(reader);
    let mut entries = archive
        .entries()
        .map_err(|e| Failure::Failed(format!("The server did not send a valid tar stream: {e}")))?;

    let mut files = 0u64;
    while let Some(entry) = entries.next().await {
        let mut entry = entry.map_err(|e| Failure::Failed(format!("Invalid tar stream: {e}")))?;
        let entry_type = entry.header().entry_type();
        let path = entry
            .path()
            .map_err(|e| Failure::Failed(format!("Invalid path in the tar stream: {e}")))?;
        let relative = match safe_tar_path(&path) {
            Some(relative) => relative,
            // Some tars start with a bare `./` directory entry; it is benign.
            None if entry_type.is_dir()
                && path
                    .components()
                    .all(|component| matches!(component, Component::CurDir)) =>
            {
                continue;
            }
            None => {
                return Err(Failure::Failed(format!(
                    "The tar stream contains an unsafe path: {}",
                    path.display()
                )))
            }
        };
        let target = staging.join(&relative);

        if entry_type.is_dir() {
            tokio::fs::create_dir_all(&target).await.map_err(|e| {
                Failure::Failed(format!(
                    "Could not create the folder {}: {e}",
                    target.display()
                ))
            })?;
            continue;
        }

        if !entry_type.is_file() {
            return Err(Failure::Failed(format!(
                "The tar stream contains an unsupported entry (symlink or special file): {}",
                path.display()
            )));
        }

        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                Failure::Failed(format!(
                    "Could not create the folder {}: {e}",
                    parent.display()
                ))
            })?;
        }

        #[cfg(unix)]
        let mode = entry
            .header()
            .mode()
            .ok()
            .map(|mode| mode & 0o777)
            .filter(|mode| *mode != 0);

        let mut file = tokio::fs::File::create(&target).await.map_err(|e| {
            Failure::Failed(format!(
                "Could not create the file {}: {e}",
                target.display()
            ))
        })?;

        // Copy in chunks so the global bandwidth cap applies to stream installs
        // too. A pause is not offered here: the tar has no offset to resume.
        let mut throttle = Throttle::new(bandwidth_kib);
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = entry.read(&mut buffer).await.map_err(|e| {
                Failure::Failed(format!(
                    "The stream was interrupted while writing {}: {e}. Stream installs are not resumable.",
                    target.display()
                ))
            })?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read]).await.map_err(|e| {
                Failure::Failed(format!(
                    "Could not write the streamed file {}: {e}",
                    target.display()
                ))
            })?;
            if let Some(throttle) = throttle.as_mut() {
                if let Some(delay) = throttle.pace(read) {
                    if delay > Duration::from_millis(2) {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
        file.flush().await.map_err(|e| {
            Failure::Failed(format!("Could not flush {} to disk: {e}", target.display()))
        })?;

        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                tokio::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode)).await;
        }

        files += 1;
    }

    Ok(files)
}

/// Convert a tar entry path to a safe relative path. Rejects absolute paths,
/// `..`, Windows drive/separator tricks and empty paths.
fn safe_tar_path(path: &Path) -> Option<PathBuf> {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let text = part.to_str()?;
                if text.is_empty() || text.contains('\\') || text.contains(':') {
                    return None;
                }
                result.push(part);
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    if result.as_os_str().is_empty() {
        None
    } else {
        Some(result)
    }
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
    fn download_space_preflight() {
        let dir = Path::new("/downloads");
        assert!(ensure_download_space(100, 200, dir).is_ok());
        let error = ensure_download_space(200, 100, dir).unwrap_err();
        assert!(error.contains("Not enough free space"));
        assert!(error.contains("/downloads"));
        // Unknown size (0) never blocks.
        assert!(ensure_download_space(0, 0, dir).is_ok());
    }

    #[test]
    fn stream_flag_precedence_and_loopback_fallback() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.stream_install = false;
        assert!(!stream_requested(
            None,
            &settings,
            "https://ludex.example.com"
        ));
        assert!(stream_requested(
            Some(true),
            &settings,
            "https://ludex.example.com"
        ));
        assert!(!stream_requested(
            Some(false),
            &settings,
            "https://ludex.example.com"
        ));

        settings.stream_install = true;
        assert!(stream_requested(
            None,
            &settings,
            "https://ludex.example.com"
        ));
        assert!(!stream_requested(None, &settings, "http://127.0.0.1:4123"));
        assert!(!stream_requested(
            Some(true),
            &settings,
            "http://[::1]:8080"
        ));
        assert!(!stream_requested(Some(true), &settings, ""));
    }

    #[test]
    fn tar_paths_are_guarded() {
        assert_eq!(
            safe_tar_path(Path::new("bin/game.sh")),
            Some(PathBuf::from("bin/game.sh"))
        );
        assert_eq!(
            safe_tar_path(Path::new("./bin/./game.sh")),
            Some(PathBuf::from("bin/game.sh"))
        );
        assert!(safe_tar_path(Path::new("../escape")).is_none());
        assert!(safe_tar_path(Path::new("dir/../../escape")).is_none());
        assert!(safe_tar_path(Path::new("/absolute")).is_none());
        assert!(safe_tar_path(Path::new("C:escape")).is_none());
        assert!(safe_tar_path(Path::new("")).is_none());
    }

    #[tokio::test]
    async fn unpacks_a_tar_stream() {
        let mut builder = tokio_tar::Builder::new(Vec::new());
        let data = b"#!/bin/sh\necho hi\n";
        let mut header = tokio_tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "bin/game.sh", &data[..])
            .await
            .unwrap();
        let bytes = builder.into_inner().await.unwrap();

        let root = std::env::temp_dir().join(format!(
            "ludex-tar-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&root).unwrap();

        let files = unpack_tar_stream(&bytes[..], &root, 0).await.unwrap();
        assert_eq!(files, 1);
        assert_eq!(std::fs::read(root.join("bin/game.sh")).unwrap(), data);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn phase_roundtrip() {
        for phase in [
            InstallPhase::Queued,
            InstallPhase::Downloading,
            InstallPhase::Extracting,
            InstallPhase::Finalizing,
            InstallPhase::Paused,
            InstallPhase::Done,
            InstallPhase::Cancelled,
            InstallPhase::Error,
        ] {
            assert_eq!(InstallPhase::parse(phase.as_str()), phase);
        }
        assert!(InstallPhase::Done.is_terminal());
        assert!(!InstallPhase::Downloading.is_terminal());
        assert!(!InstallPhase::Paused.is_terminal());
    }

    fn download_temp_dir(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ludex-download-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[tokio::test]
    async fn pause_then_resume_produces_identical_bytes() {
        let payload: Vec<u8> = (0..512 * 1024).map(|index| (index % 251) as u8).collect();
        let server = crate::test_support::serve_bytes(payload.clone()).await;
        let root = download_temp_dir("pause");
        let part = root.join("archive.part");
        let client = reqwest::Client::builder().build().unwrap();

        let cancel = CancellationToken::new();
        let pause = CancellationToken::new();
        let pause_from_progress = pause.clone();
        let mut paused_once = false;

        let first = download_file(
            &client,
            &server.url,
            None,
            &part,
            "Paused game",
            &cancel,
            &pause,
            256,
            move |update| {
                if update.done > 0 && !paused_once {
                    paused_once = true;
                    pause_from_progress.cancel();
                }
            },
        )
        .await;
        assert!(matches!(&first, Err(Failure::Paused)));

        let partial = std::fs::metadata(&part).unwrap().len();
        assert!(partial > 0, "the partial archive must be kept");
        assert!(
            partial < payload.len() as u64,
            "the download should stop before the end"
        );

        // Resume from the byte offset with a fresh pause token.
        let pause = CancellationToken::new();
        download_file(
            &client,
            &server.url,
            None,
            &part,
            "Paused game",
            &cancel,
            &pause,
            0,
            |_| {},
        )
        .await
        .unwrap();

        let (size, sha256) = manifest::hash_file(&part).unwrap();
        assert_eq!(size, payload.len() as u64);
        let source = root.join("source.bin");
        std::fs::write(&source, &payload).unwrap();
        let expected = manifest::hash_file(&source).unwrap().1;
        assert_eq!(sha256, expected, "resume must produce the same bytes");

        let ranges = server.ranges.lock().unwrap().clone();
        assert!(
            ranges.iter().any(|range| range.starts_with("bytes=")),
            "the resume must use a Range request, got {ranges:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn bandwidth_limit_slows_the_download() {
        let payload = vec![3u8; 256 * 1024];
        let server = crate::test_support::serve_bytes(payload.clone()).await;
        let root = download_temp_dir("throttle");
        let part = root.join("archive.part");
        let client = reqwest::Client::builder().build().unwrap();

        let started = Instant::now();
        download_file(
            &client,
            &server.url,
            None,
            &part,
            "Throttled game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            512,
            |_| {},
        )
        .await
        .unwrap();
        let elapsed = started.elapsed();

        assert_eq!(
            std::fs::metadata(&part).unwrap().len(),
            payload.len() as u64
        );
        // 256 KiB at 512 KiB/s is ~0.5 s; allow a loose lower bound for slow CI.
        assert!(
            elapsed >= Duration::from_millis(300),
            "the download ignored the bandwidth limit: {elapsed:?}"
        );
        assert!(
            elapsed <= Duration::from_secs(10),
            "the throttle stalled the download: {elapsed:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn throttle_paces_bytes() {
        let mut throttle = Throttle::new(256).unwrap();
        let started = Instant::now();
        for _ in 0..2 {
            if let Some(delay) = throttle.pace(32 * 1024) {
                std::thread::sleep(delay);
            }
        }
        // 64 KiB at 256 KiB/s is ~0.25 s.
        assert!(started.elapsed() >= Duration::from_millis(180));
        assert!(Throttle::new(0).is_none());
    }
}
