//! Install pipeline: resolve the archive, download with resume, extract to a
//! staging folder (or copy a single-file game), detect the executable, move
//! into place and index the result.
//!
//! Jobs are persisted in SQLite so an interrupted download stays visible after
//! a restart and the next attempt resumes the `.part` file with HTTP Range.
//! Only one job runs at a time by default; the rest wait in the queued phase.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::db::{self, InstallManifestRecord, InstalledGame};
use crate::manifest;
use crate::paths;
use crate::pipeline::{self, Admission, Candidate};
use crate::settings::{self, LocalSettings, Mode};
use crate::space::{self, SpacePlan, SpaceStatus};
use crate::state::{AppState, JobEntry, SlotGuard};
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
    /// The head of the queue has no disk room yet (#222). It keeps its queue
    /// slot and starts by itself when a reservation is released.
    #[serde(rename = "waitingForSpace")]
    WaitingForSpace,
    Downloading,
    /// The archive is being re-read for its SHA-256 (#223). Disk/CPU work, not
    /// network: the next download may already run.
    Verifying,
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
            InstallPhase::WaitingForSpace => "waiting_for_space",
            InstallPhase::Downloading => "downloading",
            InstallPhase::Verifying => "verifying",
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
            "waiting_for_space" => InstallPhase::WaitingForSpace,
            "downloading" => InstallPhase::Downloading,
            "verifying" => InstallPhase::Verifying,
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
    /// True while this archive is fetched ahead of its turn, i.e. another job
    /// holds the extractor (#221). The UI says "Downloading ahead" instead of
    /// implying the archive is already being unpacked.
    pub prefetching: bool,
    /// Worst-case disk requirement of the job and the volume it applies to
    /// (#222). `None` when no size is known yet; `ok: false` while the job
    /// waits for space.
    pub space: Option<space::SpaceStatus>,
    /// Full archive SHA-256 that was verified before extraction (#216/#223).
    pub verified_sha256: Option<String>,
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
                InstallPhase::WaitingForSpace => "Waiting for disk space".to_string(),
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
            prefetching: false,
            space: None,
            verified_sha256: record.verified_sha256,
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
    /// touched the disk yet, or they keep their checkpoint while paused. A job
    /// parked on disk space (#222) still holds its place and can be moved.
    fn is_waiting(&self) -> bool {
        matches!(
            self.phase,
            InstallPhase::Queued | InstallPhase::WaitingForSpace | InstallPhase::Paused
        )
    }

    fn is_running(&self) -> bool {
        self.running
            || matches!(
                self.phase,
                InstallPhase::Downloading
                    | InstallPhase::Verifying
                    | InstallPhase::Extracting
                    | InstallPhase::Finalizing
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

/// The job the scheduler starts next: the lowest position among the jobs that
/// wait for a worker. Mirrors the queue order `pipeline::plan_next` enforces;
/// kept here because it is the rule the reorder tests assert against.
#[cfg(test)]
pub(crate) fn next_queued(jobs: &[QueueJob]) -> Option<String> {
    jobs.iter()
        .filter(|job| {
            matches!(
                job.phase,
                InstallPhase::Queued | InstallPhase::WaitingForSpace
            ) && !job.running
        })
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

/// Jobs a worker may start: waiting for a slot, not paused, not owned.
fn candidates(state: &AppState) -> Vec<Candidate> {
    let Ok(jobs) = state.jobs.lock() else {
        return Vec::new();
    };
    jobs.values()
        .filter_map(|entry| {
            let job = entry.snapshot();
            if job.phase.is_terminal() || entry.running.load(Ordering::SeqCst) {
                return None;
            }
            let startable = matches!(
                job.phase,
                InstallPhase::Queued | InstallPhase::WaitingForSpace
            );
            if !startable {
                return None;
            }
            Some(Candidate {
                job_id: job.job_id.clone(),
                position: job.queue_position,
                created_at: entry.created_at.clone(),
                phase: job.phase,
                space: Some(plan_for_job(state, entry)),
            })
        })
        .collect()
}

/// Worst-case disk requirement of one job (#222).
///
/// `bytes_total` is the archive size once the server told us; before that the
/// UI estimate (`expectedBytes`) is used. The extracted size is unknown, so the
/// plan estimates it at 1.5× and flags it.
fn plan_for_job(state: &AppState, entry: &JobEntry) -> SpacePlan {
    let settings = state.settings_snapshot();
    let job = entry.snapshot();
    let install_root = entry
        .params
        .install_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| settings.install_path());
    let download_root = entry
        .params
        .download_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| settings.download_path());
    SpacePlan::build(
        job.bytes_total.or(entry.params.expected_bytes),
        None,
        &install_root,
        &download_root,
        job.stream,
        settings.keep_archives,
    )
}

/// Park the head of the queue on its disk requirement (#222).
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

/// Re-evaluate a job parked on disk space ("Check again" in Downloads, #222).
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

/// Can this persisted job still need its staging folder (#225)?
///
/// A resumable stream job keeps its checkpoint across a failure or a pause, so
/// its folder is never swept. A `done` job is never protected, even if an older
/// build left `resumable` set: its staging was moved into place, so any folder
/// still on disk is a leftover the sweep may remove.
fn staging_is_protected(record: &db::InstallJobRecord) -> bool {
    let phase = InstallPhase::parse(&record.phase);
    phase != InstallPhase::Done && (record.resumable || !phase.is_terminal())
}

/// Staging folders a live or resumable job still owns (#225).
///
/// Built from the in-memory queue and from SQLite: a job that was interrupted
/// in a previous app run has no in-memory entry, and deleting its checkpoint
/// would force a full re-download.
fn protected_staging(app: &AppHandle) -> HashSet<PathBuf> {
    let state = app.state::<AppState>();
    let mut protected = HashSet::new();

    if let Ok(jobs) = state.jobs.lock() {
        for entry in jobs.values() {
            let job = entry.snapshot();
            let active = job.resumable || !job.phase.is_terminal();
            if job.phase == InstallPhase::Done || !active {
                continue;
            }
            if let Some(path) = entry.staging_path() {
                protected.insert(PathBuf::from(path));
            }
        }
    }

    if let Ok(connection) = state.db.lock() {
        match db::all_install_jobs(&connection) {
            Ok(records) => {
                for record in records {
                    if !staging_is_protected(&record) {
                        continue;
                    }
                    if let Some(path) = record.staging_path {
                        protected.insert(PathBuf::from(path));
                    }
                }
            }
            Err(error) => {
                // Never sweep blind: without the persisted jobs a resumable
                // checkpoint could be treated as stale.
                tracing::warn!(error = %error, "could not read the persisted install jobs for the staging sweep");
            }
        }
    }

    protected
}

/// Install roots and protected staging folders for the startup sweep (#225).
///
/// Roots: the default install folder, every configured install location and
/// the install folder of every persisted job (a location can be removed while
/// a job still points at it). A root that no longer exists is harmless: the
/// sweep skips it.
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

/// Staging folders a finished job may remove (#225).
///
/// Its own folder (unless it is the final path) plus stale siblings of the
/// same slug that no job owns. `protected` holds [`paths::path_key`] values, so
/// a differently spelled path still protects its folder.
fn success_staging_candidates(
    staging: &Path,
    final_path: &Path,
    slug: &str,
    protected: &HashSet<String>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let final_key = paths::path_key(final_path);
    if staging.is_dir() && paths::path_key(staging) != final_key {
        candidates.push(staging.to_path_buf());
    }
    let Some(root) = staging.parent() else {
        return candidates;
    };
    let Ok(entries) = std::fs::read_dir(root) else {
        return candidates;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(candidate_slug) = paths::staging_dir_slug(&name) else {
            continue;
        };
        if !candidate_slug.eq_ignore_ascii_case(slug) {
            continue;
        }
        let path = entry.path();
        if !path.is_dir() || paths::path_key(&path) == final_key {
            continue;
        }
        if path == staging || protected.contains(&paths::path_key(&path)) {
            continue;
        }
        candidates.push(path);
    }
    candidates
}

/// Delete the staging folder of a job that just finished (#225) and any stale
/// sibling staging folder of the same game.
///
/// Never fails the install: every error is logged and the final folder is
/// already in place. `final_path` is checked so a bug can never delete the
/// installed game.
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

/// Clears the `running` flag when the worker exits, however it exits.
struct RunGuard<'a>(&'a AtomicBool);

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

async fn run(app: AppHandle, job_id: String, download_slot: SlotGuard) {
    let mut download_slot = Some(download_slot);
    run_job(app.clone(), job_id, &mut download_slot).await;
    // Both slots are free again: start the next download in queue order. This
    // also picks up a resume that landed while the worker was finishing a
    // pause, so a job never stays queued without a worker.
    drop(download_slot);
    dispatch(&app);
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

    let result = execute(
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
            // The staging folder was moved into place: the per-entry
            // checkpoint is no longer needed.
            clear_stream_checkpoint(&app, &entry);
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
    release_space(app, entry);
    entry.update(|job| {
        job.phase = InstallPhase::Paused;
        job.speed_bps = 0;
        job.prefetching = false;
        job.clear_reconnect();
        job.message = Some("Paused".to_string());
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

/// Full archive SHA-256 the server published for this version (#216).
///
/// Read from the game detail payload (players can load it, unlike the editor
/// version list). A missing digest means a legacy row or a backfill still
/// running: the install proceeds with a warning instead of blocking. A failed
/// request is logged and treated the same way, so an offline/local install is
/// never held hostage by metadata.
async fn expected_archive_sha256(state: &AppState, params: &InstallParams) -> Option<String> {
    #[derive(Deserialize)]
    struct Detail {
        #[serde(default)]
        versions: Vec<VersionDigest>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct VersionDigest {
        id: i64,
        content_sha256: Option<String>,
    }

    let base = params.server_url.trim_end_matches('/');
    if base.is_empty() {
        return None;
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
                "could not read the archive hash; installing without strong verification"
            );
            return None;
        }
    };
    if !response.status().is_success() {
        tracing::warn!(
            game_id = params.game_id,
            version_id = params.version_id,
            status = response.status().as_u16(),
            "the server did not return the game detail; installing without strong verification"
        );
        return None;
    }
    let Ok(detail) = response.json::<Detail>().await else {
        return None;
    };
    let digest = detail
        .versions
        .into_iter()
        .find(|version| version.id == params.version_id)
        .and_then(|version| version.content_sha256)
        .map(|digest| digest.trim().to_ascii_lowercase())
        .filter(|digest| digest.len() == 64);
    match digest.as_deref() {
        Some(digest) => tracing::info!(
            game_id = params.game_id,
            version_id = params.version_id,
            sha256 = digest,
            "server published the archive SHA-256; it will be verified before install"
        ),
        None => tracing::warn!(
            "{}",
            crate::archive_verify::legacy_warning(params.game_id, params.version_id)
        ),
    }
    digest
}

/// The staging folder the transport actually wrote to (#225).
///
/// A fresh job writes to its own `.{slug}.staging-{job id}` folder, but
/// `stream_install` adopts the checkpoint of a previous attempt and records
/// that folder in [`JobPaths::staging`]. The commit must use it, otherwise the
/// move reads an empty folder and leaves the full checkpoint behind.
fn effective_staging(job_paths: &JobPaths, fallback: &Path) -> PathBuf {
    job_paths
        .staging
        .clone()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| fallback.to_path_buf())
}

async fn execute(
    app: &AppHandle,
    entry: &Arc<JobEntry>,
    job_paths: &mut JobPaths,
    download_slot: &mut Option<SlotGuard>,
    extract_slot: &mut Option<SlotGuard>,
) -> Result<ExecutedInstall, Failure> {
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

    // Full archive hash published by the server (#216). Read once here so both
    // the local shortcut and the classic download verify the same value.
    let mut expected_sha256 = expected_archive_sha256(&state, &params).await;
    // Set when the classic transfer already proved the digest for `archive`.
    let mut verified_sha256: Option<String> = None;

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
        // #221: a stream install is network *and* extraction, and it writes
        // entry by entry while it downloads. It holds **both** slots for its
        // whole duration, so it can never interleave with another unpacker; the
        // documented policy is "stream = download + extract, no prefetch".
        hold_extract(app, entry, extract_slot).await?;
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
                // The job is a normal download now: give the extractor back so
                // another install can prefetch/unpack while it transfers (#221).
                drop(extract_slot.take());
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
                drop(extract_slot.take());
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
        // The classic path writes to the current job's own folder: make that
        // the effective staging before the commit reads it (#225).
        job_paths.staging = Some(staging.clone());
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
            let report = download(app, entry, &params, &part).await?;
            check_cancel(&entry.cancel)?;
            release_download(app, download_slot);
            entry.update(|job| job.prefetching = false);
            hold_extract(app, entry, extract_slot).await?;

            let verdict = verify_archive_on_disk(
                app,
                entry,
                &params,
                &part,
                expected_sha256.as_deref(),
                report.served_sha256.as_deref(),
                report.sha256.as_deref(),
                report.etag.as_deref(),
            )
            .await?;
            match verdict {
                crate::archive_verify::ArchiveVerdict::Verified(digest) => {
                    tracing::info!(
                        game_id = params.game_id,
                        version_id = params.version_id,
                        bytes = report.bytes,
                        resumed = report.resumed,
                        sha256 = %digest,
                        "archive verified against the server SHA-256"
                    );
                    verified_sha256 = Some(digest);
                    break;
                }
                crate::archive_verify::ArchiveVerdict::Unverified => break,
                crate::archive_verify::ArchiveVerdict::Mismatch(mismatch) => {
                    // Never extract these bytes: remove them before anything
                    // else can read the `.part`.
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
                                "The archive changed on the server; downloading it again"
                                    .to_string()
                            } else {
                                "The download failed verification; retrying from 0%".to_string()
                            }),
                        );
                        // The archive may have been replaced since the first
                        // metadata read: refresh before re-downloading.
                        if let Some(fresh) = expected_archive_sha256(&state, &params).await {
                            expected_sha256 = Some(fresh);
                        }
                        drop(extract_slot.take());
                        continue;
                    }
                    return Err(Failure::Failed(mismatch.message(&params.title)));
                }
            }
        }
        archive = Some(part.clone());
        downloaded = Some(part);
    }

    check_cancel(&entry.cancel)?;

    // #225: `stream_install` can adopt the staging folder of a previous job
    // and records it in `job_paths.staging`. Everything that inspects or
    // commits staged files must use the folder the transport actually wrote
    // to; the locally computed path is only a fallback.
    let effective_staging = effective_staging(job_paths, &staging);

    // Verification and extraction are disk work: one at a time (#221). The
    // stream path already took the extractor; the classic path took it in the
    // loop above; the local shortcut takes it here.
    hold_extract(app, entry, extract_slot).await?;

    // The local shortcut reads the library file directly: verify it too, once,
    // before extraction. The classic path already proved its digest above.
    if !streamed && verified_sha256.is_none() {
        if let Some(path) = archive.clone() {
            match verify_archive_on_disk(
                app,
                entry,
                &params,
                &path,
                expected_sha256.as_deref(),
                None,
                None,
                None,
            )
            .await?
            {
                crate::archive_verify::ArchiveVerdict::Verified(digest) => {
                    tracing::info!(
                        game_id = params.game_id,
                        version_id = params.version_id,
                        sha256 = %digest,
                        "local archive verified against the server SHA-256"
                    );
                    verified_sha256 = Some(digest);
                }
                crate::archive_verify::ArchiveVerdict::Unverified => {}
                crate::archive_verify::ArchiveVerdict::Mismatch(mismatch) => {
                    // A library file is never deleted: report and stop.
                    return Err(Failure::Failed(mismatch.message(&params.title)));
                }
            }
        }
    }

    if let Some(digest) = verified_sha256.clone() {
        entry.update(|job| job.verified_sha256 = Some(digest));
        persist(app, entry);
    }

    // Stream installs retain no archive: the per-entry CRC-32 and size checks
    // are the verification model, recorded in the stream manifest (#216).
    if streamed {
        tracing::info!(
            game_id = params.game_id,
            version_id = params.version_id,
            "{}",
            crate::archive_verify::stream_note(expected_sha256.as_deref())
        );
    }

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
        let staging_for_exe = effective_staging.clone();
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

            tokio::fs::create_dir_all(&effective_staging)
                .await
                .map_err(|e| {
                    Failure::Failed(format!(
                        "Could not create the staging folder {}: {e}",
                        effective_staging.display()
                    ))
                })?;
            let destination = paths::safe_child(&effective_staging, &file_name)?;

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
            .map_err(|e| {
                Failure::Failed(download_write_failure(
                    &destination,
                    &std::io::Error::other(e),
                ))
            })?;

            if let Some(part) = downloaded.as_deref() {
                cleanup_archive(part, settings.keep_archives, &file_name).await;
            }

            Some(PathBuf::from(&file_name))
        } else {
            // 3b. Extract the archive into the staging folder.
            // #222: the ledger reserved the peak at admission, but the disk may
            // have been filled by something else since: fail before unpacking
            // and keep the archive so the job resumes after freeing space.
            ensure_extraction_space(&install_root, archive_size)?;
            set_progress(
                app,
                entry,
                InstallPhase::Extracting,
                0,
                archive_size,
                0,
                Some("Extracting archive".to_string()),
            );

            let staging_for_extract = effective_staging.clone();
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
            extraction
                .map_err(|error| Failure::Failed(extraction_failure(&effective_staging, error)))?;

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

            let staging_for_exe = effective_staging.clone();
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

    // 5. Move staging into place (rename, or copy across devices). For a stream
    // install this is the folder it adopted from the previous attempt (#225).
    let staging_for_move = effective_staging;
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

    // #214: a repack archive finishes with no executable. Detect the setup
    // program now so the job can point the user at the guided flow; nothing is
    // executed here.
    let pending_setup = if executable_path.is_none() {
        let root = final_path.clone();
        tokio::task::spawn_blocking(move || crate::installer::detect_repack(&root))
            .await
            .unwrap_or(None)
    } else {
        None
    };

    Ok(ExecutedInstall {
        install_path: final_path,
        pending_setup,
    })
}

/// What [`execute`] produced: the final folder, plus the setup payload found
/// when no game executable could be recorded (#214).
struct ExecutedInstall {
    install_path: PathBuf,
    pending_setup: Option<crate::installer::RepackLayout>,
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

/// Wait for a slot without losing cancel/pause responsiveness (#221).
async fn acquire_slot(
    entry: &JobEntry,
    limiter: &Arc<crate::state::SlotLimiter>,
) -> Result<SlotGuard, Failure> {
    let pause = entry.pause_token();
    tokio::select! {
        biased;
        _ = entry.cancel.cancelled() => Err(Failure::Cancelled),
        _ = pause.cancelled() => Err(Failure::Paused),
        guard = limiter.acquire() => Ok(guard),
    }
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
    let limiter = Arc::clone(&app.state::<AppState>().download_slots);
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
    let limiter = Arc::clone(&app.state::<AppState>().extract_slots);
    *slot = Some(acquire_slot(entry, &limiter).await?);
    Ok(())
}

/// "Verifying archive (42.5 GiB of 96.3 GiB)" (#223).
pub(crate) fn verifying_message(done: u64, total: u64) -> String {
    if total > 0 {
        format!(
            "Verifying archive ({} of {})",
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
async fn verify_archive_on_disk(
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

fn hash_failure(failure: crate::archive_verify::HashFailure) -> Failure {
    match failure {
        crate::archive_verify::HashFailure::Cancelled => Failure::Cancelled,
        crate::archive_verify::HashFailure::Paused => Failure::Paused,
        crate::archive_verify::HashFailure::Failed(message) => Failure::Failed(message),
    }
}

/// Re-check the install volume before unpacking (#222).
///
/// The ledger reserved the peak at admission, but the disk can be filled by
/// something outside Ludex between the check and the extraction. The extracted
/// size is the archive × 1.5 when unknown, plus the safety margin.
fn ensure_extraction_space(install_root: &Path, archive_bytes: Option<u64>) -> Result<(), Failure> {
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
    /// Full archive SHA-256 advertised by the server (#216), when present.
    fn archive_sha256(&self) -> Option<String> {
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

    fn archive_sha256(&self) -> Option<String> {
        self.response
            .headers()
            .get(crate::archive_verify::ARCHIVE_SHA256_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
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

/// What one completed transfer produced, for verification (#216).
#[derive(Debug, Clone, Default)]
pub(crate) struct DownloadReport {
    /// SHA-256 computed while writing, set only when every byte was written
    /// once from offset 0 in this run. A resumed transfer leaves it `None` and
    /// the caller rehashes the file from byte 0.
    pub sha256: Option<String>,
    /// Bytes in the finished file.
    pub bytes: u64,
    /// Full archive SHA-256 the server advertised on the last response.
    pub served_sha256: Option<String>,
    /// ETag of the last response, part of the verification cache key (#223).
    pub etag: Option<String>,
    /// A Range resume or an append happened: the file is not a single
    /// from-zero write.
    pub resumed: bool,
}

/// Download `url` into `part`, resuming an existing file with HTTP Range.
///
/// A silent response is abandoned after `tuning.idle_timeout` and the next
/// request resumes at the last flushed byte; reconnects are bounded by
/// `tuning` (#195). The `.part` file is kept when the user pauses; cancelling
/// is handled by the caller (which deletes it). `progress` is called at most
/// every 250 ms.
///
/// The returned [`DownloadReport`] carries the digests needed to verify the
/// archive before extraction (#216).
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
) -> Result<DownloadReport, Failure> {
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
) -> Result<DownloadReport, Failure> {
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

    // The streaming hasher is only trustworthy when the file is written once
    // from byte 0. A pre-existing `.part` (resume) or any append invalidates
    // it and the caller rehashes the file from byte 0 instead (#216).
    let mut hasher = (existing == 0).then(Sha256::new);
    let mut resumed = existing > 0;
    let mut served_sha256: Option<String> = None;

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
        if (200..300).contains(&status) {
            if let Some(digest) = response.archive_sha256() {
                served_sha256 = Some(digest);
            }
        }

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
                // The file predates this run: the caller rehashes it from 0.
                return Ok(DownloadReport {
                    sha256: None,
                    bytes: existing,
                    served_sha256,
                    etag: response.etag(),
                    resumed: true,
                });
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
                hasher = Some(Sha256::new());
                resumed = false;
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
            hasher = Some(Sha256::new());
            resumed = false;
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
                    hasher = Some(Sha256::new());
                    resumed = false;
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

        if append {
            // Bytes already on disk were not hashed by this run (or were
            // hashed before an uncertain flush): a full rehash is required.
            hasher = None;
            resumed = true;
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
                    // #222: reserve the blocks now, so the filesystem refuses
                    // before 40 GB are on the wire instead of at the end.
                    if let Err(message) = preallocate_blocks(directory, needed) {
                        return Err(Failure::Failed(message));
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
                                Failure::Failed(download_write_failure(part, &e))
                            })?;
                            if let Some(hasher) = hasher.as_mut() {
                                hasher.update(&bytes);
                            }
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
            // the next attempt asks for exactly this offset. The hasher may
            // cover bytes past the flushed offset, so it is discarded and the
            // resumed file is rehashed from byte 0 before install (#216).
            hasher = None;
            resumed = true;
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

        return Ok(DownloadReport {
            sha256: hasher.map(|hasher| hex::encode(hasher.finalize())),
            bytes: done,
            served_sha256,
            etag: expected_etag,
            resumed,
        });
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
) -> Result<DownloadReport, Failure> {
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

/// True for a filesystem that ran out of space (#222). `ErrorKind::StorageFull`
/// is still unstable, so the OS error number and the message are used.
pub(crate) fn is_out_of_space(error: &std::io::Error) -> bool {
    // 28 = ENOSPC, 112 = ERROR_DISK_FULL, 39 = ERROR_HANDLE_DISK_FULL.
    if matches!(error.raw_os_error(), Some(28) | Some(112) | Some(39)) {
        return true;
    }
    let message = error.to_string().to_ascii_lowercase();
    message.contains("no space left") || message.contains("disk full")
}

/// Message for a failed write: a full volume names the volume and keeps the
/// checkpoint, everything else keeps the OS detail (#222).
pub(crate) fn download_write_failure(part: &Path, error: &std::io::Error) -> String {
    if is_out_of_space(error) {
        let volume = part
            .parent()
            .map(|parent| parent.display().to_string())
            .unwrap_or_else(|| part.display().to_string());
        format!(
            "The download volume {volume} is full. The partial archive was kept at {}; free space on that volume and resume the install.",
            part.display()
        )
    } else {
        format!(
            "Could not write the download to {}: {error}",
            part.display()
        )
    }
}

/// Message for a failed extraction (#222): a full volume names the volume, and
/// the archive is kept so the install can be resumed after freeing space.
pub(crate) fn extraction_failure(staging: &Path, error: String) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("no space left") || lower.contains("disk full") || lower.contains("enospc") {
        format!(
            "Extraction ran out of space in {}. The downloaded archive was kept; free space on that volume and resume the install. ({error})",
            staging.display()
        )
    } else {
        format!(
            "Could not extract the archive into {}: {error}",
            staging.display()
        )
    }
}

/// Reserve the blocks for the remaining bytes before any of them is written
/// (#222), so a nearly-full filesystem fails at once instead of at byte 40 GB.
///
/// The reservation is a **sibling probe file**: growing the real `.part` would
/// make a sparse file look complete to the resume offset, which is the file
/// length. `set_len` alone is sparse, so `fs2::allocate` asks the filesystem to
/// reserve real blocks; a filesystem that cannot answer (EOPNOTSUPP) is
/// accepted, a full one is reported.
pub(crate) fn preallocate_blocks(directory: &Path, bytes: u64) -> Result<(), String> {
    if bytes == 0 {
        return Ok(());
    }
    let probe = directory.join(format!(
        ".ludex-space-{}.probe",
        uuid::Uuid::new_v4().simple()
    ));
    let reserved = (|| -> std::io::Result<()> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)?;
        file.set_len(bytes)?;
        match fs2::FileExt::allocate(&file, bytes) {
            Ok(()) => Ok(()),
            // The filesystem cannot reserve blocks (FAT, some network shares):
            // the free-space check is all the protection there is.
            Err(error) if error.raw_os_error() == Some(95) => Ok(()),
            Err(error) => Err(error),
        }
    })();
    let _ = std::fs::remove_file(&probe);
    match reserved {
        Ok(()) => Ok(()),
        Err(error) if is_out_of_space(&error) => Err(format!(
            "Not enough free space in {} to reserve {} for the download. Free space or change the download folder.",
            directory.display(),
            human_bytes(bytes)
        )),
        Err(error) => {
            tracing::debug!(error = %error, path = %directory.display(), "could not reserve download blocks");
            Ok(())
        }
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
    fn an_adopted_staging_folder_wins_over_the_job_default() {
        let fallback = Path::new("/games/.hades.staging-new");
        let mut paths = JobPaths::default();
        assert_eq!(
            effective_staging(&paths, fallback),
            fallback,
            "a fresh job commits its own folder"
        );

        paths.staging = Some(Path::new("/games/.hades.staging-old").to_path_buf());
        assert_eq!(
            effective_staging(&paths, fallback),
            Path::new("/games/.hades.staging-old"),
            "the adopted checkpoint is the folder to commit (#225)"
        );

        // An empty path is not an adopted checkpoint: keep the default.
        paths.staging = Some(PathBuf::new());
        assert_eq!(effective_staging(&paths, fallback), fallback);
    }

    #[test]
    fn a_finished_install_removes_its_staging_and_stale_siblings() {
        let root = download_temp_dir("success-cleanup");
        let final_path = root.join("hades").join("1.0");
        std::fs::create_dir_all(&final_path).unwrap();
        std::fs::write(final_path.join("game.exe"), b"MZ").unwrap();

        let own = root.join(".hades.staging-aaaabbbb");
        let stale_legacy = root.join("hades-staging-ccccdddd");
        let stale_dot = root.join(".hades.staging-eeeeffff");
        let protected = root.join(".hades.staging-11112222");
        let other_game = root.join(".doom.staging-33334444");
        for (folder, bytes) in [
            (&own, 1024u64),
            (&stale_legacy, 2048),
            (&stale_dot, 4096),
            (&protected, 8192),
            (&other_game, 16384),
        ] {
            std::fs::create_dir_all(folder).unwrap();
            std::fs::write(folder.join("game.bin"), vec![0u8; bytes as usize]).unwrap();
        }

        let protected_set = HashSet::from([paths::path_key(&protected)]);
        let candidates = success_staging_candidates(&own, &final_path, "hades", &protected_set);

        let mut names: Vec<String> = candidates
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                ".hades.staging-aaaabbbb".to_string(),
                ".hades.staging-eeeeffff".to_string(),
                "hades-staging-ccccdddd".to_string(),
            ],
            "the protected folder and another game's staging are not candidates"
        );

        for path in &candidates {
            std::fs::remove_dir_all(path).unwrap();
        }
        assert!(!own.exists());
        assert!(!stale_legacy.exists());
        assert!(!stale_dot.exists());
        assert!(protected.join("game.bin").exists());
        assert!(other_game.join("game.bin").exists());
        assert!(final_path.join("game.exe").exists());

        // The final folder itself is never a candidate, even if passed as the
        // job's staging by a bug.
        assert!(
            success_staging_candidates(&final_path, &final_path, "hades", &HashSet::new())
                .is_empty()
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_a_resumable_or_active_job_protects_its_staging_folder() {
        // A failed stream job keeps its checkpoint: never sweep it.
        let mut record = persisted_record("job-protect", "error");
        assert!(staging_is_protected(&record));

        // A done job is never protected, even with a stale `resumable` flag.
        record.phase = "done".into();
        assert!(!staging_is_protected(&record));

        // A cancelled resumable job (completed entries kept) stays protected.
        record.phase = "cancelled".into();
        record.resumable = true;
        assert!(staging_is_protected(&record));
        record.resumable = false;
        assert!(!staging_is_protected(&record));

        // Every non-terminal phase is protected while the job runs.
        record.phase = "downloading".into();
        assert!(staging_is_protected(&record));
        record.phase = "waiting_for_space".into();
        assert!(staging_is_protected(&record));
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
            InstallPhase::WaitingForSpace,
            InstallPhase::Downloading,
            InstallPhase::Verifying,
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
        assert!(!InstallPhase::Verifying.is_terminal());
        assert!(!InstallPhase::WaitingForSpace.is_terminal());
        assert!(!InstallPhase::Paused.is_terminal());
        // The persisted names are part of the API (#223/#222): a rename would
        // silently break every row an older build wrote.
        assert_eq!(InstallPhase::Verifying.as_str(), "verifying");
        assert_eq!(InstallPhase::WaitingForSpace.as_str(), "waiting_for_space");
    }

    #[test]
    fn the_verifying_message_names_the_bytes_and_never_a_percent() {
        // #223: the row must say what is happening during the rehash, never
        // leave "Download already complete" as the only signal.
        let message = verifying_message(42 * 1024 * 1024 * 1024, 96 * 1024 * 1024 * 1024);
        assert!(message.starts_with("Verifying archive ("), "{message}");
        assert!(message.contains("42.0 GiB"), "{message}");
        assert!(message.contains("96.0 GiB"), "{message}");
        // An unknown size still says "Verifying archive".
        assert_eq!(verifying_message(0, 0), "Verifying archive");
    }

    #[test]
    fn a_full_volume_is_reported_with_the_volume_and_keeps_the_checkpoint() {
        let enospc = std::io::Error::from_raw_os_error(28);
        assert!(is_out_of_space(&enospc));
        assert!(is_out_of_space(&std::io::Error::other(
            "No space left on device"
        )));
        assert!(!is_out_of_space(&std::io::Error::from_raw_os_error(13)));

        let part = Path::new("/mnt/games/.7-9.part");
        let message = download_write_failure(part, &enospc);
        assert!(message.contains("/mnt/games"), "{message}");
        assert!(message.contains("full"), "{message}");
        assert!(message.contains("partial archive was kept"), "{message}");
        assert!(message.contains("/mnt/games/.7-9.part"), "{message}");

        // A non-space error keeps the OS detail instead of blaming the disk.
        let other = std::io::Error::from_raw_os_error(5);
        let message = download_write_failure(part, &other);
        assert!(
            message.contains("Could not write the download"),
            "{message}"
        );
    }

    #[test]
    fn an_extraction_out_of_space_names_the_install_volume() {
        let message = extraction_failure(
            Path::new("/mnt/games/.hades.staging-abc"),
            "7z error: No space left on device".to_string(),
        );
        assert!(
            message.contains("/mnt/games/.hades.staging-abc"),
            "{message}"
        );
        assert!(message.contains("downloaded archive was kept"), "{message}");
        let message =
            extraction_failure(Path::new("/mnt/games/staging"), "bad password".to_string());
        assert!(message.contains("Could not extract"), "{message}");
        assert!(!message.contains("kept"), "{message}");
    }

    #[test]
    fn the_download_blocks_are_reserved_before_the_first_byte() {
        let root = download_temp_dir("preallocate");
        // A tiny reservation succeeds and leaves no probe file behind.
        preallocate_blocks(&root, 64 * 1024).expect("reserve");
        let leftovers: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.contains("space-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "probe files left behind: {leftovers:?}"
        );
        // Zero bytes is a no-op, never an error.
        preallocate_blocks(&root, 0).expect("no-op");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_job_waiting_for_disk_space_keeps_its_queue_turn() {
        let mut jobs = vec![
            running("job-a", 0, "2026-01-01T00:00:00Z"),
            queued("job-b", 1, "2026-01-01T00:01:00Z"),
        ];
        // #222: a parked job is still startable, in its recorded position.
        jobs[1].phase = InstallPhase::WaitingForSpace;
        assert_eq!(next_queued(&jobs).as_deref(), Some("job-b"));
        // It is reorderable like a waiting job, never treated as running.
        assert!(jobs[1].is_waiting());
        assert!(!jobs[1].is_running());
        let assignments = plan_reorder(&jobs, "job-b", QueueMove::Top).unwrap();
        assert_eq!(
            assignments,
            vec![("job-a".to_string(), 0), ("job-b".to_string(), 1)]
        );
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
            verified_sha256: None,
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

    /* --------------------- archive verification (#216) --------------------- */

    #[tokio::test]
    async fn a_fresh_download_reports_a_streaming_digest() {
        let payload: Vec<u8> = (0..200 * 1024).map(|index| (index % 251) as u8).collect();
        let digest = hex::encode(Sha256::digest(&payload));
        let (mut transport, _ranges) = FakeTransport::new(vec![FakeConn {
            status: 200,
            declared_length: Some(payload.len() as u64),
            range_total: None,
            etag: None,
            sha256: Some(digest.clone()),
            body: payload.clone(),
            after: After::End,
            chunk_delay: Duration::ZERO,
        }]);
        let root = download_temp_dir("verify-fresh");
        let part = root.join("archive.part");

        let report = download_into(
            &mut transport,
            &part,
            "Verified game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(50, 3),
            |_| {},
        )
        .await
        .expect("download");

        assert!(!report.resumed);
        assert_eq!(report.sha256.as_deref(), Some(digest.as_str()));
        assert_eq!(report.served_sha256.as_deref(), Some(digest.as_str()));
        assert_eq!(report.bytes, payload.len() as u64);

        // The streaming digest is trusted: verifying must not need a re-read.
        let missing = root.join("already-moved.part");
        let verdict = crate::archive_verify::verify_download(
            &missing,
            Some(&digest),
            report.sha256.as_deref(),
            report.served_sha256.as_deref(),
        )
        .expect("verify");
        assert!(
            matches!(verdict, crate::archive_verify::ArchiveVerdict::Verified(_)),
            "{verdict:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_resumed_download_is_rehashed_from_byte_zero() {
        let payload: Vec<u8> = (0..300 * 1024).map(|index| (index % 243) as u8).collect();
        let split = 100 * 1024;
        let digest = hex::encode(Sha256::digest(&payload));
        let root = download_temp_dir("verify-resume");
        let part = root.join("archive.part");
        std::fs::write(&part, &payload[..split]).expect("seed partial");
        let (mut transport, ranges) = FakeTransport::new(vec![FakeConn {
            status: 206,
            declared_length: Some((payload.len() - split) as u64),
            range_total: Some(payload.len() as u64),
            etag: Some("\"v1\"".to_string()),
            sha256: Some(digest.clone()),
            body: payload[split..].to_vec(),
            after: After::End,
            chunk_delay: Duration::ZERO,
        }]);

        let report = download_into(
            &mut transport,
            &part,
            "Resumed game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(50, 3),
            |_| {},
        )
        .await
        .expect("resume");

        assert_eq!(*ranges.lock().unwrap(), vec![Some(split as u64)]);
        assert!(report.resumed);
        assert!(
            report.sha256.is_none(),
            "a resumed writer cannot produce a from-zero digest"
        );

        // Hashing the file from byte 0 (not only the new bytes) verifies it.
        let verdict = crate::archive_verify::verify_download(
            &part,
            Some(&digest),
            report.sha256.as_deref(),
            report.served_sha256.as_deref(),
        )
        .expect("verify");
        assert_eq!(
            verdict,
            crate::archive_verify::ArchiveVerdict::Verified(digest)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_corrupted_body_never_reaches_the_install_gate() {
        let honest: Vec<u8> = (0..120 * 1024).map(|index| (index % 239) as u8).collect();
        let expected = hex::encode(Sha256::digest(&honest));
        // The server advertises the honest digest, the body is damaged in
        // transit: a transport corruption, not a replaced archive.
        let mut corrupt = honest.clone();
        corrupt[64 * 1024] ^= 0x7F;
        let (mut transport, _ranges) = FakeTransport::new(vec![FakeConn {
            status: 200,
            declared_length: Some(corrupt.len() as u64),
            range_total: None,
            etag: None,
            sha256: Some(expected.clone()),
            body: corrupt,
            after: After::End,
            chunk_delay: Duration::ZERO,
        }]);
        let root = download_temp_dir("verify-corrupt");
        let part = root.join("archive.part");

        let report = download_into(
            &mut transport,
            &part,
            "Corrupt game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(50, 3),
            |_| {},
        )
        .await
        .expect("download");

        let verdict = crate::archive_verify::verify_download(
            &part,
            Some(&expected),
            report.sha256.as_deref(),
            report.served_sha256.as_deref(),
        )
        .expect("verify");
        let crate::archive_verify::ArchiveVerdict::Mismatch(mismatch) = verdict else {
            panic!("a corrupted body must be rejected: {verdict:?}");
        };
        assert!(!mismatch.changed_on_server);
        let message = mismatch.message("Corrupt game");
        assert!(message.contains(&expected), "{message}");
        assert!(message.contains(&mismatch.actual), "{message}");
        assert!(message.contains("Nothing was installed"), "{message}");
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
        /// Full archive digest the fake server advertises (#216).
        sha256: Option<String>,
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

        fn archive_sha256(&self) -> Option<String> {
            self.conn.sha256.clone()
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
                sha256: None,
                body: payload[..split].to_vec(),
                after: After::Stall,
                chunk_delay: Duration::ZERO,
            },
            FakeConn {
                status: 206,
                declared_length: Some((payload.len() - split) as u64),
                range_total: Some(payload.len() as u64),
                etag: Some("\"v1\"".to_string()),
                sha256: None,
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
            sha256: None,
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
                sha256: None,
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
                sha256: None,
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
                sha256: None,
                body: Vec::new(),
                after: After::End,
                chunk_delay: Duration::ZERO,
            },
            FakeConn {
                status: 200,
                declared_length: Some(second.len() as u64),
                range_total: None,
                etag: Some("\"v2\"".to_string()),
                sha256: None,
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
            sha256: None,
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
                sha256: None,
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
            sha256: None,
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
            sha256: None,
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
            sha256: None,
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
