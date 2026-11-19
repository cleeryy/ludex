//! The job scheduler and worker lifecycle.
//!
//! This is the stateful half of the pipeline: `begin` inserts a job, `dispatch`
//! admits the next one under the download/extract slots, the worker runs one
//! job and every exit path (done, paused, cancelled, failed, panicked) ends in
//! a small finalizer that persists the row and re-dispatches. The queue
//! commands (`pause`, `resume`, `cancel`, `reorder`, `check_space`) only mutate
//! the registry; they never touch the transfer, which is what makes them safe
//! to call while a worker is running.
//!
//! The commit steps themselves live in [`super::commit`]; this module only
//! supplies the slots, the heartbeat, the cleanup and the persistence.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

use crate::db;
use crate::paths;
use crate::pipeline::{self, Admission};
use crate::settings;
use crate::space::{self, SpaceStatus};
use crate::state::{AppState, JobEntry, SlotGuard};
use crate::util::{human_bytes, now_rfc3339, short_id};

use super::files::clear_file_checkpoint;
use super::job::{
    Failure, InstallErrorEvent, InstallJob, InstallParams, InstallPhase, JobPaths, QueueMove,
    ResumeMode, ResumeOrigin, INTERRUPTED_MESSAGE, LEGACY_INTERRUPTED_MESSAGE,
    RESUMED_HEALTHY_AFTER, RESUME_DISABLED_MESSAGE,
};
use super::plan::{
    absolute_path, candidates, next_queue_position, plan_compact, plan_for_job, plan_reorder,
    planned_paths, protected_staging, queue_snapshot, success_staging_candidates,
};
use super::source::stream_requested;
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

    let stream_requested_now = stream_requested(params.stream, &settings, &params.server_url);
    let plan = crate::locations::plan_install(
        &settings,
        params.install_location_id.as_deref(),
        params.download_location_id.as_deref(),
        params.install_dir.as_deref(),
        params.download_dir.as_deref(),
        stream_requested_now,
    )?;
    // Worst-case disk requirement (#222). It is reserved at admission, not
    // here: a job that cannot fit yet stays in the queue as
    // `waiting_for_space` with a readable reason instead of failing after a
    // long download, and starts by itself when a reservation is released.
    let space_plan = space::SpacePlan::build(
        params.expected_bytes.filter(|value| *value > 0),
        None,
        &plan.install_root,
        &plan.download_dir,
        stream_requested_now,
        settings.keep_archives,
    );
    tracing::info!(
        cross_filesystem = plan.cross_filesystem,
        note = %plan.peak_note,
        download_dir = %plan.download_dir.display(),
        download_staging = settings.download_staging.as_str(),
        download_policy = settings.download_policy.as_str(),
        archive_bytes = ?space_plan.archive_bytes,
        extracted_bytes = ?space_plan.extracted_bytes,
        estimated_extraction = space_plan.extraction_estimated,
        game_id = params.game_id,
        version_id = params.version_id,
        "install locations and peak disk requirement resolved"
    );
    // Validate the folders now: a relative path or a read-only drive fails the
    // click with a clear message instead of the worker after a long queue wait.
    let install_root = absolute_path(plan.install_root.clone())?;
    let download_dir = absolute_path(plan.download_dir.clone())?;
    // #290: an install start is an explicit moment: try the library's
    // automount before declaring the folder missing.
    let install_status = crate::locations::ensure_path(&install_root)?;
    if !install_status.writable {
        return Err(format!(
            "The install location {} is not writable.{}",
            install_root.display(),
            install_status
                .note
                .map(|note| format!(" {note}"))
                .unwrap_or_default()
        ));
    }
    if !stream_requested_now {
        let download_status = crate::locations::ensure_path(&download_dir)?;
        if !download_status.writable {
            return Err(format!(
                "The download location {} is not writable.{}",
                download_dir.display(),
                download_status
                    .note
                    .map(|note| format!(" {note}"))
                    .unwrap_or_default()
            ));
        }
    }

    // Pin the resolved folders on the job so a location renamed or unplugged
    // while it runs cannot move a paused download.
    params.install_dir = Some(install_root.to_string_lossy().to_string());
    params.download_dir = Some(download_dir.to_string_lossy().to_string());

    let install_root = PathBuf::from(params.install_dir.clone().unwrap_or_default());
    let slug = paths::slug_for_title(&params.title);
    let version_label = paths::version_label(params.version.as_deref());
    let planned_install = install_root.join(&slug).join(&version_label);
    let download_root = PathBuf::from(params.download_dir.clone().unwrap_or_default());

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

    // #205/#208: a retry from the game page must reuse the failed job instead
    // of inserting a second history row. Only a new install (not an update)
    // adopts a resumable job of the same version and transport.
    if params.replace_path.is_none() {
        if let Some(existing) = adoptable_job(&state, &params, stream_requested_now) {
            return resume_entry(app, existing, ResumeMode::Keep, ResumeOrigin::Manual);
        }
    }

    let job_id = uuid::Uuid::new_v4().to_string();
    let stream = stream_requested(params.stream, &settings, &params.server_url);
    let created_at = now_rfc3339();
    // #202: a new job joins the end of the queue; `compact_queue` renumbers
    // the whole active set so a previous gap cannot leak through.
    let queue_position = next_queue_position(&queue_snapshot(&state));
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
        stream,
        resumable: false,
        entries_done: None,
        entries_total: None,
        current_entry: None,
        resumed_entries: None,
        reconnecting: false,
        attempt: 1,
        last_error: None,
        stalled_for_ms: None,
        next_retry_at: None,
        http_status: None,
        waiting_for_network: false,
        queue_position: Some(queue_position),
        prefetching: false,
        space: Some(SpaceStatus {
            volume: space_plan
                .requirements
                .first()
                .map(|requirement| requirement.volume.path.display().to_string())
                .unwrap_or_default(),
            needed_bytes: space_plan
                .requirements
                .iter()
                .map(|requirement| requirement.bytes)
                .max()
                .unwrap_or(0),
            free_bytes: 0,
            reserved_bytes: 0,
            available_bytes: 0,
            ok: true,
        }),
        verified_sha256: None,
        file_manifest: false,
        manifest_fallback: false,
    };

    let entry = Arc::new(JobEntry {
        params,
        cancel: CancellationToken::new(),
        retry_now: tokio::sync::Notify::new(),
        pause: std::sync::Mutex::new(CancellationToken::new()),
        running: AtomicBool::new(false),
        job: std::sync::Mutex::new(job.clone()),
        created_at,
        part_path: std::sync::Mutex::new(Some(planned_part.to_string_lossy().to_string())),
        install_path: std::sync::Mutex::new(Some(planned_install.to_string_lossy().to_string())),
        staging_path: std::sync::Mutex::new(Some(
            install_root
                .join(format!(".{slug}.staging-{}", short_id(&job_id)))
                .to_string_lossy()
                .to_string(),
        )),
        stream_completed: std::sync::Mutex::new(None),
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
                    job.clear_reconnect();
                    job.message = Some("Queued".to_string());
                });
                persist(app, &existing);
                emit_jobs(app);
                dispatch(app);
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

    // #202: place the new job last, then renumber the active queue so the
    // position is always a compact 0..n.
    compact_queue(app);
    persist(app, &entry);
    emit_jobs(app);
    dispatch(app);

    Ok(job)
}

/// Start the next queued install while a download slot is free (#202/#221).
///
/// Two resources are scheduled separately: the **download** slot (1 by
/// default, 2 with `downloadConcurrency`) and the single **extract/commit**
/// slot. A worker gives the download slot back as soon as its transfer ends and
/// then takes the extractor, so the next queued game downloads while the
/// previous one unpacks. The job that starts while the extractor is busy is
/// flagged `prefetching`.
///
/// The queue order still rules: only the lowest position is considered, and a
/// job whose peak disk requirement (#222) does not fit is parked as
/// `waiting_for_space` instead of starting and failing mid-download. A running
/// job is never preempted: it is only stopped by an explicit pause or cancel.
pub(crate) fn dispatch(app: &AppHandle) {
    let state = app.state::<AppState>();
    loop {
        let Some(slot) = state.download_slots.try_acquire() else {
            // Every download slot is busy; the worker that frees one calls this.
            return;
        };
        let admission = pipeline::plan_next(
            &candidates(&state),
            true,
            state.extract_slots.in_use() > 0,
            &state.space,
            &space::SystemCapacity,
        );
        match admission {
            Admission::Idle => return,
            Admission::WaitingForSpace { job_id, space } => {
                mark_waiting_for_space(app, &job_id, space);
                return;
            }
            Admission::Start {
                job_id,
                prefetching,
                space,
            } => {
                // Claim the job under the registry lock, so a cancel or a
                // second dispatch can never start the same entry twice.
                let claimed = match state.jobs.lock() {
                    Ok(jobs) => match jobs.get(&job_id) {
                        Some(entry) => !entry.running.swap(true, Ordering::SeqCst),
                        None => false,
                    },
                    Err(_) => return,
                };
                if !claimed {
                    continue;
                }
                if let Ok(jobs) = state.jobs.lock() {
                    if let Some(entry) = jobs.get(&job_id) {
                        entry.update(|job| {
                            job.prefetching = prefetching;
                            job.space = Some(space.clone());
                            if job.phase == InstallPhase::WaitingForSpace {
                                job.phase = InstallPhase::Queued;
                                job.message = Some("Starting install".to_string());
                            }
                        });
                    }
                }
                emit_jobs(app);
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    run(handle, job_id, slot).await;
                });
            }
        }
    }
}

fn mark_waiting_for_space(app: &AppHandle, job_id: &str, space: SpaceStatus) {
    let state = app.state::<AppState>();
    let entry = match state.jobs.lock() {
        Ok(jobs) => jobs.get(job_id).cloned(),
        Err(_) => None,
    };
    let Some(entry) = entry else {
        return;
    };
    let already = entry.snapshot();
    if already.phase == InstallPhase::WaitingForSpace && already.space.as_ref() == Some(&space) {
        // Nothing changed: do not rewrite the row on every dispatch.
        return;
    }
    let message = space.message();
    entry.update(|job| {
        job.phase = InstallPhase::WaitingForSpace;
        job.speed_bps = 0;
        job.prefetching = false;
        job.clear_reconnect();
        job.message = Some(message.clone());
        job.space = Some(space.clone());
    });
    persist(app, &entry);
    emit_jobs(app);
    tracing::info!(job_id = %job_id, %message, "install waiting for disk space");
}

pub fn check_space(app: &AppHandle, job_id: &str) -> Result<InstallJob, String> {
    let state = app.state::<AppState>();
    let entry = {
        let jobs = state
            .jobs
            .lock()
            .map_err(|_| "The install queue is unavailable; restart Ludex.".to_string())?;
        jobs.get(job_id).cloned()
    }
    .ok_or_else(|| format!("Unknown install job: {job_id}"))?;
    let snapshot = entry.snapshot();
    if snapshot.phase != InstallPhase::WaitingForSpace {
        return Ok(snapshot);
    }
    // Drop the stale status so `dispatch` re-evaluates even when nothing moved.
    let plan = plan_for_job(&state, &entry);
    let reservation = state
        .space
        .try_reserve(job_id, &plan, &space::SystemCapacity);
    if !reservation.is_granted() {
        let space = reservation.status().clone();
        entry.update(|job| {
            job.message = Some(space.message());
            job.space = Some(space);
        });
        persist(app, &entry);
        emit_jobs(app);
        return Ok(entry.snapshot());
    }
    dispatch(app);
    Ok(entry.snapshot())
}

/// Reserved bytes per volume (#222), for Settings → Storage.
pub fn space_ledger(app: &AppHandle) -> Vec<space::LedgerTotal> {
    app.state::<AppState>().space.totals()
}

/// Rebuild the reservation ledger from the active jobs after a restart (#222).
///
/// The rows are the source of truth, so nothing is counted twice: the ledger is
/// cleared first and every active job reserves its requirement again, in queue
/// order. A job that no longer fits stays `waiting_for_space`.
pub(crate) fn rebuild_space_ledger(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.space.reset();
    let entries: Vec<Arc<JobEntry>> = match state.jobs.lock() {
        Ok(jobs) => jobs.values().cloned().collect(),
        Err(_) => return,
    };
    let mut active: Vec<Arc<JobEntry>> = entries
        .into_iter()
        .filter(|entry| !entry.snapshot().phase.is_terminal())
        .collect();
    active.sort_by_key(|entry| entry.snapshot().queue_position.unwrap_or(u32::MAX));
    for entry in active {
        let plan = plan_for_job(&state, &entry);
        let _ = state
            .space
            .try_reserve(&entry.snapshot().job_id, &plan, &space::SystemCapacity);
    }
    tracing::info!(
        totals = ?state.space.totals(),
        "rebuilt the install disk-space ledger"
    );
}

fn apply_queue_positions(app: &AppHandle, assignments: &[(String, u32)]) {
    let state = app.state::<AppState>();
    let entries: Vec<Arc<JobEntry>> = match state.jobs.lock() {
        Ok(jobs) => jobs.values().cloned().collect(),
        Err(_) => return,
    };
    for (job_id, position) in assignments {
        let Some(entry) = entries
            .iter()
            .find(|entry| entry.snapshot().job_id == *job_id)
        else {
            continue;
        };
        if entry.snapshot().queue_position != Some(*position) {
            entry.update(|job| job.queue_position = Some(*position));
        }
    }
    if let Ok(connection) = state.db.lock() {
        let rows: Vec<(String, i64)> = assignments
            .iter()
            .map(|(job_id, position)| (job_id.clone(), i64::from(*position)))
            .collect();
        if let Err(error) = db::set_install_queue_positions(&connection, &rows) {
            tracing::warn!(error = %error, "could not persist the install queue order");
        }
    };
}

/// Renumber the active queue `0..n` so a finished or cancelled job leaves no
/// gap behind.
fn compact_queue(app: &AppHandle) {
    let state = app.state::<AppState>();
    let assignments = plan_compact(&queue_snapshot(&state));
    apply_queue_positions(app, &assignments);
}

/// Move a waiting install in the queue (#202).
///
/// The running job keeps its slot and the front of the list; a running or
/// finished job is refused with a clear message. Returns the new queue order.
pub fn reorder(
    app: &AppHandle,
    job_id: &str,
    action: QueueMove,
) -> Result<Vec<InstallJob>, String> {
    let state = app.state::<AppState>();
    let snapshot = queue_snapshot(&state);
    let assignments = match plan_reorder(&snapshot, job_id, action) {
        Ok(assignments) => assignments,
        Err(error) => {
            // A job that only exists in the database (previous app run) has no
            // queue slot: say so instead of "unknown".
            if !snapshot.iter().any(|job| job.job_id == job_id) {
                if let Ok(connection) = state.db.lock() {
                    if let Ok(Some(record)) = db::get_install_job(&connection, job_id) {
                        return Err(if InstallPhase::parse(&record.phase).is_terminal() {
                            "This install already finished and cannot be reordered.".to_string()
                        } else {
                            "This install is from a previous session. Resume it before reordering the queue."
                                .to_string()
                        });
                    }
                }
            }
            return Err(error);
        }
    };
    apply_queue_positions(app, &assignments);
    emit_jobs(app);
    list(app)
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

    // Active jobs first in queue order (#202), then history newest first; the
    // job id breaks ties deterministically. Downloads and Big Picture share
    // this order.
    let mut active: Vec<(u32, String, String, InstallJob)> = Vec::new();
    let mut history: Vec<(String, String, InstallJob)> = Vec::new();
    for (created_at, job_id, job) in merged {
        if job.phase.is_terminal() {
            history.push((created_at, job_id, job));
        } else {
            active.push((
                job.queue_position.unwrap_or(u32::MAX),
                created_at,
                job_id,
                job,
            ));
        }
    }
    active.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
    });
    history.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    Ok(active
        .into_iter()
        .map(|(_, _, _, job)| job)
        .chain(history.into_iter().map(|(_, _, job)| job))
        .collect())
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
    let (entry, orphan) = {
        let jobs = state
            .jobs
            .lock()
            .map_err(|_| "The install queue is unavailable; restart Ludex.".to_string())?;
        let Some(entry) = jobs.get(job_id) else {
            return Err(format!("Unknown install job: {job_id}"));
        };
        entry.cancel.cancel();
        // A queued or paused job has no worker watching the token, and the
        // scheduler may start it at any moment. Claim it here (the same
        // `running` swap the scheduler uses) so exactly one path finishes it.
        let phase = entry.snapshot().phase;
        let orphan = matches!(
            phase,
            InstallPhase::Queued | InstallPhase::WaitingForSpace | InstallPhase::Paused
        ) && !entry.running.swap(true, Ordering::SeqCst);
        if orphan {
            // Leave the queue before the cleanup: the scheduler only picks
            // `queued` jobs, so it cannot start this one mid-cancel.
            entry.update(|job| job.phase = InstallPhase::Cancelled);
        }
        (Arc::clone(entry), orphan)
    };
    if orphan {
        // Clean up the partial archive/staging folder and persist the row.
        let paths = planned_paths(&state, &entry);
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            finalize_cancelled(&handle, &entry, &paths).await;
            entry.running.store(false, Ordering::SeqCst);
        });
    }
    Ok(())
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
        InstallPhase::Queued | InstallPhase::WaitingForSpace => {
            entry.request_pause();
            entry.update(|job| {
                job.phase = InstallPhase::Paused;
                job.speed_bps = 0;
                job.prefetching = false;
                job.clear_reconnect();
                job.message = Some("Paused".to_string());
            });
            // Pausing gives the disk reservation back: another queued job may
            // use the room while this one waits for an explicit resume (#222).
            release_space(app, &entry);
            persist(app, &entry);
            emit_jobs(app);
            // A paused job no longer blocks the head of the queue (#221).
            dispatch(app);
            Ok(entry.snapshot())
        }
        InstallPhase::Downloading | InstallPhase::Verifying => {
            if snapshot.stream && !snapshot.resumable {
                // A stream job from an older build may have no checkpoint at
                // all: pausing it would throw away the work (#203).
                return Err(
                    "This stream install has no checkpoint yet and cannot be paused. Wait for the first files, or cancel and retry."
                        .to_string(),
                );
            }
            entry.request_pause();
            // The worker flushes the `.part`/entry file and emits the paused state.
            Ok(entry.snapshot())
        }
        _ => Err(
            "This install is not downloading, so it cannot be paused. Wait for it to finish or cancel it."
                .to_string(),
        ),
    }
}

/// Skip the current retry backoff of a waiting download (#212). The worker
/// wakes immediately, keeps the same job id and the same attempt budget.
pub fn retry_now(app: &AppHandle, job_id: &str) -> Result<InstallJob, String> {
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

    if !entry.snapshot().reconnecting {
        return Err("This download is not waiting to retry.".to_string());
    }
    entry.update(|job| {
        job.next_retry_at = None;
        job.message = Some("Retrying now".to_string());
    });
    entry.request_retry_now();
    let job = entry.snapshot();
    let _ = app.emit("install://progress", &job);
    tracing::info!(job_id = %job_id, attempt = job.attempt, "retry now requested");
    Ok(job)
}

/// Resume any resumable job — paused, failed, interrupted, cancelled, or from a
/// previous app run — always with the same job id (#205/#208).
///
/// `mode` only changes the transport: `Download` is the explicit "retry as
/// download", `Stream` the explicit "restart stream". Both reuse the row.
pub fn resume_with_mode(
    app: &AppHandle,
    job_id: &str,
    mode: ResumeMode,
) -> Result<InstallJob, String> {
    let state = app.state::<AppState>();
    let in_memory = {
        let jobs = state
            .jobs
            .lock()
            .map_err(|_| "The install queue is unavailable; restart Ludex.".to_string())?;
        jobs.get(job_id).cloned()
    };

    if let Some(entry) = in_memory {
        let snapshot = entry.snapshot();
        if !matches!(
            snapshot.phase,
            InstallPhase::Paused | InstallPhase::Error | InstallPhase::Cancelled
        ) {
            // Queued/downloading/… is already active, or done: nothing to do.
            return Ok(snapshot);
        }
        if snapshot.phase == InstallPhase::Cancelled {
            // A cancelled job's cancel token is spent; rebuild a fresh entry
            // from its own parameters but keep the same id and checkpoint.
            let rebuilt = Arc::new(rebuild_entry(
                &entry,
                mode,
                entry.staging_path(),
                entry.stream_completed(),
            ));
            return resume_entry(app, rebuilt, mode, ResumeOrigin::Manual);
        }
        let entry =
            if mode == ResumeMode::Keep || mode.stream_flag(snapshot.stream) == snapshot.stream {
                entry
            } else {
                Arc::new(rebuild_entry(
                    &entry,
                    mode,
                    entry.staging_path(),
                    entry.stream_completed(),
                ))
            };
        return resume_entry(app, entry, mode, ResumeOrigin::Manual);
    }

    // A job from a previous app run: rebuild it from the persisted row.
    let (record, params_record) = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        let record = db::get_install_job(&connection, job_id)?.ok_or_else(|| {
            format!("Unknown install job: {job_id}. It was removed from Downloads history.")
        })?;
        let params = db::get_install_job_params(&connection, job_id)?.ok_or_else(|| {
            format!(
                "Install job {job_id} was recorded by an older Ludex build and cannot be resumed. Start the install again from the game page."
            )
        })?;
        (record, params)
    };

    let token = state.settings_snapshot().token.clone();
    let entry = Arc::new(entry_from_persisted(record, params_record, token, mode)?);
    resume_entry(app, entry, mode, ResumeOrigin::Manual)
}

/// Picks a failed/paused job of the same version so `install_game` doubles as
/// "retry" instead of inserting a second history row (#205).
fn adoptable_job(state: &AppState, params: &InstallParams, stream: bool) -> Option<Arc<JobEntry>> {
    let game_id = params.game_id;
    let version_id = params.version_id;
    {
        let jobs = state.jobs.lock().ok()?;
        if let Some(entry) = jobs.values().find(|entry| {
            entry.params.game_id == game_id
                && entry.params.version_id == version_id
                && entry.snapshot().stream == stream
                && matches!(
                    entry.snapshot().phase,
                    InstallPhase::Paused | InstallPhase::Error | InstallPhase::Cancelled
                )
        }) {
            return Some(Arc::clone(entry));
        }
    }

    let (record, params_record) = {
        let connection = state.db.lock().ok()?;
        let record = db::latest_resumable_job_for_version(&connection, game_id, version_id, stream)
            .ok()
            .flatten()?;
        let params_record = db::get_install_job_params(&connection, &record.job_id)
            .ok()
            .flatten()?;
        (record, params_record)
    };
    // A different install root means a different request: never adopt it.
    if let (Some(stored), Some(requested)) = (
        params_record.install_dir.as_deref(),
        params.install_dir.as_deref(),
    ) {
        if Path::new(stored) != Path::new(requested) {
            return None;
        }
    }
    let token = state.settings_snapshot().token.clone();
    let entry = entry_from_persisted(record, params_record, token, ResumeMode::Keep).ok()?;
    Some(Arc::new(entry))
}

/// Rebuilds a `JobEntry` from the persisted job and its parameters (#208).
///
/// The job id, the `.part` path, the staging folder and the stream checkpoint
/// are preserved, so the worker continues exactly where the previous run
/// stopped. Only `done` jobs are refused: a finished install has nothing to
/// resume.
pub(crate) fn entry_from_persisted(
    record: db::InstallJobRecord,
    params_record: db::InstallJobParamsRecord,
    token: Option<String>,
    mode: ResumeMode,
) -> Result<JobEntry, String> {
    if record.phase == InstallPhase::Done.as_str() {
        return Err("This install already finished. There is nothing to resume.".to_string());
    }
    let created_at = record.created_at.clone();
    let mut params = InstallParams::from_record(params_record, token);
    params.stream = Some(mode.stream_flag(record.stream));
    let mut job = InstallJob::from_record(record.clone());
    job.phase = InstallPhase::Queued;
    job.speed_bps = 0;
    job.stream = params.stream.unwrap_or(false);
    job.message = Some("Resuming".to_string());
    Ok(JobEntry {
        params,
        cancel: CancellationToken::new(),
        retry_now: tokio::sync::Notify::new(),
        pause: std::sync::Mutex::new(CancellationToken::new()),
        running: AtomicBool::new(false),
        job: std::sync::Mutex::new(job),
        created_at,
        part_path: std::sync::Mutex::new(record.part_path),
        install_path: std::sync::Mutex::new(record.install_path),
        staging_path: std::sync::Mutex::new(record.staging_path),
        stream_completed: std::sync::Mutex::new(record.stream_completed),
    })
}

/// Copies an in-memory entry under a new transport mode, keeping its id and
/// checkpoint paths.
fn rebuild_entry(
    entry: &JobEntry,
    mode: ResumeMode,
    staging: Option<String>,
    completed: Option<String>,
) -> JobEntry {
    let snapshot = entry.snapshot();
    let mut params = entry.params.clone();
    params.stream = Some(mode.stream_flag(snapshot.stream));
    let mut job = snapshot;
    job.stream = params.stream.unwrap_or(false);
    job.phase = InstallPhase::Queued;
    job.speed_bps = 0;
    job.message = Some("Resuming".to_string());
    JobEntry {
        params,
        cancel: CancellationToken::new(),
        retry_now: tokio::sync::Notify::new(),
        pause: std::sync::Mutex::new(CancellationToken::new()),
        running: AtomicBool::new(false),
        job: std::sync::Mutex::new(job),
        created_at: entry.created_at.clone(),
        part_path: std::sync::Mutex::new(entry.stored_paths().0),
        install_path: std::sync::Mutex::new(entry.stored_paths().1),
        staging_path: std::sync::Mutex::new(staging),
        stream_completed: std::sync::Mutex::new(completed),
    }
}

/// Inserts (or replaces) a resumable entry and starts its worker.
fn resume_entry(
    app: &AppHandle,
    entry: Arc<JobEntry>,
    mode: ResumeMode,
    origin: ResumeOrigin,
) -> Result<InstallJob, String> {
    let state = app.state::<AppState>();
    let job_id = entry.snapshot().job_id.clone();
    let game_id = entry.params.game_id;
    let version_id = entry.params.version_id;

    // One active job per game version: focus the running one instead of
    // starting a second (#205).
    let mut jobs = state
        .jobs
        .lock()
        .map_err(|_| "The install queue is unavailable; restart Ludex.".to_string())?;
    if let Some(active) = jobs.values().find(|candidate| {
        candidate.snapshot().job_id != job_id
            && candidate.params.game_id == game_id
            && candidate.params.version_id == version_id
            && !candidate.snapshot().phase.is_terminal()
    }) {
        return Ok(active.snapshot());
    }
    jobs.insert(job_id.clone(), Arc::clone(&entry));
    drop(jobs);

    // #234: an explicit Resume is a fresh decision by the user, so the job gets
    // its full two-strike budget again. The startup path never resets it.
    if origin == ResumeOrigin::Manual {
        reset_resume_failures_for(app, &job_id);
    }

    // An explicit transport change restarts from zero: drop the checkpoint the
    // new mode cannot use (the classic path then downloads the archive).
    if mode != ResumeMode::Keep && entry.stream_completed().is_some() {
        clear_stream_checkpoint(app, &entry);
        entry.set_stream_completed(None);
    }

    let previous_phase = entry.snapshot().phase;
    // A paused job kept its slot; a cancelled, failed or previous-session job
    // has none and re-joins at the end of the queue (#202).
    let had_position = entry.snapshot().queue_position.is_some();
    entry.clear_pause();
    entry.update(|job| {
        job.phase = InstallPhase::Queued;
        job.speed_bps = 0;
        job.clear_reconnect();
        job.message = Some(if job.bytes_done > 0 {
            format!("Resuming from {}", human_bytes(job.bytes_done))
        } else {
            "Resuming".to_string()
        });
    });
    if !had_position {
        let position = next_queue_position(&queue_snapshot(&state));
        entry.update(|job| job.queue_position = Some(position));
    }
    compact_queue(app);
    persist(app, &entry);
    emit_jobs(app);
    let snapshot = entry.snapshot();
    tracing::info!(
        job_id = %job_id,
        from = ?previous_phase,
        to = "queued",
        mode = ?mode,
        stream = snapshot.stream,
        resumable = snapshot.resumable,
        entries_done = ?snapshot.entries_done,
        entries_total = ?snapshot.entries_total,
        bytes_done = snapshot.bytes_done,
        "resuming install job"
    );
    dispatch(app);
    Ok(entry.snapshot())
}

pub(crate) fn staging_gc_targets(app: &AppHandle) -> (Vec<PathBuf>, HashSet<PathBuf>) {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let mut roots = vec![settings.install_path()];
    roots.extend(
        settings
            .locations
            .iter()
            .filter(|location| location.kind == settings::LocationKind::Install)
            .filter(|location| !location.path.trim().is_empty())
            .map(|location| PathBuf::from(location.path.trim())),
    );
    if let Ok(jobs) = state.jobs.lock() {
        for entry in jobs.values() {
            if let Some(dir) = entry
                .params
                .install_dir
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                roots.push(PathBuf::from(dir));
            }
        }
    }
    if let Ok(connection) = state.db.lock() {
        match db::all_install_job_params(&connection) {
            Ok(records) => roots.extend(
                records
                    .iter()
                    .filter_map(|record| record.install_dir.as_deref())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from),
            ),
            Err(error) => {
                tracing::warn!(error = %error, "could not read the persisted install folders for the staging sweep")
            }
        }
    }
    (roots, protected_staging(app))
}

/// Startup sweep of stale staging folders (#225).
///
/// Runs on a blocking thread after `auto_resume_on_startup` and
/// `rebuild_space_ledger`, so a folder owned by a job that is about to resume
/// is already known and protected.
pub(crate) fn sweep_stale_staging(app: &AppHandle) {
    let (roots, protected) = staging_gc_targets(app);
    let sweep = paths::cleanup_staging_dirs(&roots, &protected);
    tracing::info!(
        roots = ?roots,
        candidates = sweep.candidates,
        protected = sweep.protected,
        removed = sweep.removed,
        failures = sweep.failures,
        bytes_freed = sweep.bytes_freed,
        "staging sweep finished"
    );
}

async fn cleanup_success_staging(
    app: &AppHandle,
    entry: &JobEntry,
    job_paths: &JobPaths,
    final_path: &Path,
) {
    let Some(staging) = job_paths.staging.clone() else {
        return;
    };
    if staging == final_path {
        tracing::warn!(
            path = %staging.display(),
            "refusing to remove a staging folder that is the installed game"
        );
        return;
    }
    let Some(root) = staging.parent().map(Path::to_path_buf) else {
        return;
    };
    let slug = paths::slug_for_title(&entry.params.title);
    let protected: HashSet<String> = protected_staging(app)
        .iter()
        .map(|path| paths::path_key(path))
        .collect();
    let final_path = final_path.to_path_buf();
    let slug_for_cleanup = slug.clone();
    let task = tokio::task::spawn_blocking(move || {
        let candidates =
            success_staging_candidates(&staging, &final_path, &slug_for_cleanup, &protected);
        let mut removed = 0u64;
        let mut freed = 0u64;
        let mut failures = 0u64;
        for path in candidates {
            let bytes = paths::dir_size(&path);
            match std::fs::remove_dir_all(&path) {
                Ok(()) => {
                    removed += 1;
                    freed = freed.saturating_add(bytes);
                }
                Err(error) => {
                    failures += 1;
                    tracing::warn!(
                        path = %path.display(),
                        error = %error,
                        "could not remove a staging folder of a finished install"
                    );
                }
            }
        }
        (removed, freed, failures)
    });

    match task.await {
        Ok((removed, freed, failures)) => tracing::info!(
            job_id = %entry.snapshot().job_id,
            root = %root.display(),
            slug = %slug,
            removed,
            failures,
            bytes_freed = freed,
            "cleaned up the staging folder of a finished install"
        ),
        Err(error) => tracing::warn!(
            error = %error,
            "the staging cleanup task of a finished install failed"
        ),
    }
}

struct RunGuard<'a>(&'a AtomicBool);

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

async fn run(app: AppHandle, job_id: String, download_slot: SlotGuard) {
    let mut download_slot = Some(download_slot);
    // #234: everything a worker touches is ultimately data from the network or
    // an archive, so the whole job future is polled inside `catch_unwind`. A
    // panic becomes a failed job with its `.part`/staging intact; the process
    // (and therefore every other download and the UI) survives.
    let outcome =
        crate::crash_guard::guard_worker(run_job(app.clone(), job_id.clone(), &mut download_slot))
            .await;
    // Both slots are free again: start the next download in queue order. This
    // also picks up a resume that landed while the worker was finishing a
    // pause, so a job never stays queued without a worker.
    drop(download_slot);
    if let Err(panic) = outcome {
        contain_worker_panic(&app, &job_id, &panic);
    }
    dispatch(&app);
}

/// Turn a contained worker panic into a normal job failure (#234).
///
/// The `.part` file and the staging folder are kept exactly as they were: the
/// next manual Resume continues from them. `RunGuard` has already cleared the
/// `running` flag while the stack unwound, and every `JobEntry` lock recovers
/// from poisoning, so the registry stays usable.
fn contain_worker_panic(app: &AppHandle, job_id: &str, panic: &crate::crash_guard::PanicDetails) {
    let state = app.state::<AppState>();
    state.space.release(job_id);
    let entry = match state.jobs.lock() {
        Ok(jobs) => jobs.get(job_id).cloned(),
        Err(poisoned) => poisoned.into_inner().get(job_id).cloned(),
    };
    let Some(entry) = entry else {
        tracing::error!(
            job_id = %job_id,
            message = %panic.describe(),
            "an install worker panicked after its job left the queue"
        );
        return;
    };
    let job = entry.snapshot();
    let (part_path, _) = entry.stored_paths();
    tracing::error!(
        job_id = %job_id,
        game_id = job.game_id,
        version_id = job.version_id,
        phase = ?job.phase,
        part_path = ?part_path,
        bytes_done = job.bytes_done,
        transport = if job.stream { "stream" } else { "archive" },
        message = %panic.message,
        panic_location = ?panic.location,
        "install worker panicked; the job is failed and the partial file is kept"
    );
    // #171/#234: the crash report names the job, so a field occurrence is
    // diagnosable without asking the user to reinstall.
    state.diagnostics.record(
        "panic",
        &format!(
            "install worker panic (job {job_id}, game {}, version {}): {}",
            job.game_id,
            job.version_id,
            panic.describe()
        ),
        Some(job.game_id),
        None,
        None,
        panic.location.as_deref(),
        &[],
    );
    finalize_error(
        app,
        &entry,
        format!(
            "The download crashed: {}. The partial file was kept — retry it manually.",
            panic.describe()
        ),
    );
}

async fn run_job(app: AppHandle, job_id: String, download_slot: &mut Option<SlotGuard>) {
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

    // The network slot is released as soon as the transfer ends (#221); the
    // extractor is taken by `execute` right before hashing/extraction.
    let mut extract_slot: Option<SlotGuard> = None;

    let mut job_paths = planned_paths(&state, &entry);
    entry.set_part_path(
        job_paths
            .part
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
    );

    // The scheduler already gave this job the download slot. A cancel or pause
    // while queued moves it out of the queue without touching the disk.
    if entry.cancel.is_cancelled() {
        drop(download_slot.take());
        finalize_cancelled(&app, &entry, &job_paths).await;
        return;
    }
    if entry.pause_token().is_cancelled() {
        drop(download_slot.take());
        finalize_paused(&app, &entry, &job_paths).await;
        return;
    }

    // #288: the job now owns a real transfer/extraction/verification; hold the
    // suspend inhibitor until this function returns (the guard releases on
    // every path, including the panic unwind handled above).
    let _awake = crate::inhibit::hold(crate::inhibit::Reason::install(
        &job_id,
        &entry.params.title,
    ));

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
                if matches!(
                    phase,
                    InstallPhase::Downloading | InstallPhase::Verifying | InstallPhase::Extracting
                ) {
                    let _ = app.emit("install://progress", entry.snapshot());
                }
            }
        })
    };

    let result = super::commit::commit(
        &app,
        &entry,
        &mut job_paths,
        download_slot,
        &mut extract_slot,
    )
    .await;
    heartbeat.abort();
    // Whatever happened, this job no longer holds the extractor or a disk
    // reservation (#221/#222).
    drop(extract_slot.take());
    drop(download_slot.take());
    state.space.release(&entry.snapshot().job_id);

    if result.is_err() {
        // Resumable stream installs keep staging and their checkpoint so the
        // next attempt continues from the completed entries (#115). Everything
        // else discards the partial tree.
        if !entry.snapshot().resumable {
            if let Some(staging) = job_paths.staging.as_deref() {
                let _ = tokio::fs::remove_dir_all(staging).await;
            }
        }
        // Cancelling discards the partial archive; pausing keeps it so the
        // next attempt resumes with HTTP Range.
        if matches!(&result, Err(Failure::Cancelled)) {
            if let Some(part) = job_paths.part.as_deref() {
                let _ = tokio::fs::remove_file(part).await;
                crate::archive_verify::clear_digest(part);
            }
        }
    }

    match result {
        Ok(outcome) => {
            let install_path = outcome.install_path;
            // The staging folder was moved into place: the per-entry and
            // per-file checkpoints are no longer needed.
            clear_stream_checkpoint(&app, &entry);
            clear_file_checkpoint(&app, &entry.snapshot().job_id);
            // #225: a finished install must leave exactly one folder. Remove
            // its own staging folder (the per-entry commit moves files out but
            // a failed move/copy can leave it behind) and any stale sibling
            // staging folder of the same game. A cleanup failure is logged,
            // never fatal: the game is already in its final folder.
            cleanup_success_staging(&app, &entry, &job_paths, &install_path).await;
            entry.set_staging_path(None);
            entry.update(|job| {
                job.phase = InstallPhase::Done;
                job.speed_bps = 0;
                job.prefetching = false;
                job.clear_reconnect();
                job.bytes_done = job.bytes_total.unwrap_or(job.bytes_done);
                // #214: a setup-only archive finishes with no executable. The
                // guided flow in the Installer panel runs the setup and records
                // the game binary it produces.
                job.message = Some(match &outcome.pending_setup {
                    Some(repack) => format!(
                        "Installed. This archive contains a setup program: run {} to finish installing.",
                        repack.setup
                    ),
                    None => "Installed".to_string(),
                });
                job.install_path = Some(install_path.to_string_lossy().to_string());
                job.resumable = false;
                // #202: a finished job leaves the queue without a gap.
                job.queue_position = None;
            });
            persist(&app, &entry);
            compact_queue(&app);
            // A finished install cannot be resumed: drop the request row so it
            // never offers a stale "Restart" (#208). Done after the last
            // persist, which would otherwise write it back.
            {
                let state = app.state::<AppState>();
                if let Ok(connection) = state.db.lock() {
                    let job_id = entry.snapshot().job_id;
                    if let Err(error) = db::delete_install_job_params(&connection, &job_id) {
                        tracing::warn!(error = %error, "could not delete the install parameters");
                    }
                };
            }
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
        Err(Failure::Offline(message)) => {
            finalize_offline(&app, &entry, &job_paths, message).await;
        }
        Err(Failure::StreamUnavailable(message)) => {
            finalize_error(&app, &entry, message);
        }
    }
}

/// Stop a job while keeping its partial archive. The next `install_game` or
/// `resume_install` continues from `bytesDone` with a Range request.
async fn finalize_paused(app: &AppHandle, entry: &JobEntry, job_paths: &JobPaths) {
    finalize_pause(app, entry, job_paths, "Paused".to_string()).await;
}

/// Pause a job whose library mount went away (#290). The staging folder and the
/// `.part` are always kept: replugging the drive and resuming continues exactly
/// where the transfer stopped.
async fn finalize_offline(
    app: &AppHandle,
    entry: &JobEntry,
    job_paths: &JobPaths,
    message: String,
) {
    tracing::warn!(
        job_id = %entry.snapshot().job_id,
        %message,
        "install paused: the library mount is offline"
    );
    finalize_pause(app, entry, job_paths, message).await;
}

async fn finalize_pause(app: &AppHandle, entry: &JobEntry, job_paths: &JobPaths, message: String) {
    // A cancel that raced with the pause must win: the user asked for the
    // partial archive to be discarded.
    if entry.cancel.is_cancelled() {
        finalize_cancelled(app, entry, job_paths).await;
        return;
    }
    // A resumable stream install keeps its staging folder and checkpoint: the
    // next `resume_install` continues from the completed entries (#115).
    if !entry.snapshot().resumable {
        if let Some(staging) = job_paths.staging.as_deref() {
            let _ = tokio::fs::remove_dir_all(staging).await;
        }
    }
    release_space(app, entry);
    entry.update(|job| {
        job.phase = InstallPhase::Paused;
        job.speed_bps = 0;
        job.prefetching = false;
        job.clear_reconnect();
        job.message = Some(message);
        // An offline pause is resumable whatever the transport: the partial
        // archive is on the (temporarily absent) volume.
        job.resumable = true;
    });
    persist(app, entry);
    let _ = app.emit("install://progress", entry.snapshot());
    emit_jobs(app);
    // The slots are free: the next queued install can start (#221).
    dispatch(app);
    tracing::info!(job_id = %entry.snapshot().job_id, "install paused");
}

async fn finalize_cancelled(app: &AppHandle, entry: &JobEntry, job_paths: &JobPaths) {
    // Cancelling a resumable stream install stops between entries and keeps
    // the files that were already written, so a later install of the same
    // version does not download them again (#115). With nothing completed
    // there is nothing worth keeping: discard the folder and the checkpoint.
    let keep_staging = entry.snapshot().resumable
        && entry
            .stream_completed()
            .as_deref()
            .map(crate::stream_install::CompletedEntries::from_hex)
            .map(|completed| completed.count() > 0)
            .unwrap_or(false);
    if keep_staging {
        tracing::info!(
            job_id = %entry.snapshot().job_id,
            "install cancelled; completed streamed entries are kept for the next attempt"
        );
    } else if let Some(staging) = job_paths.staging.as_deref() {
        let _ = tokio::fs::remove_dir_all(staging).await;
        clear_stream_checkpoint(app, entry);
        clear_file_checkpoint(app, &entry.snapshot().job_id);
    }
    if let Some(part) = job_paths.part.as_deref() {
        let _ = tokio::fs::remove_file(part).await;
        crate::archive_verify::clear_digest(part);
    }
    entry.set_part_path(None);
    let message = if keep_staging {
        "Install cancelled; the files already downloaded are kept for the next attempt"
    } else {
        "Install cancelled"
    };
    release_space(app, entry);
    entry.update(|job| {
        job.phase = InstallPhase::Cancelled;
        job.speed_bps = 0;
        job.prefetching = false;
        job.clear_reconnect();
        job.message = Some(message.to_string());
        // #202: a cancelled job leaves the queue without a gap.
        job.queue_position = None;
    });
    persist(app, entry);
    compact_queue(app);
    let _ = app.emit("install://progress", entry.snapshot());
    emit_jobs(app);
    dispatch(app);
    tracing::info!(job_id = %entry.snapshot().job_id, "install cancelled");
}

/// Removes the per-entry manifest and clears the completed bitset.
fn clear_stream_checkpoint(app: &AppHandle, entry: &JobEntry) {
    let state = app.state::<AppState>();
    let job_id = entry.snapshot().job_id;
    if let Ok(connection) = state.db.lock() {
        if let Err(error) = db::delete_stream_manifest(&connection, &job_id) {
            tracing::warn!(error = %error, "could not delete the stream manifest");
        }
    }
    entry.set_stream_completed(None);
}

fn finalize_error(app: &AppHandle, entry: &JobEntry, message: String) {
    release_space(app, entry);
    entry.update(|job| {
        job.phase = InstallPhase::Error;
        job.speed_bps = 0;
        job.prefetching = false;
        job.clear_reconnect();
        job.message = Some(message.clone());
        // #202: a failed job leaves the queue without a gap.
        job.queue_position = None;
    });
    persist(app, entry);
    compact_queue(app);
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
    dispatch(app);
    tracing::warn!(job_id = %job.job_id, error = %message, "install failed");
}

/// Update a job row, persist it and emit `install://progress`.
///
/// The single place that maps a stage transition onto the wire shape; every
/// stage uses it so "persist then emit" cannot drift between call sites.
pub(super) fn set_progress(
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
        job.clear_reconnect();
    });
    persist(app, entry);
    let _ = app.emit("install://progress", entry.snapshot());
}

/// Drop this job's disk reservation (#222). Idempotent: the worker calls it on
/// every exit path and the cancel/pause finalizers call it again.
pub(crate) fn release_space(app: &AppHandle, entry: &JobEntry) {
    let state = app.state::<AppState>();
    state.space.release(&entry.snapshot().job_id);
}

pub(crate) fn persist(app: &AppHandle, entry: &JobEntry) {
    let state = app.state::<AppState>();
    let job = entry.snapshot();
    let (part_path, install_path) = entry.stored_paths();
    let staging_path = entry.staging_path();
    // A job can resume when it is paused, keeps a `.part` file, or has a
    // per-entry checkpoint. This flag drives the UI label: "Resume" only when
    // the action really continues, "Restart" otherwise (#205/#208).
    let resumable = job.resumable
        || job.phase == InstallPhase::Paused
        || (job.phase.is_terminal()
            && part_path
                .as_deref()
                .is_some_and(|path| std::path::Path::new(path).is_file()));
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
        staging_path,
        error: if job.phase == InstallPhase::Error {
            job.message.clone()
        } else {
            None
        },
        stream: job.stream,
        resumable,
        stream_completed: entry.stream_completed(),
        entries_total: job.entries_total.map(|value| value as i64),
        queue_position: job.queue_position.map(i64::from),
        verified_sha256: job.verified_sha256.clone(),
        created_at: entry.created_at.clone(),
        updated_at: now_rfc3339(),
    };

    if let Ok(connection) = state.db.lock() {
        if let Err(error) = db::upsert_install_job(&connection, &record) {
            tracing::warn!(error = %error, "could not persist the install job");
        }
        // Immutable request parameters, kept so `resume_install` can rebuild a
        // worker after an app restart (#208). Cheap `ON CONFLICT DO NOTHING`.
        let mut params = entry.params.to_record(&job.job_id, &entry.created_at);
        params.stream = job.stream;
        if let Err(error) = db::upsert_install_job_params(&connection, &params) {
            tracing::warn!(error = %error, "could not persist the install parameters");
        }
    };
}

pub(crate) fn auto_resume_on_startup(app: &AppHandle) {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    // #234: support escape hatch. Nothing below runs, so no worker touches a
    // suspicious `.part` and the app always opens on a usable Downloads screen.
    if crate::crash_guard::safe_mode_requested_now() {
        tracing::warn!(
            env = crate::crash_guard::SAFE_MODE_ENV,
            flag = crate::crash_guard::SAFE_MODE_FLAG,
            "safe mode: automatic download resume is disabled for this launch"
        );
        return;
    }
    if !settings.auto_resume_downloads {
        tracing::info!("auto-resume of interrupted downloads is disabled");
        return;
    }

    let (records, failures) = {
        let Ok(connection) = state.db.lock() else {
            tracing::warn!("could not read interrupted downloads: database unavailable");
            return;
        };
        // Markers describe the *previous* launch: reconcile them before this
        // one writes its own.
        let failures = match db::reconcile_resume_markers(&connection, &now_rfc3339()) {
            Ok(failures) => failures,
            Err(error) => {
                tracing::warn!(error = %error, "could not reconcile the startup resume markers");
                Vec::new()
            }
        };
        let records = match db::interrupted_install_jobs(
            &connection,
            INTERRUPTED_MESSAGE,
            LEGACY_INTERRUPTED_MESSAGE,
        ) {
            Ok(records) => records,
            Err(error) => {
                tracing::warn!(error = %error, "could not list interrupted downloads");
                return;
            }
        };
        (records, failures)
    };
    if records.is_empty() {
        return;
    }

    let token = settings.token.clone();
    let mut resumed: Vec<String> = Vec::new();
    let mut parked = 0usize;
    for record in records {
        let job_id = record.job_id.clone();
        let previous_failures = failures
            .iter()
            .find(|(failed, _)| *failed == job_id)
            .map(|(_, failures)| *failures)
            .unwrap_or(0);
        // #234: two consecutive unclean launches mean this job (or its partial
        // file) is what kills the app. Park it; only a human Resume restarts it.
        if crate::crash_guard::startup_resume_decision(previous_failures)
            == crate::crash_guard::StartupResume::Park
        {
            park_repeated_crash(app, &record, previous_failures);
            parked += 1;
            continue;
        }

        let already_active = state
            .jobs
            .lock()
            .map(|jobs| jobs.contains_key(&job_id))
            .unwrap_or(false);
        if already_active {
            continue;
        }
        let params = {
            let Ok(connection) = state.db.lock() else {
                break;
            };
            db::get_install_job_params(&connection, &job_id)
                .ok()
                .flatten()
        };
        let Some(params) = params else {
            tracing::warn!(
                job_id = %job_id,
                "an interrupted download has no stored parameters; leaving it for a manual retry"
            );
            continue;
        };

        // #234: everything a field report needs, before the worker starts. The
        // next crash must be diagnosable from the log alone, without a reinstall.
        tracing::info!(
            job_id = %job_id,
            game_id = record.game_id,
            version_id = record.version_id,
            phase = %record.phase,
            bytes_done = record.bytes_done,
            bytes_total = ?record.bytes_total,
            transport = if record.stream { "stream" } else { "archive" },
            part_path = ?record.part_path,
            part_bytes = record
                .part_path
                .as_deref()
                .and_then(|path| std::fs::metadata(path).ok())
                .map(|metadata| metadata.len()),
            staging_path = ?record.staging_path,
            resumable = record.resumable,
            last_error = ?record.error,
            resume_failures = previous_failures,
            "auto-resuming an interrupted download"
        );

        // #234: the marker is committed *before* the worker exists. A row still
        // present at the next launch is the evidence of an unclean startup.
        let marked = match state.db.lock() {
            Ok(connection) => match db::set_resume_marker(&connection, &job_id, &now_rfc3339()) {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(job_id = %job_id, error = %error, "could not write the startup resume marker");
                    false
                }
            },
            Err(_) => false,
        };

        match entry_from_persisted(record, params, token.clone(), ResumeMode::Keep) {
            Ok(entry) => {
                match resume_entry(
                    app,
                    Arc::new(entry),
                    ResumeMode::Keep,
                    ResumeOrigin::Startup,
                ) {
                    Ok(_) => {
                        if marked {
                            resumed.push(job_id.clone());
                        }
                    }
                    Err(error) => {
                        if marked {
                            clear_resume_marker(app, &job_id);
                        }
                        tracing::warn!(job_id = %job_id, error = %error, "could not auto-resume a download")
                    }
                }
            }
            Err(error) => {
                if marked {
                    clear_resume_marker(app, &job_id);
                }
                tracing::warn!(job_id = %job_id, error = %error, "could not rebuild an interrupted download")
            }
        }
    }
    tracing::info!(
        resumed = resumed.len(),
        parked,
        "auto-resumed interrupted downloads"
    );
    if !resumed.is_empty() {
        schedule_resume_health_check(app, resumed);
    }
}

/// Drop one marker whose resume never actually started (#234).
fn clear_resume_marker(app: &AppHandle, job_id: &str) {
    let state = app.state::<AppState>();
    if let Ok(connection) = state.db.lock() {
        if let Err(error) = db::delete_resume_marker(&connection, job_id) {
            tracing::warn!(job_id = %job_id, error = %error, "could not clear the startup resume marker");
        }
    };
}

/// Reset the automatic-resume failure streak for one job (#234).
fn reset_resume_failures_for(app: &AppHandle, job_id: &str) {
    let state = app.state::<AppState>();
    if let Ok(connection) = state.db.lock() {
        if let Err(error) = db::reset_resume_failures(&connection, job_id) {
            tracing::warn!(job_id = %job_id, error = %error, "could not reset the automatic resume failure count");
        }
    };
}

/// Park a job the circuit breaker refuses to resume (#234).
///
/// The row becomes `error` with [`RESUME_DISABLED_MESSAGE`], which
/// `interrupted_install_jobs` and therefore this function never pick up again.
/// `.part`, staging and the stored parameters stay untouched: a manual Resume
/// still continues the download, and the UI shows the error with that reason.
fn park_repeated_crash(app: &AppHandle, record: &db::InstallJobRecord, failures: u32) {
    let state = app.state::<AppState>();
    let job_id = record.job_id.as_str();
    let parked = match state.db.lock() {
        Ok(connection) => {
            match db::park_install_job(&connection, job_id, RESUME_DISABLED_MESSAGE, &now_rfc3339())
            {
                Ok(parked) => parked,
                Err(error) => {
                    tracing::warn!(job_id = %job_id, error = %error, "could not park the interrupted download");
                    false
                }
            }
        }
        Err(_) => false,
    };
    tracing::error!(
        job_id = %job_id,
        game_id = record.game_id,
        version_id = record.version_id,
        failures,
        part_path = ?record.part_path,
        bytes_done = record.bytes_done,
        parked,
        "automatic resume disabled after repeated crashes; the partial file is kept"
    );
    if parked {
        // #171/#234: a crash-loop park is a support case. Keep a local report
        // naming the job so the bundle explains the state without a reinstall.
        state.diagnostics.record(
            "install",
            &format!(
                "auto-resume parked job {job_id} (game {}, version {}) after {failures} unclean startups: {RESUME_DISABLED_MESSAGE}",
                record.game_id, record.version_id
            ),
            Some(record.game_id),
            None,
            None,
            None,
            &[],
        );
    }
}

/// A clean shutdown is not a crash (#234).
///
/// `RunEvent::Exit` runs on a normal quit, so any marker still around belongs
/// to a launch that ended cleanly before the 15 s health window. Drop it (and
/// its streak): a quick launch/quit cycle must never park a job.
pub(crate) fn clear_resume_markers_on_clean_exit(app: &AppHandle) {
    let state = app.state::<AppState>();
    if let Ok(connection) = state.db.lock() {
        match db::resume_markers(&connection) {
            Ok(markers) => {
                for marker in markers {
                    let _ = db::delete_resume_marker(&connection, &marker.job_id);
                    let _ = db::reset_resume_failures(&connection, &marker.job_id);
                }
            }
            Err(error) => {
                tracing::warn!(error = %error, "could not read the startup resume markers on exit")
            }
        }
    };
}

/// Clear the startup markers once the app proved it can stay up (#234).
///
/// Runs on a detached task so nothing on the startup path waits 15 s. A job
/// that reaches this point has not taken the process down, so its streak is
/// reset: only *consecutive* unclean launches count.
fn schedule_resume_health_check(app: &AppHandle, job_ids: Vec<String>) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(RESUMED_HEALTHY_AFTER).await;
        let state = handle.state::<AppState>();
        let mut cleared = 0usize;
        match state.db.lock() {
            Ok(connection) => {
                for job_id in &job_ids {
                    if let Err(error) = db::delete_resume_marker(&connection, job_id) {
                        tracing::warn!(job_id = %job_id, error = %error, "could not clear the startup resume marker");
                        continue;
                    }
                    if let Err(error) = db::reset_resume_failures(&connection, job_id) {
                        tracing::warn!(job_id = %job_id, error = %error, "could not reset the automatic resume failure count");
                    }
                    cleared += 1;
                }
            }
            Err(_) => {
                tracing::warn!("could not clear the startup resume markers: database unavailable");
                return;
            }
        }
        tracing::info!(
            cleared,
            healthy_after_secs = RESUMED_HEALTHY_AFTER.as_secs(),
            "startup resume markers cleared after a healthy launch"
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn persisted_record(job_id: &str, phase: &str) -> db::InstallJobRecord {
        db::InstallJobRecord {
            job_id: job_id.into(),
            game_id: 3,
            version_id: 4,
            phase: phase.into(),
            bytes_done: 100,
            bytes_total: Some(200),
            speed_bps: 0,
            part_path: Some("/tmp/.3-4.part".into()),
            install_path: Some("/games/elden-ring/1.10".into()),
            staging_path: Some("/games/.elden-ring.staging-old".into()),
            error: Some("interrupted".into()),
            stream: false,
            resumable: true,
            stream_completed: Some("0f".into()),
            entries_total: Some(8),
            queue_position: Some(1),
            verified_sha256: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:10:00Z".into(),
        }
    }

    fn persisted_params(job_id: &str) -> db::InstallJobParamsRecord {
        db::InstallJobParamsRecord {
            job_id: job_id.into(),
            game_id: 3,
            version_id: 4,
            title: "Elden Ring".into(),
            version: Some("1.10".into()),
            executable: Some("Game/eldenring.exe".into()),
            install_dir: Some("/games".into()),
            download_dir: Some("/downloads".into()),
            install_location_id: None,
            download_location_id: None,
            expected_bytes: Some(69_200_000_000),
            stream: false,
            archive_format: Some("zip".into()),
            replace_path: None,
            server_url: "https://ludex.example.com".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn persisted_job_resume_reuses_the_job_id_and_checkpoint() {
        let entry = entry_from_persisted(
            persisted_record("job-resume", "error"),
            persisted_params("job-resume"),
            Some("token-1".into()),
            ResumeMode::Keep,
        )
        .expect("persisted resume");

        let snapshot = entry.snapshot();
        assert_eq!(snapshot.job_id, "job-resume");
        assert_eq!(snapshot.phase, InstallPhase::Queued);
        assert!(snapshot.resumable, "a failed job with a .part can resume");
        assert_eq!(snapshot.bytes_done, 100);
        assert_eq!(
            snapshot.queue_position,
            Some(1),
            "an app-restart resume keeps its queue slot (#212)"
        );
        assert_eq!(
            entry.stored_paths().0.as_deref(),
            Some("/tmp/.3-4.part"),
            "the .part path must be preserved"
        );
        assert_eq!(
            entry.staging_path().as_deref(),
            Some("/games/.elden-ring.staging-old"),
            "the staging folder must be preserved"
        );
        assert_eq!(entry.stream_completed().as_deref(), Some("0f"));
        assert_eq!(entry.params.title, "Elden Ring");
        assert_eq!(entry.params.server_url, "https://ludex.example.com");
        assert_eq!(entry.params.token.as_deref(), Some("token-1"));
        assert_eq!(entry.params.expected_bytes, Some(69_200_000_000));

        // An explicit retry-as-download flips the transport, same job id.
        let download = entry_from_persisted(
            persisted_record("job-resume", "error"),
            persisted_params("job-resume"),
            None,
            ResumeMode::Download,
        )
        .expect("retry as download");
        assert!(!download.snapshot().stream);

        // A finished job has nothing to resume.
        assert!(entry_from_persisted(
            persisted_record("job-done", "done"),
            persisted_params("job-done"),
            None,
            ResumeMode::Keep,
        )
        .is_err());
    }
}
