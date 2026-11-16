//! Pure planning for the install queue and the on-disk layout.
//!
//! Every function here answers a question without performing I/O: which job
//! runs next, where a job's `.part`/staging folder live, and which staging
//! folders a live checkpoint still owns. The scheduler feeds them snapshots and
//! applies the result; the tests exercise them without a Tauri runtime, which
//! is why a queue-ordering regression is caught before it can park a download.
//!
//! The admission decision itself stays in [`crate::pipeline::plan_next`] — this
//! module only builds the candidates and the paths that decision needs.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use crate::db;
use crate::paths;
use crate::pipeline::Candidate;
use crate::settings::LocalSettings;
use crate::space::SpacePlan;
use crate::state::{AppState, JobEntry};
use crate::util::short_id;
use tauri::{AppHandle, Manager};

use super::job::{InstallParams, InstallPhase, JobPaths, QueueJob, QueueMove};
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

pub(super) fn candidates(state: &AppState) -> Vec<Candidate> {
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
pub(super) fn plan_for_job(state: &AppState, entry: &JobEntry) -> SpacePlan {
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
        .unwrap_or_else(|| settings.effective_download_dir(&install_root));
    // A file-manifest attempt writes the extracted tree only: no archive lands
    // on this volume, so charging one would park a job that would have fit
    // (#231). Only a job that already proved a manifest is planned that way;
    // an `automatic` job (#235) is planned from the transport it will fall back
    // to, and `manifest_install` replaces the estimate with the exact extracted
    // peak before the first byte.
    let wants_files = job.file_manifest;
    SpacePlan::build(
        job.bytes_total.or(entry.params.expected_bytes),
        None,
        &install_root,
        &download_root,
        job.stream || wants_files,
        settings.keep_archives,
    )
}

pub(super) fn queue_snapshot(state: &AppState) -> Vec<QueueJob> {
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

pub(super) fn planned_paths(state: &AppState, entry: &JobEntry) -> JobPaths {
    let settings = state.settings_snapshot();
    let (stored_part, _) = entry.stored_paths();
    planned_paths_for(
        &settings,
        &entry.params,
        stored_part.as_deref(),
        entry.staging_path().as_deref(),
        &entry.snapshot().job_id,
    )
}

/// Pure form of [`planned_paths`], testable without a Tauri runtime.
pub(super) fn planned_paths_for(
    settings: &LocalSettings,
    params: &InstallParams,
    stored_part: Option<&str>,
    stored_staging: Option<&str>,
    job_id: &str,
) -> JobPaths {
    let install_root = params
        .install_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| settings.install_path());
    let slug = paths::slug_for_title(&params.title);
    let part = resolve_part_path(settings, params, &install_root, stored_part);
    let staging = stored_staging
        .map(PathBuf::from)
        .unwrap_or_else(|| install_root.join(format!(".{slug}.staging-{}", short_id(job_id))));
    JobPaths {
        staging: Some(staging),
        part: Some(part),
    }
}

/// Path of the temporary archive for a job (#226).
///
/// A stored checkpoint always wins and is used exactly as recorded: flipping
/// `downloadStaging` must never relocate a resumable `.part`. Otherwise the
/// job's pinned download folder is used, then the staging policy.
pub(super) fn resolve_part_path(
    settings: &LocalSettings,
    params: &InstallParams,
    install_root: &Path,
    stored_part: Option<&str>,
) -> PathBuf {
    if let Some(stored) = stored_part.map(str::trim).filter(|value| !value.is_empty()) {
        return PathBuf::from(stored);
    }
    let download_root = params
        .download_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| settings.effective_download_dir(install_root));
    download_root.join(format!(".{}-{}.part", params.game_id, params.version_id))
}

pub(super) fn staging_is_protected(record: &db::InstallJobRecord) -> bool {
    let phase = InstallPhase::parse(&record.phase);
    phase != InstallPhase::Done && (record.resumable || !phase.is_terminal())
}

/// Staging folders a live or resumable job still owns (#225).
///
/// Built from the in-memory queue and from SQLite: a job that was interrupted
/// in a previous app run has no in-memory entry, and deleting its checkpoint
/// would force a full re-download.
pub(super) fn protected_staging(app: &AppHandle) -> HashSet<PathBuf> {
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

pub(super) fn success_staging_candidates(
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

pub(super) fn effective_staging(job_paths: &JobPaths, fallback: &Path) -> PathBuf {
    job_paths
        .staging
        .clone()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| fallback.to_path_buf())
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
pub(super) fn free_space(path: &Path) -> Option<u64> {
    let existing = paths::nearest_existing(path)?;
    fs2::available_space(existing).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::paths;
    use crate::settings;

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

    fn download_temp_dir(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ludex-download-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn install_params(download_dir: Option<&str>) -> InstallParams {
        InstallParams {
            game_id: 3,
            version_id: 4,
            server_url: "https://ludex.example.com".into(),
            token: None,
            title: "Elden Ring".into(),
            version: Some("1.10".into()),
            executable: None,
            install_dir: None,
            download_dir: download_dir.map(str::to_string),
            install_location_id: None,
            download_location_id: None,
            expected_bytes: None,
            stream: Some(false),
            replace_path: None,
        }
    }

    #[test]
    fn planned_paths_follow_the_download_staging_policy() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.install_dir = "/games".into();
        settings.download_dir = "/mnt/nas/Downloads".into();
        settings.normalize(Path::new("/data"));

        // #235: the default stages under the install root so a NAS download
        // folder is never written, hashed and extracted over SMB.
        let default = planned_paths_for(&settings, &install_params(None), None, None, "job-1");
        assert_eq!(
            default.part,
            Some(PathBuf::from("/games/.ludex-tmp/.3-4.part"))
        );

        // `configured` uses the download folder when the user asked for it.
        settings.download_staging = settings::DownloadStaging::Configured;
        let configured = planned_paths_for(&settings, &install_params(None), None, None, "job-1");
        assert_eq!(
            configured.part,
            Some(PathBuf::from("/mnt/nas/Downloads/.3-4.part"))
        );

        // #226: `installVolume` stages under the install root.
        settings.download_staging = settings::DownloadStaging::InstallVolume;
        let staged = planned_paths_for(&settings, &install_params(None), None, None, "job-1");
        assert_eq!(
            staged.part,
            Some(PathBuf::from("/games/.ludex-tmp/.3-4.part"))
        );
        // `begin` pins the resolved folder on the job: same answer.
        let pinned = planned_paths_for(
            &settings,
            &install_params(Some("/games/.ludex-tmp")),
            None,
            None,
            "job-1",
        );
        assert_eq!(pinned.part, staged.part);

        // A stored `.part` wins over the policy: never relocate a checkpoint,
        // including after flipping the setting back.
        for staging in [
            settings::DownloadStaging::InstallVolume,
            settings::DownloadStaging::Configured,
        ] {
            settings.download_staging = staging;
            let resumed = planned_paths_for(
                &settings,
                &install_params(None),
                Some("/mnt/nas/Downloads/.3-4.part"),
                None,
                "job-1",
            );
            assert_eq!(
                resumed.part,
                Some(PathBuf::from("/mnt/nas/Downloads/.3-4.part"))
            );
        }

        // An explicit per-job download folder still wins.
        let explicit = planned_paths_for(
            &settings,
            &install_params(Some("/mnt/fast")),
            None,
            None,
            "job-1",
        );
        assert_eq!(explicit.part, Some(PathBuf::from("/mnt/fast/.3-4.part")));
    }
}
