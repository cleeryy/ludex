//! Value types of the install pipeline.
//!
//! Everything here is plain data: the JSON shape the UI receives
//! ([`InstallJob`]), the request a user clicked ([`InstallParams`]), the queue
//! ordering types ([`QueueJob`], [`QueueMove`]), and the small failure enum the
//! stages return. Keeping them in one module is what lets the scheduler, the
//! transfer and the commit steps share a vocabulary without importing each
//! other: none of these types performs I/O.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::db;
use crate::net::cancel::Stopped;
use crate::space;
pub(crate) const INTERRUPTED_MESSAGE: &str =
    "Ludex closed before this install finished. Resuming automatically.";
/// The message v0.4.5 and earlier stamped on an interrupted row. Recognised by
/// the startup auto-resume so an upgrade does not strand an in-flight job.
pub(crate) const LEGACY_INTERRUPTED_MESSAGE: &str =
    "Ludex closed before this install finished. Install again to resume.";

/// Message of a job the startup circuit breaker refused to resume (#234).
///
/// Deliberately not one of the `INTERRUPTED_*` messages:
/// `interrupted_install_jobs` must never pick a parked row up again.
pub(crate) const RESUME_DISABLED_MESSAGE: &str =
    "Resume disabled after repeated crashes — keep the partial file and retry manually";

/// How long the app must stay up before the startup markers are cleared (#234).
///
/// Long enough that a worker panicking right after the resume loop has already
/// failed its job (contained) or crashed the process (marker kept), short
/// enough that a healthy launch does not leave a marker behind.
pub(crate) const RESUMED_HEALTHY_AFTER: Duration = Duration::from_secs(15);

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
    /// True while this job uses the server's file manifest (#231): files are
    /// downloaded directly, verified one by one, and no archive is ever written
    /// on this device.
    pub file_manifest: bool,
    /// True once a file-manifest attempt fell back to the classic archive
    /// (#231). `message` carries the reason the row shows.
    pub manifest_fallback: bool,
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
            // The transport is a property of the running attempt, not of the
            // history row: an interrupted file-manifest job shows the classic
            // labels until its worker adopts the manifest checkpoint again.
            file_manifest: false,
            manifest_fallback: false,
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
    pub(super) fn is_waiting(&self) -> bool {
        matches!(
            self.phase,
            InstallPhase::Queued | InstallPhase::WaitingForSpace | InstallPhase::Paused
        )
    }

    pub(super) fn is_running(&self) -> bool {
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
    pub(super) fn order_key(&self) -> (u8, u32, &str, &str) {
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
    pub(super) fn to_record(&self, job_id: &str, created_at: &str) -> db::InstallJobParamsRecord {
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
    pub(super) fn from_record(record: db::InstallJobParamsRecord, token: Option<String>) -> Self {
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

    pub(super) fn stream_flag(self, current: bool) -> bool {
        match self {
            ResumeMode::Keep => current,
            ResumeMode::Download => false,
            ResumeMode::Stream => true,
        }
    }
}

/// Who asked for a resume (#234).
///
/// Only a human clears the automatic-resume failure streak: the startup path
/// must keep counting consecutive unclean launches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResumeOrigin {
    Startup,
    Manual,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct InstallErrorEvent {
    pub(super) job_id: String,
    pub(super) message: String,
}

#[derive(Debug)]
pub(crate) enum Failure {
    Cancelled,
    /// The user paused the job: the `.part` file is kept for a Range resume.
    Paused,
    Failed(String),
    /// The destination library's mount disappeared (#290): the job pauses with
    /// this message, keeps its partial archive and resumes when the drive is
    /// back.
    Offline(String),
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

impl From<Stopped> for Failure {
    fn from(stop: Stopped) -> Self {
        match stop {
            Stopped::Cancelled => Failure::Cancelled,
            Stopped::Paused => Failure::Paused,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
