//! Install pipeline: resolve the archive, download with resume, extract to a
//! staging folder (or copy a single-file game), detect the executable, move
//! into place and index the result.
//!
//! Jobs are persisted in SQLite so an interrupted download stays visible after
//! a restart and the next attempt resumes the `.part` file with HTTP Range.
//! Only one job runs at a time by default; the rest wait in the queued phase.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::db::{self, InstallManifestRecord, InstalledGame};
use crate::manifest;
use crate::paths;
use crate::settings::{self, LocalSettings, Mode};
use crate::state::{AppState, JobEntry};
use crate::util::{human_bytes, non_empty, now_rfc3339, short_id};

pub(crate) const INTERRUPTED_MESSAGE: &str =
    "Ludex closed before this install finished. Resuming automatically.";
/// The message v0.4.5 and earlier stamped on an interrupted row. Recognised by
/// the startup auto-resume so an upgrade does not strand an in-flight job.
pub(crate) const LEGACY_INTERRUPTED_MESSAGE: &str =
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
    /// True when the job has per-entry checkpoints (#115): a failed or paused
    /// install can resume from the files already in staging.
    pub resumable: bool,
    /// Completed / total file entries of a per-entry stream install.
    pub entries_done: Option<u32>,
    pub entries_total: Option<u32>,
    /// Entry currently being written, relative to the staging folder.
    pub current_entry: Option<String>,
    /// Files already present when this attempt started (#115).
    pub resumed_entries: Option<u32>,
    /// True while the download waits to reconnect after a stall or a dropped
    /// response (#195). The UI shows the resume point instead of a stale speed.
    pub reconnecting: bool,
    /// Connection attempt of the current download, starting at 1.
    pub attempt: u32,
    /// Last transport error, kept until the transfer completes.
    pub last_error: Option<String>,
    /// Milliseconds the connection was silent when the stall was detected.
    pub stalled_for_ms: Option<u64>,
    /// RFC 3339 time of the next automatic retry (#212). `None` when no retry
    /// is scheduled. The UI renders the countdown and a "Retry now" action.
    pub next_retry_at: Option<String>,
    /// HTTP status of the last response for this job (#212), kept for the
    /// diagnostics bundle. `None` before the first response.
    pub http_status: Option<u16>,
    /// True while a retry is parked because the OS reports no network (#212).
    pub waiting_for_network: bool,
    /// Position in the install queue (#202): the scheduler starts the lowest
    /// one when a slot frees. `None` for a finished job and for a row that
    /// predates the queue order.
    pub queue_position: Option<u32>,
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
        let completed = record
            .stream_completed
            .as_deref()
            .map(crate::stream_install::CompletedEntries::from_hex)
            .unwrap_or_default();
        // A job can resume when it has a real checkpoint: a persisted flag, a
        // completed stream entry, or a `.part`/staging path still on disk. The
        // UI labels the action "Resume" only in that case (#205/#208).
        let resumable = record.resumable
            || record.phase == "paused"
            || (record.stream && completed.count() > 0)
            || record
                .part_path
                .as_deref()
                .is_some_and(|path| Path::new(path).is_file());
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
            resumable,
            entries_done: resumable
                .then(|| completed.count() as u32)
                .filter(|count| *count > 0),
            entries_total: record
                .entries_total
                .map(|value| value.max(0) as u32)
                .filter(|value| *value > 0),
            current_entry: None,
            resumed_entries: None,
            reconnecting: false,
            attempt: 1,
            last_error: None,
            stalled_for_ms: None,
            next_retry_at: None,
            http_status: None,
            waiting_for_network: false,
            // #202: a finished job leaves the queue for good. An interrupted
            // job (error after a crash) keeps the slot recorded by the previous
            // session, so resuming restores the order instead of appending.
            queue_position: if phase.is_terminal() && phase != InstallPhase::Error {
                None
            } else {
                record
                    .queue_position
                    .and_then(|value| u32::try_from(value).ok())
            },
        }
    }

    /// Clear the transient reconnect state when the job leaves `downloading`.
    pub fn clear_reconnect(&mut self) {
        self.reconnecting = false;
        self.last_error = None;
        self.stalled_for_ms = None;
        self.next_retry_at = None;
        self.waiting_for_network = false;
    }
}

/// One active job as seen by the queue scheduler and the reorder commands
/// (#202). Positions are unique among active jobs; the running job keeps the
/// front slot and is never reordered silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueueJob {
    pub job_id: String,
    pub phase: InstallPhase,
    /// `None` for a job that has not been placed yet (a row written by an
    /// older build): it sorts last and gets a real position on the next pass.
    pub position: Option<u32>,
    pub created_at: String,
    /// True while a worker owns the job, even during the short window before
    /// the phase leaves `queued`.
    pub running: bool,
}

impl QueueJob {
    /// Waiting jobs are the reorderable part of the queue: they have not
    /// touched the disk yet, or they keep their checkpoint while paused.
    fn is_waiting(&self) -> bool {
        matches!(self.phase, InstallPhase::Queued | InstallPhase::Paused)
    }

    fn is_running(&self) -> bool {
        self.running
            || matches!(
                self.phase,
                InstallPhase::Downloading | InstallPhase::Extracting | InstallPhase::Finalizing
            )
    }

    /// Queue order: running jobs first, then waiting jobs by position.
    fn order_key(&self) -> (u8, u32, &str, &str) {
        (
            u8::from(!self.is_running()),
            self.position.unwrap_or(u32::MAX),
            self.created_at.as_str(),
            self.job_id.as_str(),
        )
    }
}

/// A queue move requested by Downloads or Big Picture (#202).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueMove {
    Top,
    Up,
    Down,
    /// 0-based index inside the waiting queue.
    Position(u32),
}

impl QueueMove {
    pub fn parse(action: &str, position: Option<u32>) -> Result<Self, String> {
        match action.trim() {
            "top" => Ok(QueueMove::Top),
            "up" => Ok(QueueMove::Up),
            "down" => Ok(QueueMove::Down),
            "position" => position.map(QueueMove::Position).ok_or_else(|| {
                "A position is required to move an install to a specific place in the queue."
                    .to_string()
            }),
            other => Err(format!(
                "Unknown queue action \"{other}\". Use \"top\", \"up\", \"down\" or \"position\"."
            )),
        }
    }
}

/// The next free position: one past the highest active position (#202).
pub(crate) fn next_queue_position(jobs: &[QueueJob]) -> u32 {
    jobs.iter()
        .filter_map(|job| job.position)
        .max()
        .map_or(0, |position| position + 1)
}

/// Renumbers the active queue `0..n` in queue order, so a finished, cancelled
/// or superseded job never leaves a gap.
pub(crate) fn plan_compact(jobs: &[QueueJob]) -> Vec<(String, u32)> {
    let mut ordered: Vec<&QueueJob> = jobs.iter().collect();
    ordered.sort_by(|left, right| left.order_key().cmp(&right.order_key()));
    ordered
        .into_iter()
        .enumerate()
        .map(|(index, job)| (job.job_id.clone(), index as u32))
        .collect()
}

/// The job the scheduler starts next: the lowest position among `queued` jobs
/// that no worker owns.
pub(crate) fn next_queued(jobs: &[QueueJob]) -> Option<String> {
    jobs.iter()
        .filter(|job| job.phase == InstallPhase::Queued && !job.running)
        .min_by(|left, right| left.order_key().cmp(&right.order_key()))
        .map(|job| job.job_id.clone())
}

/// Reorders the waiting queue and returns the compacted position of every
/// active job. Running and finished jobs are refused with a clear reason: a
/// running install is only stopped by an explicit pause or cancel.
pub(crate) fn plan_reorder(
    jobs: &[QueueJob],
    job_id: &str,
    action: QueueMove,
) -> Result<Vec<(String, u32)>, String> {
    let target = jobs
        .iter()
        .find(|job| job.job_id == job_id)
        .ok_or_else(|| format!("Unknown install job: {job_id}"))?;
    if target.is_running() {
        return Err(
            "This install is running. Pause or cancel it before reordering the queue.".to_string(),
        );
    }
    if !target.is_waiting() {
        return Err("This install already finished and cannot be reordered.".to_string());
    }

    let mut waiting: Vec<&QueueJob> = jobs.iter().filter(|job| job.is_waiting()).collect();
    waiting.sort_by(|left, right| left.order_key().cmp(&right.order_key()));
    let current = waiting
        .iter()
        .position(|job| job.job_id == job_id)
        .expect("the target is a waiting job");
    let target_index = match action {
        QueueMove::Top => 0,
        QueueMove::Up => current.saturating_sub(1),
        QueueMove::Down => (current + 1).min(waiting.len().saturating_sub(1)),
        QueueMove::Position(position) => {
            let position = position as usize;
            if position >= waiting.len() {
                return Err(format!(
                    "Position {position} is out of range: the queue has {} waiting install(s).",
                    waiting.len()
                ));
            }
            position
        }
    };
    if target_index != current {
        let moved = waiting.remove(current);
        waiting.insert(target_index, moved);
    }

    // Running jobs keep the front slots; the waiting queue follows in the
    // order chosen here.
    let mut ordered: Vec<String> = {
        let mut running: Vec<&QueueJob> = jobs.iter().filter(|job| job.is_running()).collect();
        running.sort_by(|left, right| left.order_key().cmp(&right.order_key()));
        running.into_iter().map(|job| job.job_id.clone()).collect()
    };
    ordered.extend(waiting.into_iter().map(|job| job.job_id.clone()));
    Ok(ordered
        .into_iter()
        .enumerate()
        .map(|(index, job_id)| (job_id, index as u32))
        .collect())
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
    /// Named location picked for the install root (#99). Takes precedence over
    /// `install_dir`.
    pub install_location_id: Option<String>,
    /// Named location picked for the temporary archive (#99).
    pub download_location_id: Option<String>,
    /// Expected extracted size, used for the free-space guard (#99).
    pub expected_bytes: Option<u64>,
    pub stream: Option<bool>,
    /// Previous install folder to delete once an update is verified. The
    /// previous files stay untouched until the new install is in place.
    pub replace_path: Option<String>,
}

impl InstallParams {
    /// Row written with the job so `resume_install` can rebuild it after a
    /// restart (#203/#208). The token is never persisted.
    fn to_record(&self, job_id: &str, created_at: &str) -> db::InstallJobParamsRecord {
        db::InstallJobParamsRecord {
            job_id: job_id.to_string(),
            game_id: self.game_id,
            version_id: self.version_id,
            title: self.title.clone(),
            version: self.version.clone(),
            executable: self.executable.clone(),
            install_dir: self.install_dir.clone(),
            download_dir: self.download_dir.clone(),
            install_location_id: self.install_location_id.clone(),
            download_location_id: self.download_location_id.clone(),
            expected_bytes: self
                .expected_bytes
                .and_then(|value| i64::try_from(value).ok()),
            stream: self.stream.unwrap_or(false),
            archive_format: None,
            replace_path: self.replace_path.clone(),
            server_url: self.server_url.clone(),
            created_at: created_at.to_string(),
        }
    }

    /// Rebuilds the request of a persisted job. `token` comes from the current
    /// settings/secret store, so no credential is stored in the install table.
    fn from_record(record: db::InstallJobParamsRecord, token: Option<String>) -> Self {
        Self {
            game_id: record.game_id,
            version_id: record.version_id,
            server_url: record.server_url,
            token,
            title: record.title,
            version: record.version,
            executable: record.executable,
            install_dir: record.install_dir,
            download_dir: record.download_dir,
            install_location_id: record.install_location_id,
            download_location_id: record.download_location_id,
            expected_bytes: record
                .expected_bytes
                .and_then(|value| u64::try_from(value).ok()),
            stream: Some(record.stream),
            replace_path: record.replace_path,
        }
    }
}

/// How `resume_install` should treat a job's transport (#205/#208).
///
/// `Keep` continues the job with the mode it was created with. `Download` and
/// `Stream` are the explicit "retry as download" / "restart stream" actions;
/// they reuse the same job id and history row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeMode {
    Keep,
    Download,
    Stream,
}

impl ResumeMode {
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.map(str::trim) {
            None | Some("") | Some("keep") => Ok(ResumeMode::Keep),
            Some("download") => Ok(ResumeMode::Download),
            Some("stream") => Ok(ResumeMode::Stream),
            Some(other) => Err(format!(
                "Unknown resume mode \"{other}\". Use \"download\" or \"stream\"."
            )),
        }
    }

    fn stream_flag(self, current: bool) -> bool {
        match self {
            ResumeMode::Keep => current,
            ResumeMode::Download => false,
            ResumeMode::Stream => true,
        }
    }
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
    /// The per-entry stream path is unusable (a proxy rejects every attempt
    /// before a single byte arrives). The caller falls back to the classic
    /// resumable download instead of failing the install (#204).
    StreamUnavailable(String),
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure::Failed(message)
    }
}

#[derive(Debug, Default)]
pub(crate) struct JobPaths {
    pub(crate) staging: Option<PathBuf>,
    pub(crate) part: Option<PathBuf>,
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

    let stream_requested_now = stream_requested(params.stream, &settings, &params.server_url);
    let plan = crate::locations::plan_install(
        &settings,
        params.install_location_id.as_deref(),
        params.download_location_id.as_deref(),
        params.install_dir.as_deref(),
        params.download_dir.as_deref(),
        stream_requested_now,
    )?;
    // Guard before queueing: a full disk fails here with an actionable message
    // instead of after a long download. Same volume + archive: peak usage is
    // the archive plus the extracted copy.
    if let Some(expected) = params.expected_bytes.filter(|value| *value > 0) {
        let install_required = if stream_requested_now || plan.cross_filesystem {
            expected
        } else {
            expected.saturating_mul(2)
        };
        if let Some(free) = free_space(&plan.install_root) {
            crate::locations::guard_free_space(install_required, free)
                .map_err(|error| format!("{} {}", plan.peak_note, error))?;
        }
        if plan.cross_filesystem && !stream_requested_now {
            if let Some(free) = free_space(&plan.download_dir) {
                crate::locations::guard_free_space(expected, free).map_err(|error| {
                    format!(
                        "The download location {} does not have room for the archive. {error}",
                        plan.download_dir.display()
                    )
                })?;
            }
        }
    }
    // Validate the folders now: a relative path or a read-only drive fails the
    // click with a clear message instead of the worker after a long queue wait.
    let install_root = absolute_path(plan.install_root.clone())?;
    let download_dir = absolute_path(plan.download_dir.clone())?;
    let install_status = crate::locations::probe_path(&install_root.to_string_lossy())?;
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
        let download_status = crate::locations::probe_path(&download_dir.to_string_lossy())?;
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
            return resume_entry(app, existing, ResumeMode::Keep);
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

/// Start the next queued install while a slot is free (#202).
///
/// The scheduler picks the lowest `queue_position` among `queued` jobs, so a
/// game moved to the top starts next. A running job is never preempted: it is
/// only stopped by an explicit pause or cancel. The worker that frees a slot
/// calls this again.
pub(crate) fn dispatch(app: &AppHandle) {
    let state = app.state::<AppState>();
    loop {
        let permit = match Arc::clone(&state.install_slots).try_acquire_owned() {
            Ok(permit) => permit,
            // Every slot is busy; the worker that finishes calls dispatch.
            Err(_) => return,
        };
        let Some(job_id) = next_queued(&queue_snapshot(&state)) else {
            return;
        };
        // Claim the job under the registry lock, so a cancel or a second
        // dispatch can never start the same entry twice.
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
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            run(handle, job_id, permit).await;
        });
    }
}

/// Snapshot of the active queue: running, queued and paused jobs.
fn queue_snapshot(state: &AppState) -> Vec<QueueJob> {
    let Ok(jobs) = state.jobs.lock() else {
        return Vec::new();
    };
    jobs.values()
        .map(|entry| {
            let job = entry.snapshot();
            QueueJob {
                job_id: job.job_id,
                phase: job.phase,
                position: job.queue_position,
                created_at: entry.created_at.clone(),
                running: entry.running.load(Ordering::SeqCst),
            }
        })
        .filter(|job| !job.phase.is_terminal())
        .collect()
}

/// Applies compacted positions to the registry and the database (#202).
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
        let orphan = matches!(phase, InstallPhase::Queued | InstallPhase::Paused)
            && !entry.running.swap(true, Ordering::SeqCst);
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
        InstallPhase::Queued => {
            entry.request_pause();
            entry.update(|job| {
                job.phase = InstallPhase::Paused;
                job.speed_bps = 0;
                job.clear_reconnect();
                job.message = Some("Paused".to_string());
            });
            persist(app, &entry);
            emit_jobs(app);
            Ok(entry.snapshot())
        }
        InstallPhase::Downloading => {
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
            return resume_entry(app, rebuilt, mode);
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
        return resume_entry(app, entry, mode);
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
    resume_entry(app, entry, mode)
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
    let staging = entry
        .staging_path()
        .map(PathBuf::from)
        .unwrap_or_else(|| install_root.join(format!(".{slug}.staging-{}", short_id(&job_id))));
    JobPaths {
        staging: Some(staging),
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

async fn run(app: AppHandle, job_id: String, permit: tokio::sync::OwnedSemaphorePermit) {
    run_job(app.clone(), job_id, permit).await;
    // The slot is free: start the next install in queue order. This also picks
    // up a resume that landed while the worker was finishing a pause, so a job
    // never stays queued without a worker.
    dispatch(&app);
}

async fn run_job(app: AppHandle, job_id: String, permit: tokio::sync::OwnedSemaphorePermit) {
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

    // The scheduler already gave this job the install slot (#202). A cancel or
    // pause while queued moves it out of the queue without touching the disk.
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
            }
        }
    }

    match result {
        Ok(install_path) => {
            // The staging folder was moved into place: the per-entry
            // checkpoint is no longer needed.
            clear_stream_checkpoint(&app, &entry);
            entry.update(|job| {
                job.phase = InstallPhase::Done;
                job.speed_bps = 0;
                job.clear_reconnect();
                job.bytes_done = job.bytes_total.unwrap_or(job.bytes_done);
                job.message = Some("Installed".to_string());
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
        Err(Failure::StreamUnavailable(message)) => {
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
    // A resumable stream install keeps its staging folder and checkpoint: the
    // next `resume_install` continues from the completed entries (#115).
    if !entry.snapshot().resumable {
        if let Some(staging) = job_paths.staging.as_deref() {
            let _ = tokio::fs::remove_dir_all(staging).await;
        }
    }
    entry.update(|job| {
        job.phase = InstallPhase::Paused;
        job.speed_bps = 0;
        job.clear_reconnect();
        job.message = Some("Paused".to_string());
    });
    persist(app, entry);
    let _ = app.emit("install://progress", entry.snapshot());
    emit_jobs(app);
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
    }
    if let Some(part) = job_paths.part.as_deref() {
        let _ = tokio::fs::remove_file(part).await;
    }
    entry.set_part_path(None);
    let message = if keep_staging {
        "Install cancelled; the files already downloaded are kept for the next attempt"
    } else {
        "Install cancelled"
    };
    entry.update(|job| {
        job.phase = InstallPhase::Cancelled;
        job.speed_bps = 0;
        job.clear_reconnect();
        job.message = Some(message.to_string());
        // #202: a cancelled job leaves the queue without a gap.
        job.queue_position = None;
    });
    persist(app, entry);
    compact_queue(app);
    let _ = app.emit("install://progress", entry.snapshot());
    emit_jobs(app);
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
    entry.update(|job| {
        job.phase = InstallPhase::Error;
        job.speed_bps = 0;
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
    tracing::warn!(job_id = %job.job_id, error = %message, "install failed");
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

/// Re-enter downloads interrupted by an app restart (#212).
///
/// `mark_interrupted_jobs` stamped every row that was still active at exit;
/// this runs once at startup and rebuilds those workers from their persisted
/// parameters. A user-paused or cancelled job is never included, and a real
/// failure (404, disk full) carries a different message and is skipped too.
/// `resume_entry` reuses the recorded queue position, so the order survives.
pub(crate) fn auto_resume_on_startup(app: &AppHandle) {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    if !settings.auto_resume_downloads {
        tracing::info!("auto-resume of interrupted downloads is disabled");
        return;
    }

    let records = {
        let Ok(connection) = state.db.lock() else {
            tracing::warn!("could not read interrupted downloads: database unavailable");
            return;
        };
        match db::interrupted_install_jobs(
            &connection,
            INTERRUPTED_MESSAGE,
            LEGACY_INTERRUPTED_MESSAGE,
        ) {
            Ok(records) => records,
            Err(error) => {
                tracing::warn!(error = %error, "could not list interrupted downloads");
                return;
            }
        }
    };
    if records.is_empty() {
        return;
    }

    let token = settings.token.clone();
    let mut resumed = 0usize;
    for record in records {
        let job_id = record.job_id.clone();
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
        match entry_from_persisted(record, params, token.clone(), ResumeMode::Keep) {
            Ok(entry) => match resume_entry(app, Arc::new(entry), ResumeMode::Keep) {
                Ok(_) => resumed += 1,
                Err(error) => {
                    tracing::warn!(job_id = %job_id, error = %error, "could not auto-resume a download")
                }
            },
            Err(error) => {
                tracing::warn!(job_id = %job_id, error = %error, "could not rebuild an interrupted download")
            }
        }
    }
    tracing::info!(resumed, "auto-resumed interrupted downloads");
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
    entry.set_staging_path(Some(staging.to_string_lossy().to_string()));
    // The staging folder is NOT cleared here: a resumable stream install
    // adopts the folder of its previous attempt and needs the completed files
    // (#115). Every non-resumable branch resets it before writing.

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

    // 2. Stream install (remote ZIP only): write the entries straight into
    // staging. The per-entry protocol (#115) is resumable. Non-ZIP archives
    // (7z/RAR) and older servers cannot stream per entry: rather than run a
    // sequential tar that cannot be paused, they fall back to the classic
    // resumable download *before* any data is written (#203).
    let mut streamed = false;
    if archive.is_none() && stream_requested(params.stream, &settings, &params.server_url) {
        // #212: a stream job retries itself after its per-entry budget is
        // exhausted, with the same checkpoint, instead of failing outright.
        let tuning = DownloadTuning {
            network: Some(&state.network),
            retry_now: Some(entry.retry_signal()),
            ..DownloadTuning::from_settings(
                settings.download_stall_timeout_secs,
                settings.download_retry_window_secs,
            )
        };
        let outcome = {
            let mut attempt = InstallStreamAttempt {
                app,
                entry,
                params: &params,
                staging: &staging,
                job_paths,
            };
            stream_with_retry(
                &mut attempt,
                tuning,
                &entry.cancel,
                &pause,
                |message, attempt, delay| {
                    let at = retry_at(delay);
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
            Ok(StreamOutcome::Done) => {
                streamed = true;
                job_paths.part = None;
                entry.set_part_path(None);
            }
            Ok(StreamOutcome::Unsupported(status)) => {
                tracing::info!(
                    status = status.as_u16(),
                    game_id = params.game_id,
                    version_id = params.version_id,
                    "per-entry stream unsupported; using the resumable download instead"
                );
                let stream_staging = job_paths.staging.clone().unwrap_or_else(|| staging.clone());
                reset_staging(&stream_staging).await?;
                // The job is now a normal, resumable download: it pauses and
                // resumes with HTTP Range and a `.part` file (#203/#208).
                entry.update(|job| {
                    job.stream = false;
                    job.resumable = false;
                    job.entries_done = None;
                    job.entries_total = None;
                    job.current_entry = None;
                    job.message = Some(
                        "Streaming is not available for this archive; downloading instead"
                            .to_string(),
                    );
                });
                entry.set_stream_completed(None);
                entry.set_staging_path(None);
                persist(app, entry);
            }
            Err(Failure::StreamUnavailable(message)) => {
                // Every entry attempt failed before receiving a byte: the
                // per-entry path is unusable behind this proxy. Fall back once
                // to the classic resumable download (#204).
                tracing::warn!(
                    error = %message,
                    game_id = params.game_id,
                    version_id = params.version_id,
                    "per-entry stream unavailable; falling back to the resumable download"
                );
                let stream_staging = job_paths.staging.clone().unwrap_or_else(|| staging.clone());
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
                entry.set_stream_completed(None);
                entry.set_staging_path(None);
                persist(app, entry);
            }
            Err(failure) => return Err(failure),
        }
    }

    // 3. Otherwise download with resume.
    if archive.is_none() && !streamed {
        // A stream attempt may have adopted the staging folder of an earlier
        // job: the classic path starts from a clean folder, so drop it.
        if let Some(previous) = job_paths
            .staging
            .clone()
            .filter(|previous| previous != &staging)
        {
            let _ = tokio::fs::remove_dir_all(&previous).await;
        }
        reset_staging(&staging).await?;
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
                        job.clear_reconnect();
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
                            job.clear_reconnect();
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
                job.clear_reconnect();
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
        job.clear_reconnect();
    });
    persist(app, entry);
    let _ = app.emit("install://progress", entry.snapshot());
}

/// Deletes a staging folder so a non-resumable branch starts from scratch.
async fn reset_staging(staging: &Path) -> Result<(), Failure> {
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

pub(crate) fn absolute_path(path: PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .map_err(|e| format!("Could not resolve a relative install path: {e}"))
    }
}

/// Free bytes on the volume that would hold `path`, or `None` when it cannot be
/// probed (a missing drive): the guard then stays out of the way and the worker
/// reports the real error.
fn free_space(path: &Path) -> Option<u64> {
    let existing = paths::nearest_existing(path)?;
    fs2::available_space(existing).ok()
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
#[derive(Debug, Clone)]
pub(crate) struct DownloadProgress {
    pub done: u64,
    pub total: Option<u64>,
    pub speed_bps: u64,
    pub message: String,
    /// True while the client waits to reconnect after a stall or a dropped
    /// response (#195). The UI shows the resume point instead of a frozen rate.
    pub reconnecting: bool,
    /// Connection attempt of this download, starting at 1.
    pub attempt: u32,
    /// Last transport error, kept until the transfer completes.
    pub last_error: Option<String>,
    /// How long the connection was silent when the stall was detected.
    pub stalled_for_ms: Option<u64>,
    /// RFC 3339 time of the next automatic retry (#212).
    pub next_retry_at: Option<String>,
    /// HTTP status of the last response, `None` before the first one (#212).
    pub http_status: Option<u16>,
    /// True while the retry waits for the OS to report a network (#212).
    pub waiting_for_network: bool,
}

impl DownloadProgress {
    fn plain(attempt: u32, done: u64, total: Option<u64>, speed_bps: u64, message: String) -> Self {
        Self {
            done,
            total,
            speed_bps,
            message,
            reconnecting: false,
            attempt,
            last_error: None,
            stalled_for_ms: None,
            next_retry_at: None,
            http_status: None,
            waiting_for_network: false,
        }
    }
}

/// Read/write tuning for one resumable download (#195, #212).
#[derive(Debug, Clone, Copy)]
pub(crate) struct DownloadTuning<'a> {
    /// Abort the response when no bytes arrive for this long. A transfer that
    /// keeps producing bytes is never judged a stall.
    pub idle_timeout: Duration,
    /// Connections per download, including the first one. `u32::MAX` for an
    /// unlimited retry window.
    pub max_attempts: u32,
    /// Give up on retries once this much wall-clock time was spent reconnecting.
    /// `None` keeps retrying while the job is active.
    pub max_retry_window: Option<Duration>,
    pub base_backoff: Duration,
    pub max_backoff: Duration,
    /// OS reachability (#212): retry waits park while offline.
    pub network: Option<&'a crate::state::NetworkGate>,
    /// "Retry now" signal of the job (#212); skips one backoff sleep.
    pub retry_now: Option<&'a tokio::sync::Notify>,
}

impl Default for DownloadTuning<'_> {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_secs(30),
            max_attempts: 50,
            max_retry_window: Some(Duration::from_secs(6 * 60 * 60)),
            base_backoff: Duration::from_secs(1),
            // A long-horizon retry must not hammer a dead link: five minutes
            // between attempts still recovers quickly after an outage (#212).
            max_backoff: Duration::from_secs(5 * 60),
            network: None,
            retry_now: None,
        }
    }
}

impl<'a> DownloadTuning<'a> {
    /// Build from the persisted settings, clamped so a bad value cannot turn
    /// every install into a reconnect storm or wait forever.
    pub(crate) fn from_settings(idle_secs: u64, retry_window_secs: u64) -> Self {
        let window = (retry_window_secs != 0)
            .then(|| Duration::from_secs(retry_window_secs.clamp(60, 7 * 24 * 60 * 60)));
        Self {
            idle_timeout: Duration::from_secs(idle_secs.clamp(5, 600)),
            // Unlimited retry: no attempt cap either, otherwise the count
            // would become the real limit and stop a working retry early.
            max_attempts: if window.is_some() { 50 } else { u32::MAX },
            max_retry_window: window,
            ..Self::default()
        }
    }

    /// Kept for call sites that only know the stall timeout (#195).
    #[cfg(test)]
    pub(crate) fn from_idle_secs(seconds: u64) -> Self {
        Self::from_settings(seconds, 6 * 60 * 60)
    }
}

/// Why a chunk read ended early.
#[derive(Debug)]
pub(crate) enum ChunkError {
    /// The connection went silent beyond the idle timeout.
    Stalled,
    /// The transport reported an error (reset, truncated body, TLS, …).
    Lost(String),
}

/// The part of an HTTP response the resume loop needs.
#[allow(async_fn_in_trait)]
pub(crate) trait DownloadResponse {
    fn status(&self) -> u16;
    fn content_length(&self) -> Option<u64>;
    fn content_range_total(&self) -> Option<u64>;
    /// First byte of a 206 response; `None` when the header is absent.
    fn content_range_start(&self) -> Option<u64> {
        None
    }
    fn etag(&self) -> Option<String>;
    /// `Retry-After` of a 429/503, in seconds (#212). `None` when absent or
    /// unparsable.
    fn retry_after(&self) -> Option<Duration> {
        None
    }
    /// Next body chunk, or `None` at end of stream.
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, ChunkError>;
    /// Body of a failed response, for the user-facing error.
    async fn error_body(self) -> String
    where
        Self: Sized,
    {
        String::new()
    }
}

/// Opens one response for a byte offset (`None` = from zero).
#[allow(async_fn_in_trait)]
pub(crate) trait DownloadTransport {
    type Response: DownloadResponse;
    async fn open(&mut self, range_start: Option<u64>) -> Result<Self::Response, String>;
}

struct HttpTransport<'a> {
    client: &'a reqwest::Client,
    url: &'a str,
    token: Option<&'a str>,
}

struct HttpDownloadResponse {
    response: reqwest::Response,
}

impl DownloadResponse for HttpDownloadResponse {
    fn status(&self) -> u16 {
        self.response.status().as_u16()
    }

    fn content_length(&self) -> Option<u64> {
        self.response.content_length()
    }

    fn content_range_total(&self) -> Option<u64> {
        self.response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_content_range_total)
    }

    fn content_range_start(&self) -> Option<u64> {
        self.response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_content_range_start)
    }

    fn etag(&self) -> Option<String> {
        self.response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    fn retry_after(&self) -> Option<Duration> {
        self.response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_retry_after)
    }

    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, ChunkError> {
        match self.response.chunk().await {
            Ok(Some(bytes)) => Ok(Some(bytes.to_vec())),
            Ok(None) => Ok(None),
            Err(error) => Err(ChunkError::Lost(error.to_string())),
        }
    }

    async fn error_body(self) -> String {
        match tokio::time::timeout(Duration::from_secs(5), self.response.text()).await {
            Ok(Ok(body)) => body,
            _ => String::new(),
        }
    }
}

impl DownloadTransport for HttpTransport<'_> {
    type Response = HttpDownloadResponse;

    async fn open(&mut self, range_start: Option<u64>) -> Result<Self::Response, String> {
        let mut request = self
            .client
            .get(self.url)
            .header(reqwest::header::ACCEPT_ENCODING, "identity");
        if let Some(token) = self.token {
            request = request.bearer_auth(token);
        }
        if let Some(start) = range_start {
            request = request.header(reqwest::header::RANGE, format!("bytes={start}-"));
        }
        let response = request.send().await.map_err(|error| error.to_string())?;
        Ok(HttpDownloadResponse { response })
    }
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
/// A silent response is abandoned after `tuning.idle_timeout` and the next
/// request resumes at the last flushed byte; reconnects are bounded by
/// `tuning` (#195). The `.part` file is kept when the user pauses; cancelling
/// is handled by the caller (which deletes it). `progress` is called at most
/// every 250 ms.
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
    tuning: DownloadTuning<'_>,
    progress: impl FnMut(DownloadProgress) + Send,
) -> Result<(), Failure> {
    let mut transport = HttpTransport { client, url, token };
    download_into(
        &mut transport,
        part,
        title,
        cancel,
        pause,
        bandwidth_kib,
        tuning,
        progress,
    )
    .await
}

/// Connection budget of one download: attempt count, retry window and the
/// circuit breaker on repeated identical permanent errors (#195, #212).
struct RetryState {
    attempt: u32,
    retry_started: Option<Instant>,
    last_failure: Option<String>,
    identical_failures: u32,
}

impl RetryState {
    fn new() -> Self {
        Self {
            attempt: 0,
            retry_started: None,
            last_failure: None,
            identical_failures: 0,
        }
    }

    fn next_attempt(&mut self) {
        self.attempt = self.attempt.saturating_add(1);
        if self.retry_started.is_none() && self.attempt > 1 {
            self.retry_started = Some(Instant::now());
        }
    }

    /// Record one failure. Returns a stop reason when the same *permanent*
    /// error repeated, so a flapping archive cannot spin the loop (#212).
    fn record_failure(&mut self, message: &str, permanent: bool) -> Option<String> {
        if self.last_failure.as_deref() == Some(message) {
            self.identical_failures = self.identical_failures.saturating_add(1);
        } else {
            self.last_failure = Some(message.to_string());
            self.identical_failures = 1;
        }
        if permanent && self.identical_failures >= 3 {
            return Some(format!(
                "the same permanent error repeated {} times",
                self.identical_failures
            ));
        }
        None
    }

    /// Whether another connection is allowed; adds a friendly reason when not.
    fn exhausted(&self, tuning: DownloadTuning<'_>) -> Option<String> {
        if self.attempt >= tuning.max_attempts {
            return Some(format!(
                "the connection failed {} times in a row",
                self.attempt
            ));
        }
        if let (Some(started), Some(window)) = (self.retry_started, tuning.max_retry_window) {
            if started.elapsed() >= window {
                return Some(format!(
                    "the connection stayed unreliable for {}",
                    human_duration(window)
                ));
            }
        }
        None
    }
}

fn human_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!("{} h", seconds / 3600)
    } else if seconds >= 60 {
        format!("{} min", seconds / 60)
    } else {
        format!("{seconds} s")
    }
}

/// Exponential backoff with ±25 % jitter, capped by the tuning.
pub(crate) fn retry_delay(attempt: u32, tuning: DownloadTuning<'_>) -> Duration {
    let exponent = attempt.saturating_sub(1).min(10);
    let base = tuning
        .base_backoff
        .saturating_mul(1u32 << exponent)
        .min(tuning.max_backoff);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    let factor = 0.75 + f64::from(nanos % 501) / 1000.0;
    base.mul_f64(factor).min(tuning.max_backoff)
}

fn resume_message(done: u64, total: Option<u64>) -> String {
    match total.filter(|total| *total > 0) {
        Some(total) => format!(
            "Connection lost, resuming from {}%",
            done.saturating_mul(100) / total
        ),
        None => format!("Connection lost, resuming from {}", human_bytes(done)),
    }
}

/// RFC 3339 timestamp of a retry that will start in `delay` (#212).
fn retry_at(delay: Duration) -> String {
    (chrono::Utc::now() + chrono::Duration::from_std(delay).unwrap_or_default()).to_rfc3339()
}

/// `Retry-After: 30` → 30 s. An HTTP-date is ignored rather than misparsed;
/// the reconnect limiter of the server uses delta-seconds (#212).
fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// The bigger of the exponential backoff and the server's `Retry-After`,
/// capped so a bogus header cannot park a job for hours (#212).
fn retry_delay_with_hint(base: Duration, hint: Option<Duration>) -> Duration {
    const MAX_HINT: Duration = Duration::from_secs(30 * 60);
    base.max(hint.unwrap_or_default().min(MAX_HINT))
}

/// One reconnecting progress snapshot, shared by every retry branch.
#[allow(clippy::too_many_arguments)]
fn reconnect_progress(
    attempt: u32,
    done: u64,
    total: Option<u64>,
    last_error: Option<String>,
    stalled_for_ms: Option<u64>,
    next_retry_at: Option<String>,
    http_status: Option<u16>,
    waiting_for_network: bool,
) -> DownloadProgress {
    DownloadProgress {
        done,
        total,
        speed_bps: 0,
        message: restart_message(next_retry_at.as_deref(), done, total),
        reconnecting: true,
        attempt,
        last_error,
        stalled_for_ms,
        next_retry_at,
        http_status,
        waiting_for_network,
    }
}

/// "Retrying automatically" while a timer runs, the resume point while waiting
/// on the OS network (#212). The UI adds its own countdown from `nextRetryAt`.
fn restart_message(next_retry_at: Option<&str>, done: u64, total: Option<u64>) -> String {
    match next_retry_at {
        Some(_) => format!("{}, retrying automatically", resume_message(done, total)),
        None => "Waiting for the network".to_string(),
    }
}

/// Sleep a backoff, honouring cancel, pause and "Retry now" (#212).
async fn sleep_or_stop(
    delay: Duration,
    cancel: &CancellationToken,
    pause: &CancellationToken,
    retry_now: Option<&tokio::sync::Notify>,
) -> Result<bool, Failure> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Ok(false),
        _ = pause.cancelled() => Err(Failure::Paused),
        // A "Retry now" click skips the remaining wait, not the attempt budget.
        _ = async {
            match retry_now {
                Some(signal) => signal.notified().await,
                None => std::future::pending().await,
            }
        } => Ok(true),
        _ = tokio::time::sleep(delay) => Ok(true),
    }
}

/// The resumable transfer core, generic over the transport so tests can inject
/// stalls, resets and a changed archive deterministically (#195).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn download_into<T: DownloadTransport>(
    transport: &mut T,
    part: &Path,
    title: &str,
    cancel: &CancellationToken,
    pause: &CancellationToken,
    bandwidth_kib: u64,
    tuning: DownloadTuning<'_>,
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

    let mut retry = RetryState::new();
    // Guards the "restart from zero" paths so a flapping server cannot loop.
    let mut restarts = 0u32;
    let mut expected_total: Option<u64> = None;
    let mut expected_etag: Option<String> = None;
    let mut preflight_done = false;
    let mut last_error: Option<String> = None;

    'connect: loop {
        // #212: park the retry while the OS reports no network. This wait does
        // not consume an attempt: nothing was tried, so nothing failed.
        if let Some(network) = tuning.network {
            if !network.is_online() {
                progress(reconnect_progress(
                    retry.attempt.saturating_add(1),
                    existing,
                    expected_total,
                    last_error.clone(),
                    None,
                    None,
                    None,
                    true,
                ));
                wait_online(network, cancel, pause).await?;
            }
        }
        retry.next_attempt();

        let opened = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(Failure::Cancelled),
            _ = pause.cancelled() => return Err(Failure::Paused),
            opened = transport.open((existing > 0).then_some(existing)) => opened,
        };

        let mut response = match opened {
            Ok(response) => response,
            Err(error) => {
                last_error = Some(error.clone());
                if let Some(reason) = retry.exhausted(tuning) {
                    return Err(Failure::Failed(format!(
                        "The download kept failing ({reason}). Last error: {error}. The partial archive was kept; install again to resume."
                    )));
                }
                let delay = retry_delay(retry.attempt, tuning);
                progress(reconnect_progress(
                    retry.attempt.saturating_add(1),
                    existing,
                    expected_total,
                    Some(error),
                    None,
                    Some(retry_at(delay)),
                    None,
                    false,
                ));
                if !sleep_or_stop(delay, cancel, pause, tuning.retry_now).await? {
                    return Err(Failure::Cancelled);
                }
                continue 'connect;
            }
        };

        let status = response.status();
        let retry_after = response.retry_after();

        if status == 416 {
            let total = response.content_range_total();
            // The `.part` is only complete if it matches the archive the server
            // still has; a replaced archive must restart, not pass as done.
            let unchanged = expected_total.is_none() || expected_total == total;
            if existing > 0 && total == Some(existing) && unchanged {
                // The `.part` file is already complete: a previous run
                // downloaded everything but failed before extraction.
                let mut update = DownloadProgress::plain(
                    retry.attempt,
                    existing,
                    Some(existing),
                    0,
                    "Download already complete".to_string(),
                );
                update.http_status = Some(status);
                progress(update);
                return Ok(());
            }
            if existing > 0 && restarts < 2 {
                restarts += 1;
                tracing::warn!(
                    part = %part.display(),
                    bytes = existing,
                    "server rejected the resume offset; restarting the archive explicitly"
                );
                let _ = tokio::fs::remove_file(part).await;
                existing = 0;
                expected_total = None;
                expected_etag = None;
                progress(DownloadProgress {
                    done: 0,
                    total: None,
                    speed_bps: 0,
                    message: "The server could not resume this file; restarting from 0%"
                        .to_string(),
                    reconnecting: true,
                    attempt: retry.attempt.saturating_add(1),
                    last_error: last_error.clone(),
                    stalled_for_ms: None,
                    next_retry_at: None,
                    http_status: Some(status),
                    waiting_for_network: false,
                });
                continue 'connect;
            }
            // #212: the resume offset was rejected twice; retrying the same
            // Range cannot help, so this is permanent.
            let message = "The server rejected the resume request (HTTP 416). Delete the partial download and retry.".to_string();
            let _ = retry.record_failure(&message, true);
            return Err(Failure::Failed(message));
        }

        if !(200..300).contains(&status) {
            let retryable = matches!(status, 408 | 425 | 429 | 500..=599);
            let body = response.error_body().await;
            let snippet = body.trim().chars().take(300).collect::<String>();
            let hint = match status {
                401 | 403 => " Check that you are signed in to the server.",
                404 => " The game version was not found on the server.",
                _ => "",
            };
            let detail = if snippet.is_empty() {
                String::new()
            } else {
                format!(": {snippet}")
            };
            let message = format!("Download failed with HTTP {status}{hint}{detail}");
            if !retryable {
                // #212: the permanent failures stop at once and keep the
                // `.part`; the circuit breaker only guards against a repeat.
                let _ = retry.record_failure(&message, true);
                return Err(Failure::Failed(message));
            }
            last_error = Some(message.clone());
            if let Some(reason) = retry
                .record_failure(&message, false)
                .or_else(|| retry.exhausted(tuning))
            {
                return Err(Failure::Failed(format!(
                    "The download kept failing ({reason}). Last error: {}. The partial archive was kept; install again to resume.",
                    last_error.clone().unwrap_or_default()
                )));
            }
            // #212: honour `Retry-After` (server reconnect rate limit); the
            // exponential delay is a floor, not a replacement.
            let delay = retry_delay_with_hint(retry_delay(retry.attempt, tuning), retry_after);
            progress(reconnect_progress(
                retry.attempt.saturating_add(1),
                existing,
                expected_total,
                last_error.clone(),
                None,
                Some(retry_at(delay)),
                Some(status),
                false,
            ));
            if !sleep_or_stop(delay, cancel, pause, tuning.retry_now).await? {
                return Err(Failure::Cancelled);
            }
            continue 'connect;
        }

        let append = status == 206 && existing > 0;
        if !append && existing > 0 {
            // The server ignored `Range` and started from zero. Truncating is
            // the only honest option; say so instead of appending a full body
            // to an existing partial file.
            tracing::warn!(
                part = %part.display(),
                bytes = existing,
                "server ignored the Range request; restarting the archive"
            );
            existing = 0;
        }

        let content_length = response.content_length();
        let content_range_total = response.content_range_total();
        let total = if status == 206 {
            content_range_total.or_else(|| content_length.map(|length| length + existing))
        } else {
            content_length
        };

        // A resumed response must describe the same archive. A size or ETag
        // change means the server replaced the file: restart explicitly (with
        // a visible message) instead of splicing two different archives.
        if append {
            let etag_changed = match (&expected_etag, response.etag()) {
                (Some(known), Some(current)) => *known != current,
                _ => false,
            };
            let total_changed =
                matches!((expected_total, total), (Some(known), Some(current)) if known != current);
            let start_changed =
                matches!(response.content_range_start(), Some(start) if start != existing);
            if start_changed || etag_changed || total_changed {
                if restarts < 2 {
                    restarts += 1;
                    tracing::warn!(
                        part = %part.display(),
                        bytes = existing,
                        ?expected_total,
                        ?total,
                        start = response.content_range_start(),
                        "the archive changed on the server; restarting from zero"
                    );
                    let _ = tokio::fs::remove_file(part).await;
                    existing = 0;
                    expected_total = None;
                    expected_etag = None;
                    progress(DownloadProgress {
                        done: 0,
                        total: None,
                        speed_bps: 0,
                        message:
                            "The archive changed on the server; restarting the download from 0%"
                                .to_string(),
                        reconnecting: true,
                        attempt: retry.attempt.saturating_add(1),
                        last_error: last_error.clone(),
                        stalled_for_ms: None,
                        next_retry_at: None,
                        http_status: Some(status),
                        waiting_for_network: false,
                    });
                    continue 'connect;
                }
                // #212: two explicit restarts did not converge; the archive
                // keeps changing, which no retry can fix.
                let message = "The archive changed on the server while the download was resuming. Start the install again to fetch the new version.".to_string();
                let _ = retry.record_failure(&message, true);
                return Err(Failure::Failed(message));
            }
            if expected_total.is_none() {
                expected_total = total;
            }
            if expected_etag.is_none() {
                expected_etag = response.etag();
            }
        } else {
            expected_total = total;
            expected_etag = response.etag();
        }

        let mut done = existing;

        if !preflight_done {
            preflight_done = true;
            // Disk preflight: never start a download that cannot fit.
            // Extraction size is unknown, so only the archive is required.
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
        }

        let mut started_update =
            DownloadProgress::plain(retry.attempt, done, total, 0, download_message(title));
        started_update.http_status = Some(status);
        progress(started_update);

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
        let mut last_chunk = Instant::now();
        let mut interrupted: Option<(ChunkError, u64)> = None;

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
                chunk = tokio::time::timeout(tuning.idle_timeout, response.next_chunk()) => {
                    match chunk {
                        Err(_elapsed) => {
                            let silent = last_chunk.elapsed();
                            flush_before_resume(&mut writer, part).await?;
                            interrupted = Some((ChunkError::Stalled, silent.as_millis() as u64));
                            break;
                        }
                        Ok(Ok(Some(bytes))) => {
                            writer.write_all(&bytes).await.map_err(|e| {
                                Failure::Failed(format!(
                                    "Could not write the download to {}: {e}",
                                    part.display()
                                ))
                            })?;
                            done = done.saturating_add(bytes.len() as u64);
                            last_chunk = Instant::now();

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
                                progress(DownloadProgress::plain(
                                    retry.attempt,
                                    done,
                                    total,
                                    speed,
                                    download_message(title),
                                ));
                                last_emit = Instant::now();
                                last_instant = Instant::now();
                                last_bytes = done;
                            }
                        }
                        Ok(Ok(None)) => break,
                        Ok(Err(error)) => {
                            let silent = last_chunk.elapsed();
                            flush_before_resume(&mut writer, part).await?;
                            interrupted = Some((error, silent.as_millis() as u64));
                            break;
                        }
                    }
                }
            }
        }

        if let Some((error, silent_ms)) = interrupted {
            // The connection is gone but every byte written so far is on disk:
            // the next attempt asks for exactly this offset.
            existing = done;
            let reason = match &error {
                ChunkError::Stalled => format!(
                    "no data for {}",
                    human_duration(Duration::from_millis(silent_ms))
                ),
                ChunkError::Lost(message) => message.clone(),
            };
            last_error = Some(reason.clone());
            if let Some(stop) = retry
                .record_failure(&reason, false)
                .or_else(|| retry.exhausted(tuning))
            {
                return Err(Failure::Failed(format!(
                    "The download stopped at {} ({stop}). Last error: {}. The partial archive was kept; install again to resume.",
                    human_bytes(existing),
                    last_error.clone().unwrap_or_default()
                )));
            }
            let delay = retry_delay(retry.attempt, tuning);
            tracing::warn!(
                part = %part.display(),
                bytes = existing,
                attempt = retry.attempt,
                error = %last_error.clone().unwrap_or_default(),
                "download interrupted; reconnecting from the last flushed byte"
            );
            progress(reconnect_progress(
                retry.attempt.saturating_add(1),
                existing,
                total,
                last_error.clone(),
                Some(silent_ms),
                Some(retry_at(delay)),
                Some(status),
                false,
            ));
            if !sleep_or_stop(delay, cancel, pause, tuning.retry_now).await? {
                return Err(Failure::Cancelled);
            }
            continue 'connect;
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

        let mut completed_update = DownloadProgress::plain(
            retry.attempt,
            done,
            Some(total.unwrap_or(done)),
            0,
            "Download complete".to_string(),
        );
        completed_update.http_status = Some(status);
        progress(completed_update);

        return Ok(());
    }
}

/// Flush the buffered bytes before a reconnect. The resume offset is only
/// valid if everything counted in `done` reached the file (#195).
async fn flush_before_resume(
    writer: &mut tokio::io::BufWriter<tokio::fs::File>,
    part: &Path,
) -> Result<(), Failure> {
    writer.flush().await.map_err(|error| {
        Failure::Failed(format!(
            "Could not flush the download to {}: {error}",
            part.display()
        ))
    })
}

/// Sleep the backoff, still honouring cancel and pause. Returns `Ok(false)`
/// when the caller should stop with `Failure::Cancelled` (checked by the call
/// site) and `Err` for pause.
async fn wait_online(
    network: &crate::state::NetworkGate,
    cancel: &CancellationToken,
    pause: &CancellationToken,
) -> Result<(), Failure> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(Failure::Cancelled),
        _ = pause.cancelled() => Err(Failure::Paused),
        _ = network.wait_online() => Ok(()),
    }
}

/// One whole per-entry stream attempt, abstracted so the long-horizon retry
/// policy can be tested without an `AppHandle` (#212).
#[allow(async_fn_in_trait)]
pub(crate) trait StreamAttempt {
    async fn run(&mut self) -> Result<StreamOutcome, Failure>;
}

/// Retry a whole stream attempt with the classic long-horizon policy (#212).
///
/// After the per-entry attempts are exhausted, the stream job itself backs off
/// and runs again against the same checkpoint instead of failing: completed
/// entries and `.part` files are adopted by `stream_install::install`. A
/// structurally unusable stream ([`Failure::StreamUnavailable`]) still returns
/// at once so the caller can fall back to classic; permanent errors stop too.
async fn stream_with_retry<A: StreamAttempt>(
    attempt: &mut A,
    tuning: DownloadTuning<'_>,
    cancel: &CancellationToken,
    pause: &CancellationToken,
    mut on_retry: impl FnMut(&str, u32, Duration),
) -> Result<StreamOutcome, Failure> {
    let mut retry = RetryState::new();
    loop {
        if let Some(network) = tuning.network {
            if !network.is_online() {
                wait_online(network, cancel, pause).await?;
            }
        }
        retry.next_attempt();
        match attempt.run().await {
            Ok(outcome) => return Ok(outcome),
            Err(Failure::Cancelled) => return Err(Failure::Cancelled),
            Err(Failure::Paused) => return Err(Failure::Paused),
            Err(Failure::StreamUnavailable(message)) => {
                return Err(Failure::StreamUnavailable(message))
            }
            Err(Failure::Failed(message)) => {
                if crate::stream_install::is_permanent_stream_failure(&message) {
                    return Err(Failure::Failed(message));
                }
                if let Some(reason) = retry.exhausted(tuning) {
                    return Err(Failure::Failed(format!(
                        "The stream install kept failing ({reason}). Last error: {message}. The files already downloaded were kept; retry the install to resume."
                    )));
                }
                let delay = retry_delay(retry.attempt, tuning);
                on_retry(&message, retry.attempt.saturating_add(1), delay);
                tracing::warn!(
                    attempt = retry.attempt,
                    delay_ms = delay.as_millis() as u64,
                    error = %message,
                    "stream install failed; retrying the same job from its checkpoint"
                );
                if !sleep_or_stop(delay, cancel, pause, tuning.retry_now).await? {
                    return Err(Failure::Cancelled);
                }
            }
        }
    }
}

/// Production [`StreamAttempt`] over the real per-entry installer.
struct InstallStreamAttempt<'a> {
    app: &'a AppHandle,
    entry: &'a Arc<JobEntry>,
    params: &'a InstallParams,
    staging: &'a Path,
    job_paths: &'a mut JobPaths,
}

impl StreamAttempt for InstallStreamAttempt<'_> {
    async fn run(&mut self) -> Result<StreamOutcome, Failure> {
        crate::stream_install::install(
            self.app,
            self.entry,
            self.params,
            self.staging,
            self.job_paths,
        )
        .await
    }
}

async fn download(
    app: &AppHandle,
    entry: &Arc<JobEntry>,
    params: &InstallParams,
    part: &Path,
) -> Result<(), Failure> {
    let state = app.state::<AppState>();
    let client = state.http.clone();
    let settings = state.settings_snapshot();
    let bandwidth_kib = settings.bandwidth_limit_kib;
    let tuning = DownloadTuning {
        network: Some(&state.network),
        retry_now: Some(entry.retry_signal()),
        ..DownloadTuning::from_settings(
            settings.download_stall_timeout_secs,
            settings.download_retry_window_secs,
        )
    };
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
        tuning,
        move |update| {
            job_entry.update(|job| {
                job.phase = InstallPhase::Downloading;
                job.bytes_done = update.done;
                job.bytes_total = update.total;
                job.speed_bps = update.speed_bps;
                job.message = Some(update.message);
                job.reconnecting = update.reconnecting;
                job.attempt = update.attempt;
                job.last_error = update.last_error;
                job.stalled_for_ms = update.stalled_for_ms;
                job.next_retry_at = update.next_retry_at;
                job.waiting_for_network = update.waiting_for_network;
                // Keep the last HTTP status: an open error carries none, and
                // the diagnostics bundle still wants the failing response.
                job.http_status = update.http_status.or(job.http_status);
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
pub(crate) enum StreamOutcome {
    Done,
    /// The server cannot stream this version (non-ZIP, or a server build
    /// without the per-entry endpoints). Fall back to the next strategy.
    Unsupported(reqwest::StatusCode),
}

/// Convert a tar entry path to a safe relative path. Rejects absolute paths,
/// `..`, Windows drive/separator tricks and empty paths.
pub(crate) fn safe_tar_path(path: &Path) -> Option<PathBuf> {
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

/// Parse the first byte of `bytes <start>-<end>/<total>`.
fn parse_content_range_start(value: &str) -> Option<u64> {
    let range = value.trim().strip_prefix("bytes ")?;
    let (start, _) = range.split_once('-')?;
    start.trim().parse().ok()
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
        assert_eq!(parse_content_range_total("bytes */4096"), Some(4096));
        assert_eq!(parse_content_range_total("bytes */"), None);
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

    fn queued(job_id: &str, position: u32, created_at: &str) -> QueueJob {
        QueueJob {
            job_id: job_id.to_string(),
            phase: InstallPhase::Queued,
            position: Some(position),
            created_at: created_at.to_string(),
            running: false,
        }
    }

    fn running(job_id: &str, position: u32, created_at: &str) -> QueueJob {
        QueueJob {
            phase: InstallPhase::Downloading,
            ..queued(job_id, position, created_at)
        }
    }

    #[test]
    fn new_jobs_get_the_next_position() {
        assert_eq!(next_queue_position(&[]), 0);
        let jobs = vec![
            running("job-a", 0, "2026-01-01T00:00:00Z"),
            queued("job-b", 1, "2026-01-01T00:01:00Z"),
            queued("job-c", 2, "2026-01-01T00:02:00Z"),
        ];
        assert_eq!(next_queue_position(&jobs), 3);
        // A legacy row without a position sorts last and is ignored when
        // computing the next one.
        let mut legacy = queued("job-d", 0, "2026-01-01T00:03:00Z");
        legacy.position = None;
        let mut with_legacy = jobs.clone();
        with_legacy.push(legacy);
        assert_eq!(next_queue_position(&with_legacy), 3);
    }

    #[test]
    fn the_scheduler_picks_the_lowest_queue_position() {
        let jobs = vec![
            running("job-a", 0, "2026-01-01T00:00:00Z"),
            queued("job-c", 2, "2026-01-01T00:02:00Z"),
            queued("job-b", 1, "2026-01-01T00:01:00Z"),
        ];
        // The running job owns the slot; the lowest waiting position is next.
        assert_eq!(next_queued(&jobs).as_deref(), Some("job-b"));
        // A paused job never starts on its own: it waits for an explicit resume.
        let mut paused = queued("job-p", 0, "2026-01-01T00:00:30Z");
        paused.phase = InstallPhase::Paused;
        let mut with_paused = jobs.clone();
        with_paused.push(paused);
        assert_eq!(next_queued(&with_paused).as_deref(), Some("job-b"));
        // A claimed job (worker starting) is never picked twice.
        let mut claimed = queued("job-z", 0, "2026-01-01T00:00:10Z");
        claimed.running = true;
        let mut with_claimed = jobs.clone();
        with_claimed.push(claimed);
        assert_eq!(next_queued(&with_claimed).as_deref(), Some("job-b"));
        // Equal positions fall back to creation time, then to the job id.
        let tied = vec![
            queued("job-y", 4, "2026-01-01T00:05:00Z"),
            queued("job-x", 4, "2026-01-01T00:04:00Z"),
        ];
        assert_eq!(next_queued(&tied).as_deref(), Some("job-x"));
    }

    #[test]
    fn moving_the_third_job_to_the_top_starts_it_next() {
        let mut jobs = vec![
            running("job-a", 0, "2026-01-01T00:00:00Z"),
            queued("job-b", 1, "2026-01-01T00:01:00Z"),
            queued("job-c", 2, "2026-01-01T00:02:00Z"),
            queued("job-d", 3, "2026-01-01T00:03:00Z"),
        ];
        let assignments = plan_reorder(&jobs, "job-d", QueueMove::Top).unwrap();
        assert_eq!(
            assignments,
            vec![
                ("job-a".to_string(), 0),
                ("job-d".to_string(), 1),
                ("job-b".to_string(), 2),
                ("job-c".to_string(), 3),
            ]
        );
        apply_positions(&mut jobs, &assignments);
        assert_eq!(next_queued(&jobs).as_deref(), Some("job-d"));
        // Up/down one place swap neighbours and never jump the running job.
        let assignments = plan_reorder(&jobs, "job-c", QueueMove::Up).unwrap();
        assert_eq!(
            assignments,
            vec![
                ("job-a".to_string(), 0),
                ("job-d".to_string(), 1),
                ("job-c".to_string(), 2),
                ("job-b".to_string(), 3),
            ]
        );
        apply_positions(&mut jobs, &assignments);
        let assignments = plan_reorder(&jobs, "job-d", QueueMove::Down).unwrap();
        assert_eq!(assignments[1], ("job-c".to_string(), 1));
        assert_eq!(assignments[2], ("job-d".to_string(), 2));
        // The running job stays pinned at the front.
        assert_eq!(assignments[0], ("job-a".to_string(), 0));
    }

    fn apply_positions(jobs: &mut [QueueJob], assignments: &[(String, u32)]) {
        for (job_id, position) in assignments {
            if let Some(job) = jobs.iter_mut().find(|job| job.job_id == *job_id) {
                job.position = Some(*position);
            }
        }
    }

    #[test]
    fn running_and_finished_jobs_cannot_be_reordered() {
        let mut jobs = vec![
            running("job-a", 0, "2026-01-01T00:00:00Z"),
            queued("job-b", 1, "2026-01-01T00:01:00Z"),
        ];
        let error = plan_reorder(&jobs, "job-a", QueueMove::Top).unwrap_err();
        assert!(error.contains("is running"), "{error}");
        assert!(error.contains("Pause or cancel"), "{error}");

        let mut done = queued("job-done", 0, "2026-01-01T00:00:00Z");
        done.phase = InstallPhase::Done;
        jobs.push(done);
        let error = plan_reorder(&jobs, "job-done", QueueMove::Top).unwrap_err();
        assert!(error.contains("already finished"), "{error}");

        let error = plan_reorder(&jobs, "job-missing", QueueMove::Top).unwrap_err();
        assert!(error.contains("Unknown install job"), "{error}");
    }

    #[test]
    fn explicit_positions_are_validated() {
        let jobs = vec![
            queued("job-a", 0, "2026-01-01T00:00:00Z"),
            queued("job-b", 1, "2026-01-01T00:01:00Z"),
            queued("job-c", 2, "2026-01-01T00:02:00Z"),
        ];
        let assignments = plan_reorder(&jobs, "job-a", QueueMove::Position(2)).unwrap();
        assert_eq!(assignments[0], ("job-b".to_string(), 0));
        assert_eq!(assignments[1], ("job-c".to_string(), 1));
        assert_eq!(assignments[2], ("job-a".to_string(), 2));

        let error = plan_reorder(&jobs, "job-a", QueueMove::Position(3)).unwrap_err();
        assert!(error.contains("out of range"), "{error}");
        assert!(error.contains("3 waiting install"), "{error}");

        // Moving to the place it already has is a no-op, not an error.
        let assignments = plan_reorder(&jobs, "job-b", QueueMove::Position(1)).unwrap();
        assert_eq!(assignments[1], ("job-b".to_string(), 1));
    }

    #[test]
    fn an_empty_or_single_item_queue_is_a_no_op() {
        assert!(plan_reorder(&[], "job-a", QueueMove::Top).is_err());
        let single = vec![queued("job-a", 0, "2026-01-01T00:00:00Z")];
        assert_eq!(
            plan_reorder(&single, "job-a", QueueMove::Top).unwrap(),
            vec![("job-a".to_string(), 0)]
        );
        assert_eq!(
            plan_reorder(&single, "job-a", QueueMove::Up).unwrap(),
            vec![("job-a".to_string(), 0)]
        );
        assert_eq!(
            plan_reorder(&single, "job-a", QueueMove::Down).unwrap(),
            vec![("job-a".to_string(), 0)]
        );
        assert_eq!(
            plan_reorder(&single, "job-a", QueueMove::Position(0)).unwrap(),
            vec![("job-a".to_string(), 0)]
        );
    }

    #[test]
    fn a_finished_job_leaves_no_gap_in_the_queue() {
        // Running job 0 finishes: the waiting queue compacts to 0..n.
        let jobs = vec![
            queued("job-b", 1, "2026-01-01T00:01:00Z"),
            queued("job-c", 2, "2026-01-01T00:02:00Z"),
            queued("job-d", 3, "2026-01-01T00:03:00Z"),
        ];
        assert_eq!(
            plan_compact(&jobs),
            vec![
                ("job-b".to_string(), 0),
                ("job-c".to_string(), 1),
                ("job-d".to_string(), 2),
            ]
        );
        // A legacy row without a position goes last instead of blocking.
        let mut legacy = queued("job-e", 0, "2026-01-01T00:00:00Z");
        legacy.position = None;
        let mut with_legacy = jobs.clone();
        with_legacy.push(legacy);
        let compacted = plan_compact(&with_legacy);
        assert_eq!(compacted.last(), Some(&("job-e".to_string(), 3)));
    }

    #[test]
    fn queue_moves_parse_and_validate_their_action() {
        assert_eq!(QueueMove::parse("top", None).unwrap(), QueueMove::Top);
        assert_eq!(QueueMove::parse("up", None).unwrap(), QueueMove::Up);
        assert_eq!(QueueMove::parse("down", None).unwrap(), QueueMove::Down);
        assert_eq!(
            QueueMove::parse("position", Some(2)).unwrap(),
            QueueMove::Position(2)
        );
        let error = QueueMove::parse("position", None).unwrap_err();
        assert!(error.contains("position is required"), "{error}");
        let error = QueueMove::parse("sideways", Some(0)).unwrap_err();
        assert!(error.contains("Unknown queue action"), "{error}");
    }

    fn job_record(job_id: &str, phase: &str, position: Option<i64>) -> db::InstallJobRecord {
        db::InstallJobRecord {
            job_id: job_id.to_string(),
            game_id: 3,
            version_id: 31,
            phase: phase.to_string(),
            bytes_done: 0,
            bytes_total: None,
            speed_bps: 0,
            part_path: None,
            install_path: None,
            staging_path: None,
            error: None,
            stream: false,
            resumable: false,
            stream_completed: None,
            entries_total: None,
            queue_position: position,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn a_persisted_queue_position_survives_a_restart() {
        // After a restart the active rows are rebuilt from SQLite; the position
        // recorded by the previous session is what restores the order.
        let job = InstallJob::from_record(job_record("job-a", "queued", Some(2)));
        assert_eq!(job.queue_position, Some(2));
        // A row written by an older build has no position: it sorts last and
        // gets one on the next compaction instead of failing.
        assert_eq!(
            InstallJob::from_record(job_record("job-old", "queued", None)).queue_position,
            None
        );
        // An install interrupted by an app exit keeps its recorded slot, so
        // resuming restores the previous order.
        assert_eq!(
            InstallJob::from_record(job_record("job-crashed", "error", Some(1))).queue_position,
            Some(1)
        );
        // A finished row never re-enters the queue with a stale position.
        assert_eq!(
            InstallJob::from_record(job_record("job-done", "done", Some(1))).queue_position,
            None
        );
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
            DownloadTuning::default(),
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
            DownloadTuning::default(),
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
            DownloadTuning::default(),
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

    /* --------------------- injected-transport resume tests (#195) ---------------- */

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum After {
        End,
        Stall,
        Lost,
    }

    struct FakeConn {
        status: u16,
        declared_length: Option<u64>,
        range_total: Option<u64>,
        etag: Option<String>,
        body: Vec<u8>,
        after: After,
        /// Pause between chunks, to model a slow but healthy transfer.
        chunk_delay: Duration,
    }

    struct FakeTransport {
        conns: std::collections::VecDeque<FakeConn>,
        ranges: Arc<std::sync::Mutex<Vec<Option<u64>>>>,
    }

    struct FakeResponse {
        conn: FakeConn,
        sent: usize,
    }

    impl FakeTransport {
        fn new(conns: Vec<FakeConn>) -> (Self, Arc<std::sync::Mutex<Vec<Option<u64>>>>) {
            let ranges = Arc::new(std::sync::Mutex::new(Vec::new()));
            (
                Self {
                    conns: conns.into(),
                    ranges: Arc::clone(&ranges),
                },
                ranges,
            )
        }
    }

    impl DownloadTransport for FakeTransport {
        type Response = FakeResponse;

        async fn open(&mut self, range_start: Option<u64>) -> Result<Self::Response, String> {
            self.ranges.lock().unwrap().push(range_start);
            let conn = self
                .conns
                .pop_front()
                .ok_or_else(|| "the fake server ran out of scripted responses".to_string())?;
            Ok(FakeResponse { conn, sent: 0 })
        }
    }

    impl DownloadResponse for FakeResponse {
        fn status(&self) -> u16 {
            self.conn.status
        }

        fn content_length(&self) -> Option<u64> {
            self.conn.declared_length
        }

        fn content_range_total(&self) -> Option<u64> {
            self.conn.range_total
        }

        fn etag(&self) -> Option<String> {
            self.conn.etag.clone()
        }

        async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, ChunkError> {
            const STEP: usize = 16 * 1024;
            if self.sent < self.conn.body.len() {
                if !self.conn.chunk_delay.is_zero() {
                    tokio::time::sleep(self.conn.chunk_delay).await;
                }
                let end = (self.sent + STEP).min(self.conn.body.len());
                let chunk = self.conn.body[self.sent..end].to_vec();
                self.sent = end;
                return Ok(Some(chunk));
            }
            match self.conn.after {
                After::End => Ok(None),
                // A silent connection: the caller's idle timeout must fire.
                After::Stall => {
                    std::future::pending::<()>().await;
                    Ok(None)
                }
                After::Lost => Err(ChunkError::Lost("connection reset by peer".to_string())),
            }
        }

        async fn error_body(self) -> String {
            "fake error".to_string()
        }
    }

    fn test_tuning(idle_ms: u64, max_attempts: u32) -> DownloadTuning<'static> {
        DownloadTuning {
            idle_timeout: Duration::from_millis(idle_ms),
            max_attempts,
            max_retry_window: Some(Duration::from_secs(30)),
            base_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(5),
            ..DownloadTuning::default()
        }
    }

    #[tokio::test]
    async fn stall_resumes_at_the_exact_flushed_offset() {
        let payload: Vec<u8> = (0..300 * 1024).map(|index| (index % 251) as u8).collect();
        let split = 64 * 1024;
        let (mut transport, ranges) = FakeTransport::new(vec![
            FakeConn {
                status: 200,
                declared_length: Some(payload.len() as u64),
                range_total: None,
                etag: Some("\"v1\"".to_string()),
                body: payload[..split].to_vec(),
                after: After::Stall,
                chunk_delay: Duration::ZERO,
            },
            FakeConn {
                status: 206,
                declared_length: Some((payload.len() - split) as u64),
                range_total: Some(payload.len() as u64),
                etag: Some("\"v1\"".to_string()),
                body: payload[split..].to_vec(),
                after: After::End,
                chunk_delay: Duration::ZERO,
            },
        ]);

        let root = download_temp_dir("stall");
        let part = root.join("archive.part");
        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&updates);

        let result = download_into(
            &mut transport,
            &part,
            "Stalled game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(40, 4),
            move |update| sink.lock().unwrap().push(update),
        )
        .await;

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(std::fs::read(&part).unwrap(), payload);
        assert_eq!(
            *ranges.lock().unwrap(),
            vec![None, Some(split as u64)],
            "the resume must ask for the last flushed byte"
        );
        let updates = updates.lock().unwrap();
        let reconnect = updates
            .iter()
            .find(|update| update.reconnecting)
            .expect("a reconnecting progress update");
        assert_eq!(reconnect.done, split as u64);
        assert_eq!(reconnect.total, Some(payload.len() as u64));
        assert_eq!(reconnect.attempt, 2, "the resume is the second connection");
        assert!(reconnect.stalled_for_ms.is_some());
        assert!(
            reconnect
                .message
                .starts_with("Connection lost, resuming from"),
            "{}",
            reconnect.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_slow_but_progressing_transfer_is_not_a_stall() {
        let payload: Vec<u8> = vec![9u8; 128 * 1024];
        let (mut transport, ranges) = FakeTransport::new(vec![FakeConn {
            status: 200,
            declared_length: Some(payload.len() as u64),
            range_total: None,
            etag: None,
            body: payload.clone(),
            after: After::End,
            chunk_delay: Duration::from_millis(20),
        }]);
        let root = download_temp_dir("slow");
        let part = root.join("archive.part");

        // Eight 16 KiB chunks at 20 ms each: 160 ms total, far beyond the
        // 50 ms idle timeout, but no single read is ever silent that long.
        let result = download_into(
            &mut transport,
            &part,
            "Slow game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(50, 3),
            |_| {},
        )
        .await;

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(std::fs::read(&part).unwrap(), payload);
        assert_eq!(*ranges.lock().unwrap(), vec![None], "no reconnect needed");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn retries_are_bounded_with_a_clear_failure() {
        let payload = vec![7u8; 128 * 1024];
        let conns = (0..3)
            .map(|_| FakeConn {
                status: 200,
                declared_length: Some(payload.len() as u64),
                range_total: None,
                etag: None,
                body: Vec::new(),
                after: After::Lost,
                chunk_delay: Duration::ZERO,
            })
            .collect();
        let (mut transport, ranges) = FakeTransport::new(conns);
        let root = download_temp_dir("retry-bound");
        let part = root.join("archive.part");

        let error = download_into(
            &mut transport,
            &part,
            "Dead server",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(50, 3),
            |_| {},
        )
        .await
        .unwrap_err();

        let Failure::Failed(message) = error else {
            panic!("expected a final failure");
        };
        assert!(message.contains("3 times in a row"), "{message}");
        assert!(message.contains("partial archive was kept"), "{message}");
        assert_eq!(ranges.lock().unwrap().len(), 3, "max attempts is honoured");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn changed_archive_restarts_explicitly() {
        let first: Vec<u8> = vec![1u8; 100 * 1024];
        let second: Vec<u8> = vec![2u8; 120 * 1024];
        let split = 50 * 1024;
        let (mut transport, ranges) = FakeTransport::new(vec![
            FakeConn {
                status: 200,
                declared_length: Some(first.len() as u64),
                range_total: None,
                etag: Some("\"v1\"".to_string()),
                body: first[..split].to_vec(),
                after: After::Lost,
                chunk_delay: Duration::ZERO,
            },
            // Same total, different ETag: the archive was replaced in place.
            FakeConn {
                status: 206,
                declared_length: Some((first.len() - split) as u64),
                range_total: Some(first.len() as u64),
                etag: Some("\"v2\"".to_string()),
                body: Vec::new(),
                after: After::End,
                chunk_delay: Duration::ZERO,
            },
            FakeConn {
                status: 200,
                declared_length: Some(second.len() as u64),
                range_total: None,
                etag: Some("\"v2\"".to_string()),
                body: second.clone(),
                after: After::End,
                chunk_delay: Duration::ZERO,
            },
        ]);

        let root = download_temp_dir("changed");
        let part = root.join("archive.part");
        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&updates);

        download_into(
            &mut transport,
            &part,
            "Changed game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(50, 5),
            move |update| sink.lock().unwrap().push(update),
        )
        .await
        .expect("the download restarts from zero and completes");

        assert_eq!(std::fs::read(&part).unwrap(), second);
        assert_eq!(
            *ranges.lock().unwrap(),
            vec![None, Some(split as u64), None],
            "the restart must not reuse the stale offset"
        );
        assert!(
            updates
                .lock()
                .unwrap()
                .iter()
                .any(|update| update.message.contains("archive changed")),
            "the restart must be visible, not silent"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn permanent_http_errors_are_not_retried() {
        let (mut transport, ranges) = FakeTransport::new(vec![FakeConn {
            status: 403,
            declared_length: Some(0),
            range_total: None,
            etag: None,
            body: Vec::new(),
            after: After::End,
            chunk_delay: Duration::ZERO,
        }]);
        let root = download_temp_dir("forbidden");
        let part = root.join("archive.part");

        let error = download_into(
            &mut transport,
            &part,
            "Forbidden game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(50, 5),
            |_| {},
        )
        .await
        .unwrap_err();

        let Failure::Failed(message) = error else {
            panic!("expected a final failure");
        };
        assert!(message.contains("403"), "{message}");
        assert_eq!(ranges.lock().unwrap().len(), 1, "a 403 is final");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn retry_backoff_grows_and_stays_capped() {
        let tuning = DownloadTuning {
            base_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_millis(800),
            ..DownloadTuning::default()
        };
        let first = retry_delay(1, tuning);
        let third = retry_delay(3, tuning);
        assert!(first >= Duration::from_millis(75), "{first:?}");
        assert!(third <= tuning.max_backoff, "{third:?}");
        assert!(third >= Duration::from_millis(300), "{third:?}");
        assert_eq!(
            DownloadTuning::from_idle_secs(1).idle_timeout,
            Duration::from_secs(5)
        );
        assert_eq!(
            DownloadTuning::from_idle_secs(900).idle_timeout,
            Duration::from_secs(600)
        );
    }

    #[test]
    fn the_retry_window_defaults_to_six_hours_and_can_be_unlimited() {
        let tuning = DownloadTuning::default();
        assert_eq!(tuning.max_retry_window, Some(Duration::from_secs(6 * 3600)));
        assert_eq!(tuning.max_attempts, 50);
        assert_eq!(tuning.max_backoff, Duration::from_secs(5 * 60));

        let unlimited = DownloadTuning::from_settings(30, 0);
        assert_eq!(unlimited.max_retry_window, None);
        assert_eq!(
            unlimited.max_attempts,
            u32::MAX,
            "an unlimited window must not be capped by the attempt counter"
        );

        // The window boundary: one second past 6 h is exhausted, one second
        // before it still retries.
        let mut retry = RetryState::new();
        retry.next_attempt();
        retry.retry_started = Some(Instant::now() - Duration::from_secs(6 * 3600 + 1));
        assert!(retry
            .exhausted(tuning)
            .expect("window exhausted")
            .contains("6 h"));
        retry.retry_started = Some(Instant::now() - Duration::from_secs(6 * 3600 - 60));
        assert!(
            retry.exhausted(tuning).is_none(),
            "inside the window, retrying continues"
        );
    }

    #[test]
    fn repeated_identical_permanent_errors_trip_the_circuit_breaker() {
        let mut retry = RetryState::new();
        let message = "Download failed with HTTP 404: gone";
        assert!(retry.record_failure(message, true).is_none());
        assert!(retry.record_failure(message, true).is_none());
        let reason = retry
            .record_failure(message, true)
            .expect("third identical permanent error stops the loop");
        assert!(reason.contains("3 times"), "{reason}");
        // A transient error repeating is normal during an outage: no breaker.
        let mut transient = RetryState::new();
        for _ in 0..10 {
            assert!(transient
                .record_failure("connection reset by peer", false)
                .is_none());
        }
    }

    #[test]
    fn retry_after_is_honoured_and_capped() {
        assert_eq!(parse_retry_after(" 30 "), Some(Duration::from_secs(30)));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after("soon"), None);
        let base = Duration::from_secs(2);
        assert_eq!(
            retry_delay_with_hint(base, Some(Duration::from_secs(45))),
            Duration::from_secs(45)
        );
        // A shorter Retry-After never shortens the exponential floor.
        assert_eq!(
            retry_delay_with_hint(base, Some(Duration::from_secs(1))),
            base
        );
        // A bogus huge header is capped instead of parking the job for hours.
        assert_eq!(
            retry_delay_with_hint(base, Some(Duration::from_secs(24 * 3600))),
            Duration::from_secs(30 * 60)
        );
    }

    #[tokio::test]
    async fn twenty_consecutive_failures_then_success() {
        let payload: Vec<u8> = (0..64 * 1024).map(|index| (index % 251) as u8).collect();
        let mut conns: Vec<FakeConn> = (0..20)
            .map(|_| FakeConn {
                status: 503,
                declared_length: Some(0),
                range_total: None,
                etag: None,
                body: Vec::new(),
                after: After::End,
                chunk_delay: Duration::ZERO,
            })
            .collect();
        conns.push(FakeConn {
            status: 200,
            declared_length: Some(payload.len() as u64),
            range_total: None,
            etag: None,
            body: payload.clone(),
            after: After::End,
            chunk_delay: Duration::ZERO,
        });
        let (mut transport, ranges) = FakeTransport::new(conns);
        let root = download_temp_dir("twenty");
        let part = root.join("archive.part");
        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&updates);

        download_into(
            &mut transport,
            &part,
            "Flaky game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(50, 25),
            move |update| sink.lock().unwrap().push(update),
        )
        .await
        .expect("a 20-failure burst must still complete");

        assert_eq!(std::fs::read(&part).unwrap(), payload);
        assert_eq!(
            ranges.lock().unwrap().len(),
            21,
            "twenty retries then the successful connection"
        );
        let last = updates.lock().unwrap().last().cloned();
        let last = last.expect("a completion update");
        assert_eq!(last.attempt, 21, "the job reports the final attempt");
        assert_eq!(last.last_error, None);
        assert_eq!(last.next_retry_at, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_404_stops_at_once_and_keeps_the_part() {
        let (mut transport, ranges) = FakeTransport::new(vec![FakeConn {
            status: 404,
            declared_length: Some(0),
            range_total: None,
            etag: None,
            body: Vec::new(),
            after: After::End,
            chunk_delay: Duration::ZERO,
        }]);
        let root = download_temp_dir("not-found");
        let part = root.join("archive.part");
        std::fs::write(&part, b"kept").unwrap();

        let error = download_into(
            &mut transport,
            &part,
            "Missing game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(50, 5),
            |_| {},
        )
        .await
        .unwrap_err();

        assert!(matches!(error, Failure::Failed(_)));
        assert_eq!(ranges.lock().unwrap().len(), 1, "a 404 is final");
        assert_eq!(std::fs::read(&part).unwrap(), b"kept");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_offline_machine_does_not_open_a_connection_or_burn_an_attempt() {
        let payload = vec![5u8; 32 * 1024];
        let (mut transport, ranges) = FakeTransport::new(vec![FakeConn {
            status: 200,
            declared_length: Some(payload.len() as u64),
            range_total: None,
            etag: None,
            body: payload.clone(),
            after: After::End,
            chunk_delay: Duration::ZERO,
        }]);
        let gate = crate::state::NetworkGate::new();
        gate.set_online(false);
        let tuning = DownloadTuning {
            network: Some(&gate),
            ..test_tuning(50, 5)
        };
        let root = download_temp_dir("offline");
        let part = root.join("archive.part");

        let offline = tokio::time::timeout(
            Duration::from_millis(120),
            download_into(
                &mut transport,
                &part,
                "Offline game",
                &CancellationToken::new(),
                &CancellationToken::new(),
                0,
                tuning,
                |_| {},
            ),
        )
        .await;
        assert!(
            offline.is_err(),
            "the download must wait while the OS reports offline"
        );
        assert!(
            ranges.lock().unwrap().is_empty(),
            "no connection may be opened while offline"
        );

        gate.set_online(true);
        download_into(
            &mut transport,
            &part,
            "Offline game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            tuning,
            |_| {},
        )
        .await
        .expect("the retry starts as soon as the network returns");
        assert_eq!(std::fs::read(&part).unwrap(), payload);
        assert_eq!(*ranges.lock().unwrap(), vec![None]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn retry_now_skips_the_backoff_and_cancel_stops_it() {
        let signal = tokio::sync::Notify::new();
        let started = Instant::now();
        let cancel = CancellationToken::new();
        let pause = CancellationToken::new();
        let waiter = sleep_or_stop(Duration::from_secs(30), &cancel, &pause, Some(&signal));
        tokio::pin!(waiter);
        tokio::select! {
            result = &mut waiter => panic!("the backoff ended too early: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
        signal.notify_waiters();
        assert!(waiter.await.unwrap(), "retry now wakes the wait");
        assert!(started.elapsed() < Duration::from_secs(2));

        let cancel = CancellationToken::new();
        cancel.cancel();
        let started = Instant::now();
        assert!(!sleep_or_stop(
            Duration::from_secs(30),
            &cancel,
            &CancellationToken::new(),
            None
        )
        .await
        .unwrap());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    struct ScriptedStreamAttempt {
        results: Vec<Result<StreamOutcome, Failure>>,
        runs: usize,
    }

    impl StreamAttempt for ScriptedStreamAttempt {
        async fn run(&mut self) -> Result<StreamOutcome, Failure> {
            self.runs += 1;
            if self.results.is_empty() {
                return Ok(StreamOutcome::Done);
            }
            self.results.remove(0)
        }
    }

    fn stream_retry_tuning() -> DownloadTuning<'static> {
        DownloadTuning {
            base_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(2),
            max_retry_window: Some(Duration::from_secs(30)),
            max_attempts: 5,
            ..DownloadTuning::default()
        }
    }

    #[tokio::test]
    async fn an_exhausted_stream_entry_retries_the_job_from_its_checkpoint() {
        let mut attempt = ScriptedStreamAttempt {
            results: vec![
                Err(Failure::Failed(
                    "Entry Game/big.bin failed after 5 attempts (1048576 bytes kept): proxy returned HTTP 524"
                        .to_string(),
                )),
                Ok(StreamOutcome::Done),
            ],
            runs: 0,
        };
        let retries = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&retries);
        let outcome = stream_with_retry(
            &mut attempt,
            stream_retry_tuning(),
            &CancellationToken::new(),
            &CancellationToken::new(),
            move |message, next_attempt, delay| {
                sink.lock()
                    .unwrap()
                    .push((message.to_string(), next_attempt, delay));
            },
        )
        .await;

        assert!(matches!(outcome, Ok(StreamOutcome::Done)), "{outcome:?}");
        assert_eq!(attempt.runs, 2, "the job runs again instead of failing");
        let retries = retries.lock().unwrap();
        assert_eq!(retries.len(), 1);
        assert_eq!(retries[0].1, 2, "the retry is visible as attempt 2");
        assert!(retries[0].0.contains("524"), "{}", retries[0].0);
    }

    #[tokio::test]
    async fn a_permanent_stream_error_is_not_retried() {
        let mut attempt = ScriptedStreamAttempt {
            results: vec![Err(Failure::Failed(
                "Entry Game/big.bin failed with HTTP 404. The game version or entry was not found on the server."
                    .to_string(),
            ))],
            runs: 0,
        };
        let outcome = stream_with_retry(
            &mut attempt,
            stream_retry_tuning(),
            &CancellationToken::new(),
            &CancellationToken::new(),
            |_, _, _| panic!("a 404 must not be retried"),
        )
        .await;
        assert!(matches!(outcome, Err(Failure::Failed(_))));
        assert_eq!(attempt.runs, 1);
    }

    #[tokio::test]
    async fn an_unusable_stream_path_returns_for_the_classic_fallback() {
        let mut attempt = ScriptedStreamAttempt {
            results: vec![Err(Failure::StreamUnavailable(
                "entry game.exe could not be reached after 5 attempts: HTTP 524".to_string(),
            ))],
            runs: 0,
        };
        let outcome = stream_with_retry(
            &mut attempt,
            stream_retry_tuning(),
            &CancellationToken::new(),
            &CancellationToken::new(),
            |_, _, _| panic!("an unusable stream path falls back instead of retrying"),
        )
        .await;
        assert!(matches!(outcome, Err(Failure::StreamUnavailable(_))));
        assert_eq!(attempt.runs, 1);
    }

    #[tokio::test]
    async fn cancel_stops_a_stream_retry_immediately() {
        let mut attempt = ScriptedStreamAttempt {
            results: vec![Err(Failure::Failed("proxy returned 502".to_string()))],
            runs: 0,
        };
        let cancel = CancellationToken::new();
        cancel.cancel();
        let started = Instant::now();
        let outcome = stream_with_retry(
            &mut attempt,
            DownloadTuning {
                base_backoff: Duration::from_secs(30),
                ..stream_retry_tuning()
            },
            &cancel,
            &CancellationToken::new(),
            |_, _, _| {},
        )
        .await;
        assert!(matches!(outcome, Err(Failure::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(attempt.runs, 1);
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

    #[test]
    fn resume_mode_parses_and_maps_the_stream_flag() {
        assert_eq!(ResumeMode::parse(None).unwrap(), ResumeMode::Keep);
        assert_eq!(ResumeMode::parse(Some("")).unwrap(), ResumeMode::Keep);
        assert_eq!(
            ResumeMode::parse(Some("download")).unwrap(),
            ResumeMode::Download
        );
        assert_eq!(
            ResumeMode::parse(Some("stream")).unwrap(),
            ResumeMode::Stream
        );
        assert!(ResumeMode::parse(Some("teleport")).is_err());

        assert!(ResumeMode::Stream.stream_flag(false));
        assert!(!ResumeMode::Download.stream_flag(true));
        assert!(ResumeMode::Keep.stream_flag(true));
        assert!(!ResumeMode::Keep.stream_flag(false));
    }
}
