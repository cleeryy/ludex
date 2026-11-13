//! Download/extract pipeline admission (#221).
//!
//! An install used to hold one slot from queue to done, so a 20 GB extraction
//! left the network idle. The scheduler now has two resources: a **download**
//! slot (1 by default, 2 when the expert setting allows it) and a single
//! **extraction/commit** slot. A job downloads, gives the network slot back,
//! then takes the extractor. The next queued job therefore downloads while the
//! previous one unpacks: that is the *prefetch*, and it is flagged on the job
//! so Downloads can say "Downloading ahead" instead of pretending the install
//! already extracts.
//!
//! The decision itself is pure so it can be tested with an injected capacity
//! provider and scheduler state: [`plan_next`] picks the head of the queue,
//! reserves its peak disk requirement (#222) or parks it as
//! `waitingForSpace`, and never returns a job a worker already owns.

use crate::install::InstallPhase;
use crate::space::{Capacity, SpaceLedger, SpacePlan, SpaceReservation, SpaceStatus};

/// One job the scheduler may start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub job_id: String,
    /// Queue slot (#202); `None` sorts last like an older row.
    pub position: Option<u32>,
    pub created_at: String,
    pub phase: InstallPhase,
    /// Worst-case disk requirement of the job. `None` for a job a worker
    /// already owns: it is not a candidate any more.
    pub space: Option<SpacePlan>,
}

impl Candidate {
    /// Queued and space-blocked jobs are startable; a paused job waits for an
    /// explicit resume and a running one owns its worker.
    pub fn is_startable(&self) -> bool {
        matches!(
            self.phase,
            InstallPhase::Queued | InstallPhase::WaitingForSpace
        )
    }

    fn order_key(&self) -> (u32, &str, &str) {
        (
            self.position.unwrap_or(u32::MAX),
            self.created_at.as_str(),
            self.job_id.as_str(),
        )
    }
}

/// What the scheduler should do next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Nothing to start (empty queue, paused jobs only, no free slot).
    Idle,
    /// Start the download of `job_id`. `prefetching` is true when the
    /// extractor is busy, i.e. this archive is fetched ahead of its turn.
    Start {
        job_id: String,
        prefetching: bool,
        space: SpaceStatus,
    },
    /// The head of the queue cannot fit yet: keep it in the queue with a
    /// readable reason instead of starting and failing mid-download.
    WaitingForSpace { job_id: String, space: SpaceStatus },
}

/// Decide the next install to start.
///
/// Only the lowest queue position is considered: with one download slot the
/// queue order is the fairness rule, and a job that does not fit must not be
/// leapfrogged by a smaller one (that would silently reorder the queue). The
/// caller re-runs this when a job finishes, pauses, is cancelled or is moved.
pub fn plan_next(
    candidates: &[Candidate],
    download_slot_free: bool,
    extracting: bool,
    ledger: &SpaceLedger,
    capacity: &dyn Capacity,
) -> Admission {
    if !download_slot_free {
        return Admission::Idle;
    }
    let Some(candidate) = candidates
        .iter()
        .filter(|candidate| candidate.is_startable() && candidate.space.is_some())
        .min_by(|left, right| left.order_key().cmp(&right.order_key()))
    else {
        return Admission::Idle;
    };

    let plan = candidate.space.clone().unwrap_or_default();
    match ledger.try_reserve(&candidate.job_id, &plan, capacity) {
        SpaceReservation::Granted(space) => Admission::Start {
            job_id: candidate.job_id.clone(),
            prefetching: extracting,
            space,
        },
        SpaceReservation::Waiting(space) => Admission::WaitingForSpace {
            job_id: candidate.job_id.clone(),
            space,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::{SpaceRequirement, Volume, VolumeId, SAFETY_MARGIN_FLOOR_BYTES};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    const GIB: u64 = 1024 * 1024 * 1024;

    struct FakeCapacity(HashMap<&'static str, u64>);

    impl FakeCapacity {
        fn new(free: u64) -> Self {
            Self(HashMap::from([("/mnt/games", free)]))
        }
    }

    impl Capacity for FakeCapacity {
        fn free_bytes(&self, path: &Path) -> Option<u64> {
            self.0.get(path.to_str()?).copied()
        }
    }

    fn plan(bytes: u64) -> SpacePlan {
        SpacePlan {
            archive_bytes: Some(bytes),
            extracted_bytes: None,
            extraction_estimated: false,
            requirements: vec![SpaceRequirement {
                volume: Volume {
                    id: VolumeId("test:/mnt/games".to_string()),
                    path: PathBuf::from("/mnt/games"),
                },
                raw_bytes: bytes,
                bytes: bytes + bytes / 20 + SAFETY_MARGIN_FLOOR_BYTES,
                label: "archive + extracted files",
            }],
        }
    }

    fn candidate(job_id: &str, position: u32, phase: InstallPhase, bytes: u64) -> Candidate {
        Candidate {
            job_id: job_id.to_string(),
            position: Some(position),
            created_at: format!("2026-01-01T00:00:{position:02}Z"),
            phase,
            space: Some(plan(bytes)),
        }
    }

    #[test]
    fn the_head_of_the_queue_downloads_while_the_previous_job_extracts() {
        let ledger = SpaceLedger::new();
        let capacity = FakeCapacity::new(500 * GIB);
        let jobs = vec![
            // A owns the extractor: it is not a candidate any more.
            Candidate {
                space: None,
                ..candidate("job-a", 0, InstallPhase::Extracting, 40 * GIB)
            },
            candidate("job-b", 1, InstallPhase::Queued, 40 * GIB),
        ];

        let admission = plan_next(&jobs, true, true, &ledger, &capacity);
        let Admission::Start {
            job_id,
            prefetching,
            ..
        } = admission
        else {
            panic!("expected a prefetch start: {admission:?}");
        };
        assert_eq!(job_id, "job-b");
        assert!(
            prefetching,
            "a download that starts while another job extracts is a prefetch"
        );
        assert_eq!(
            ledger.job_bytes("job-b"),
            plan(40 * GIB).requirements[0].bytes
        );
    }

    #[test]
    fn a_free_extractor_means_no_prefetch_flag() {
        let ledger = SpaceLedger::new();
        let capacity = FakeCapacity::new(500 * GIB);
        let jobs = vec![candidate("job-a", 0, InstallPhase::Queued, 10 * GIB)];
        let admission = plan_next(&jobs, true, false, &ledger, &capacity);
        assert!(matches!(
            admission,
            Admission::Start {
                prefetching: false,
                ..
            }
        ));
    }

    #[test]
    fn without_a_free_download_slot_nothing_starts() {
        let ledger = SpaceLedger::new();
        let capacity = FakeCapacity::new(500 * GIB);
        let jobs = vec![candidate("job-a", 0, InstallPhase::Queued, 10 * GIB)];
        assert_eq!(
            plan_next(&jobs, false, false, &ledger, &capacity),
            Admission::Idle
        );
        assert_eq!(ledger.job_bytes("job-a"), 0);
    }

    #[test]
    fn a_job_that_cannot_fit_waits_instead_of_prefetching() {
        let ledger = SpaceLedger::new();
        let capacity = FakeCapacity::new(30 * GIB);
        let jobs = vec![candidate("job-a", 0, InstallPhase::Queued, 40 * GIB)];

        let admission = plan_next(&jobs, true, true, &ledger, &capacity);
        let Admission::WaitingForSpace { job_id, space } = admission else {
            panic!("expected a space wait: {admission:?}");
        };
        assert_eq!(job_id, "job-a");
        assert!(!space.ok);
        assert_eq!(space.volume, "/mnt/games");
        // A refused prefetch must not hold a reservation.
        assert_eq!(ledger.job_bytes("job-a"), 0);
    }

    #[test]
    fn a_space_blocked_job_is_re_evaluated_in_place() {
        let ledger = SpaceLedger::new();
        let capacity = FakeCapacity::new(500 * GIB);
        let jobs = vec![candidate(
            "job-a",
            3,
            InstallPhase::WaitingForSpace,
            10 * GIB,
        )];
        // Keeping its queue position is what lets it start ahead of the jobs
        // that arrived after it while it waited.
        let admission = plan_next(&jobs, true, false, &ledger, &capacity);
        assert!(matches!(admission, Admission::Start { ref job_id, .. } if job_id == "job-a"));
    }

    #[test]
    fn the_queue_order_wins_over_size() {
        let ledger = SpaceLedger::new();
        let capacity = FakeCapacity::new(500 * GIB);
        let jobs = vec![
            candidate("job-big", 1, InstallPhase::Queued, 40 * GIB),
            candidate("job-small", 2, InstallPhase::Queued, GIB),
        ];
        let admission = plan_next(&jobs, true, true, &ledger, &capacity);
        assert!(
            matches!(admission, Admission::Start { ref job_id, .. } if job_id == "job-big"),
            "the waiting queue is not leapfrogged: {admission:?}"
        );
    }

    #[test]
    fn a_paused_or_running_job_never_starts_again() {
        let ledger = SpaceLedger::new();
        let capacity = FakeCapacity::new(500 * GIB);
        let jobs = vec![
            candidate("job-paused", 0, InstallPhase::Paused, 10 * GIB),
            Candidate {
                space: None,
                ..candidate("job-running", 1, InstallPhase::Downloading, 10 * GIB)
            },
            candidate("job-next", 2, InstallPhase::Queued, 10 * GIB),
        ];
        let admission = plan_next(&jobs, true, false, &ledger, &capacity);
        assert!(
            matches!(admission, Admission::Start { ref job_id, .. } if job_id == "job-next"),
            "only the queued job starts: {admission:?}"
        );
        assert_eq!(ledger.job_bytes("job-paused"), 0);
        assert_eq!(ledger.job_bytes("job-running"), 0);
    }

    #[test]
    fn reordering_the_queue_re_targets_the_prefetch() {
        let ledger = SpaceLedger::new();
        let capacity = FakeCapacity::new(500 * GIB);
        let mut jobs = vec![
            candidate("job-b", 1, InstallPhase::Queued, 10 * GIB),
            candidate("job-c", 2, InstallPhase::Queued, 10 * GIB),
        ];
        assert!(
            matches!(plan_next(&jobs, true, true, &ledger, &capacity), Admission::Start { ref job_id, .. } if job_id == "job-b")
        );
        ledger.release("job-b");
        // Moving `job-c` to the top is what makes it the next archive fetched
        // while the current install keeps extracting.
        jobs[1].position = Some(0);
        jobs[0].position = Some(1);
        assert!(
            matches!(plan_next(&jobs, true, true, &ledger, &capacity), Admission::Start { ref job_id, .. } if job_id == "job-c"),
            "the reordered job is prefetched next"
        );
        // The paused job that used to own the prefetch never starts by itself.
        jobs[1].phase = InstallPhase::Paused;
        assert!(matches!(
            plan_next(&jobs, true, true, &ledger, &capacity),
            Admission::Start { ref job_id, .. } if job_id == "job-b"
        ));
    }

    #[test]
    fn one_archive_per_version_is_admitted_once() {
        // The registry deduplicates by game/version before it builds candidates
        // (`begin`/`resume_entry`), so the scheduler can only see one entry per
        // version. Starting it once must reserve exactly one requirement.
        let ledger = SpaceLedger::new();
        let capacity = FakeCapacity::new(500 * GIB);
        let jobs = vec![candidate("job-a", 0, InstallPhase::Queued, 10 * GIB)];
        assert!(matches!(
            plan_next(&jobs, true, false, &ledger, &capacity),
            Admission::Start { .. }
        ));
        // A second pass without the job having started (same candidate) must
        // not double the reservation.
        let bytes = plan(10 * GIB).requirements[0].bytes;
        assert!(matches!(
            plan_next(&jobs, true, false, &ledger, &capacity),
            Admission::Start { .. }
        ));
        assert_eq!(ledger.job_bytes("job-a"), bytes);
    }
}
