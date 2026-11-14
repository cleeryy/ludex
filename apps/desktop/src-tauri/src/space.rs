//! Per-volume disk-space reservations for installs (#222).
//!
//! A preflight against the free space alone is not enough: two 40 GB jobs can
//! both pass it against the same bytes and then fail during extraction. This
//! module keeps a ledger keyed by **volume** (the device, not the path string,
//! so an install folder and a download folder on the same disk share it) and by
//! job id, so the next job only starts when
//! `free - other reservations >= its peak requirement`.
//!
//! The peak requirement is the worst case of one install: the archive plus the
//! extracted tree on one volume, or one requirement per volume when the archive
//! and the install live on different disks. An unknown extracted size is
//! estimated from the archive (`UNKNOWN_EXTRACTION_PERCENT`) and flagged, and
//! every requirement carries a safety margin (5 % + 2 GiB) so the filesystem is
//! never asked for the last byte.
//!
//! The ledger is in memory and rebuilt from the active jobs at startup, so a
//! restart cannot double-count a reservation (#222).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::util::human_bytes;

/// Headroom added to every peak estimate: 5 % of the bytes plus a flat floor.
pub const SAFETY_MARGIN_PERCENT: u64 = 5;
pub const SAFETY_MARGIN_FLOOR_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Extracted-tree estimate when the archive size is known but the unpacked
/// size is not: archive × 1.5, flagged as an estimate in the UI.
pub const UNKNOWN_EXTRACTION_PERCENT: u64 = 150;

/// Adds the safety margin to one volume's peak requirement.
pub fn with_margin(bytes: u64) -> u64 {
    if bytes == 0 {
        return 0;
    }
    bytes
        .saturating_add(bytes.saturating_mul(SAFETY_MARGIN_PERCENT) / 100)
        .saturating_add(SAFETY_MARGIN_FLOOR_BYTES)
}

/// Identity of a volume: the device id on Unix, the volume prefix on Windows.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct VolumeId(pub String);

/// A volume plus the folder the user pointed at, which is what messages show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    pub id: VolumeId,
    pub path: PathBuf,
}

#[cfg(unix)]
fn volume_key(path: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let existing = crate::paths::nearest_existing(path)?;
    let metadata = std::fs::metadata(&existing).ok()?;
    Some(format!("dev:{}", metadata.dev()))
}

#[cfg(windows)]
fn volume_key(path: &Path) -> Option<String> {
    use std::path::Component;
    let existing = crate::paths::nearest_existing(path)?;
    let prefix = existing
        .components()
        .find_map(|component| match component {
            Component::Prefix(prefix) => Some(prefix.as_os_str().to_string_lossy().to_lowercase()),
            _ => None,
        })?;
    Some(format!("volume:{prefix}"))
}

#[cfg(not(any(unix, windows)))]
fn volume_key(_path: &Path) -> Option<String> {
    None
}

/// The volume holding `path` (the nearest existing folder is used when the
/// install/download folder does not exist yet).
pub fn volume_for(path: &Path) -> Option<Volume> {
    let key = volume_key(path)?;
    Some(Volume {
        id: VolumeId(key),
        path: crate::paths::nearest_existing(path).unwrap_or_else(|| path.to_path_buf()),
    })
}

/// Free-space probe, injectable so the ledger is testable without real disks.
pub trait Capacity {
    fn free_bytes(&self, path: &Path) -> Option<u64>;
}

/// The real filesystem (`fs2::available_space` on the nearest existing folder).
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemCapacity;

impl Capacity for SystemCapacity {
    fn free_bytes(&self, path: &Path) -> Option<u64> {
        let existing = crate::paths::nearest_existing(path)?;
        fs2::available_space(existing).ok()
    }
}

/// Worst-case bytes one job needs on one volume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpaceRequirement {
    pub volume: Volume,
    /// Peak bytes including the safety margin.
    pub bytes: u64,
    /// Peak bytes before the margin.
    pub raw_bytes: u64,
    /// What the bytes are, for the message: "archive + extracted files".
    pub label: &'static str,
}

/// The volumes one install will write to, with its worst-case peak usage.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpacePlan {
    /// Archive size on the server, when the UI or a response told us.
    pub archive_bytes: Option<u64>,
    /// Extracted-tree size, measured or estimated.
    pub extracted_bytes: Option<u64>,
    /// True when `extracted_bytes` is archive × 1.5, not a measured value.
    pub extraction_estimated: bool,
    pub requirements: Vec<SpaceRequirement>,
}

impl SpacePlan {
    /// Nothing to reserve: no size known, or the volumes cannot be probed.
    pub fn is_empty(&self) -> bool {
        self.requirements.is_empty()
    }

    /// `archive + extracted` when both live on one volume, otherwise one
    /// requirement per volume. Streamed installs only write the extracted tree
    /// (no archive is kept), apart from the staging copy.
    ///
    /// `keep_archives` changes the steady state, not the peak: the archive is
    /// counted while it is being downloaded in every mode.
    pub fn build(
        archive_bytes: Option<u64>,
        extracted_bytes: Option<u64>,
        install_path: &Path,
        download_path: &Path,
        stream: bool,
        keep_archives: bool,
    ) -> Self {
        // `keep_archives` changes the steady state, not the peak: the archive
        // exists on disk while it is downloaded in every mode, and a kept
        // archive stays on the download volume, which is already counted.
        let _ = keep_archives;
        let archive = archive_bytes.filter(|bytes| *bytes > 0);
        let mut extraction_estimated = false;
        let extracted = match (extracted_bytes.filter(|bytes| *bytes > 0), archive) {
            (Some(bytes), _) => Some(bytes),
            (None, Some(archive)) => {
                extraction_estimated = true;
                Some(archive.saturating_mul(UNKNOWN_EXTRACTION_PERCENT) / 100)
            }
            (None, None) => None,
        };

        let mut requirements: Vec<SpaceRequirement> = Vec::new();
        let mut push = |volume: Volume, raw_bytes: u64, label: &'static str| {
            if raw_bytes == 0 {
                return;
            }
            requirements.push(SpaceRequirement {
                volume,
                raw_bytes,
                bytes: with_margin(raw_bytes),
                label,
            });
        };

        let install_volume = volume_for(install_path);
        let download_volume = volume_for(download_path);
        let same_volume = match (&install_volume, &download_volume) {
            (Some(install), Some(download)) => install.id == download.id,
            // An unprobeable volume is treated as shared: over-reserving on the
            // one volume we do know is safer than under-reserving.
            _ => true,
        };

        if stream {
            if let (Some(volume), Some(extracted)) = (install_volume, extracted) {
                push(volume, extracted, "extracted files");
            }
        } else if same_volume {
            if let Some(volume) = install_volume {
                let raw = archive.unwrap_or(0).saturating_add(extracted.unwrap_or(0));
                push(volume, raw, "archive + extracted files");
            }
        } else {
            if let (Some(volume), Some(archive)) = (download_volume, archive) {
                push(volume, archive, "archive");
            }
            if let (Some(volume), Some(extracted)) = (install_volume, extracted) {
                push(volume, extracted, "extracted files");
            }
        }

        Self {
            archive_bytes: archive,
            extracted_bytes: extracted,
            extraction_estimated,
            requirements,
        }
    }
}

/// One job's requirement on one volume, as the job and the UI see it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceStatus {
    /// Folder on the volume, for the message.
    pub volume: String,
    pub needed_bytes: u64,
    pub free_bytes: u64,
    /// Bytes held by the other active jobs on the same volume.
    pub reserved_bytes: u64,
    /// `free - reserved`, never negative.
    pub available_bytes: u64,
    pub ok: bool,
}

impl SpaceStatus {
    /// Granted but nothing known (unknown size or unprobeable volume).
    fn unknown() -> Self {
        Self {
            volume: String::new(),
            needed_bytes: 0,
            free_bytes: 0,
            reserved_bytes: 0,
            available_bytes: 0,
            ok: true,
        }
    }

    /// Message shown on the job while it waits for disk space.
    pub fn message(&self) -> String {
        if self.ok {
            format!(
                "Reserved {} on {}",
                human_bytes(self.needed_bytes),
                self.volume
            )
        } else {
            format!(
                "Waiting for disk space on {}: {} needed, {} available after other installs ({} reserved, {} free).",
                self.volume,
                human_bytes(self.needed_bytes),
                human_bytes(self.available_bytes),
                human_bytes(self.reserved_bytes),
                human_bytes(self.free_bytes)
            )
        }
    }
}

/// Outcome of one reservation attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpaceReservation {
    Granted(SpaceStatus),
    Waiting(SpaceStatus),
}

impl SpaceReservation {
    pub fn status(&self) -> &SpaceStatus {
        match self {
            SpaceReservation::Granted(status) | SpaceReservation::Waiting(status) => status,
        }
    }

    pub fn is_granted(&self) -> bool {
        matches!(self, SpaceReservation::Granted(_))
    }
}

/// Per-volume totals shown in Settings → Storage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerTotal {
    pub volume: String,
    pub reserved_bytes: u64,
    pub jobs: u32,
}

/// `job id -> bytes` per volume key.
type Volumes = BTreeMap<String, BTreeMap<String, u64>>;

/// The reservation ledger. All methods are cheap and lock a single map.
#[derive(Debug, Default)]
pub struct SpaceLedger {
    inner: Mutex<Volumes>,
}

impl SpaceLedger {
    pub fn new() -> Self {
        Self::default()
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, Volumes> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Bytes held by one job on every volume.
    #[cfg(test)]
    pub fn job_bytes(&self, job_id: &str) -> u64 {
        let guard = self.guard();
        guard
            .values()
            .filter_map(|jobs| jobs.get(job_id).copied())
            .sum()
    }

    /// Reserve every volume of `plan` for `job_id`, or nothing at all.
    ///
    /// The check and the insert happen under one lock, so two jobs dispatching
    /// at once cannot both pass against the same free space. Reserving the same
    /// job twice is idempotent (same key, same bytes), which is what a
    /// re-evaluation after a queue move or an app restart needs.
    pub fn try_reserve(
        &self,
        job_id: &str,
        plan: &SpacePlan,
        capacity: &dyn Capacity,
    ) -> SpaceReservation {
        if plan.is_empty() {
            return SpaceReservation::Granted(SpaceStatus::unknown());
        }
        let mut guard = self.guard();
        let mut pending: Vec<(&SpaceRequirement, Option<u64>, u64)> = Vec::new();
        let mut shortfall: Option<SpaceStatus> = None;

        for requirement in &plan.requirements {
            let volume = &requirement.volume.id.0;
            let free = capacity.free_bytes(&requirement.volume.path);
            let reserved = guard
                .get(volume)
                .map(|jobs| {
                    jobs.iter()
                        .filter(|(id, _)| id.as_str() != job_id)
                        .map(|(_, bytes)| *bytes)
                        .sum::<u64>()
                })
                .unwrap_or(0);
            let Some(free) = free else {
                // Unprobeable volume (unplugged drive): the write fails with a
                // precise error, blocking every job here would be wrong.
                continue;
            };
            let available = free.saturating_sub(reserved);
            if available < requirement.bytes {
                shortfall.get_or_insert_with(|| SpaceStatus {
                    volume: requirement.volume.path.display().to_string(),
                    needed_bytes: requirement.bytes,
                    free_bytes: free,
                    reserved_bytes: reserved,
                    available_bytes: available,
                    ok: false,
                });
            }
            pending.push((requirement, Some(free), reserved));
        }

        if let Some(shortfall) = shortfall {
            return SpaceReservation::Waiting(shortfall);
        }

        let mut granted: Option<SpaceStatus> = None;
        for (requirement, free, reserved) in pending {
            let Some(free) = free else { continue };
            guard
                .entry(requirement.volume.id.0.clone())
                .or_default()
                .insert(job_id.to_string(), requirement.bytes);
            let status = SpaceStatus {
                volume: requirement.volume.path.display().to_string(),
                needed_bytes: requirement.bytes,
                free_bytes: free,
                reserved_bytes: reserved,
                available_bytes: free.saturating_sub(reserved),
                ok: true,
            };
            // Report the binding volume: the one asking for the most bytes.
            if granted
                .as_ref()
                .map(|current| status.needed_bytes > current.needed_bytes)
                .unwrap_or(true)
            {
                granted = Some(status);
            }
        }
        SpaceReservation::Granted(granted.unwrap_or_else(SpaceStatus::unknown))
    }

    /// Release every reservation of a job (done, failed, paused or cancelled).
    pub fn release(&self, job_id: &str) {
        let mut guard = self.guard();
        guard.retain(|_, jobs| {
            jobs.remove(job_id);
            !jobs.is_empty()
        });
    }

    /// Forget everything: the startup rebuild starts from the active jobs.
    pub fn reset(&self) {
        self.guard().clear();
    }

    /// Reserved bytes per volume, roomiest volume name first.
    pub fn totals(&self) -> Vec<LedgerTotal> {
        let guard = self.guard();
        let mut totals: Vec<LedgerTotal> = guard
            .iter()
            .map(|(volume, jobs)| LedgerTotal {
                volume: volume.clone(),
                reserved_bytes: jobs.values().sum(),
                jobs: jobs.len() as u32,
            })
            .filter(|total| total.reserved_bytes > 0)
            .collect();
        totals.sort_by(|left, right| {
            right
                .reserved_bytes
                .cmp(&left.reserved_bytes)
                .then_with(|| left.volume.cmp(&right.volume))
        });
        totals
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const GIB: u64 = 1024 * 1024 * 1024;

    /// Fixed free space per volume path, so tests never touch the real disk.
    struct FakeCapacity {
        free: HashMap<&'static str, u64>,
    }

    impl FakeCapacity {
        fn new(entries: &[(&'static str, u64)]) -> Self {
            Self {
                free: entries.iter().copied().collect(),
            }
        }

        fn set(&mut self, path: &'static str, free: u64) {
            self.free.insert(path, free);
        }
    }

    impl Capacity for FakeCapacity {
        fn free_bytes(&self, path: &Path) -> Option<u64> {
            self.free.get(path.to_str()?).copied()
        }
    }

    /// `volume_for` needs real volumes; tests build the plan by hand instead.
    fn plan_one(volume: &str, raw: u64) -> SpacePlan {
        SpacePlan {
            archive_bytes: Some(raw),
            extracted_bytes: None,
            extraction_estimated: false,
            requirements: vec![SpaceRequirement {
                volume: Volume {
                    id: VolumeId(format!("test:{volume}")),
                    path: PathBuf::from(volume),
                },
                raw_bytes: raw,
                bytes: with_margin(raw),
                label: "archive + extracted files",
            }],
        }
    }

    #[test]
    fn the_margin_is_five_percent_plus_two_gib() {
        assert_eq!(with_margin(0), 0);
        let ten_gib = 10 * GIB;
        assert_eq!(with_margin(ten_gib), ten_gib + ten_gib / 20 + 2 * GIB);
    }

    #[test]
    fn enough_space_grants_and_blocks_the_next_job() {
        let ledger = SpaceLedger::new();
        // One 40 GiB archive needs ~44 GiB with the margin: two of them do not
        // fit in 60 GiB, which is exactly the overcommit #222 closes.
        let capacity = FakeCapacity::new(&[("/mnt/games", 60 * GIB)]);
        let plan = plan_one("/mnt/games", 40 * GIB);
        let needed = plan.requirements[0].bytes;

        let first = ledger.try_reserve("job-a", &plan, &capacity);
        assert!(first.is_granted(), "{first:?}");
        assert_eq!(ledger.job_bytes("job-a"), needed);

        // The second job sees the first reservation and waits instead of
        // overcommitting the disk.
        let second = ledger.try_reserve("job-b", &plan, &capacity);
        assert!(!second.is_granted(), "the second job must wait: {second:?}");
        let status = second.status();
        assert_eq!(status.needed_bytes, needed);
        assert_eq!(status.reserved_bytes, needed);
        assert_eq!(status.free_bytes, 60 * GIB);
        assert_eq!(status.available_bytes, 60 * GIB - needed);
        assert!(status.message().contains("Waiting for disk space"));
        assert!(status.message().contains("/mnt/games"));

        // Releasing the first job starts the second on the next evaluation.
        ledger.release("job-a");
        assert_eq!(ledger.job_bytes("job-a"), 0);
        let second = ledger.try_reserve("job-b", &plan, &capacity);
        assert!(second.is_granted(), "{second:?}");
    }

    #[test]
    fn a_resume_reuses_its_own_reservation_without_double_counting() {
        let ledger = SpaceLedger::new();
        let capacity = FakeCapacity::new(&[("/mnt/games", 50 * GIB)]);
        let plan = plan_one("/mnt/games", 20 * GIB);
        let needed = plan.requirements[0].bytes;

        assert!(ledger.try_reserve("job-a", &plan, &capacity).is_granted());
        // Resuming or re-evaluating the same job must not count it twice.
        assert!(ledger.try_reserve("job-a", &plan, &capacity).is_granted());
        assert_eq!(ledger.job_bytes("job-a"), needed);
        assert_eq!(ledger.totals()[0].jobs, 1);
    }

    #[test]
    fn different_volumes_are_independent() {
        let ledger = SpaceLedger::new();
        let capacity = FakeCapacity::new(&[("/mnt/fast", 60 * GIB), ("/mnt/slow", 60 * GIB)]);
        let fast = plan_one("/mnt/fast", 40 * GIB);
        let slow = plan_one("/mnt/slow", 40 * GIB);

        assert!(ledger.try_reserve("job-a", &fast, &capacity).is_granted());
        // A job on the other disk is not blocked by the first reservation.
        assert!(ledger.try_reserve("job-b", &slow, &capacity).is_granted());
        assert_eq!(ledger.totals().len(), 2);
    }

    #[test]
    fn a_not_enough_job_holds_nothing() {
        let ledger = SpaceLedger::new();
        let capacity = FakeCapacity::new(&[("/mnt/games", 10 * GIB)]);
        let plan = plan_one("/mnt/games", 40 * GIB);
        let reservation = ledger.try_reserve("job-a", &plan, &capacity);
        assert!(!reservation.is_granted());
        // A refused attempt must not leave a partial reservation behind.
        assert_eq!(ledger.job_bytes("job-a"), 0);
        assert!(ledger.totals().is_empty());
    }

    #[test]
    fn an_external_free_space_drop_holds_the_next_job() {
        let mut capacity = FakeCapacity::new(&[("/mnt/games", 100 * GIB)]);
        let ledger = SpaceLedger::new();
        let plan = plan_one("/mnt/games", 20 * GIB);
        assert!(ledger.try_reserve("job-a", &plan, &capacity).is_granted());

        // Something outside Ludex filled the disk before the next admission.
        capacity.set("/mnt/games", 5 * GIB);
        let next = ledger.try_reserve("job-b", &plan, &capacity);
        assert!(!next.is_granted());
        assert_eq!(next.status().free_bytes, 5 * GIB);
    }

    #[test]
    fn an_unknown_extracted_size_uses_the_factor_and_is_flagged() {
        let plan = SpacePlan::build(
            Some(40 * GIB),
            None,
            std::env::current_dir().unwrap().as_path(),
            std::env::current_dir().unwrap().as_path(),
            false,
            false,
        );
        assert!(plan.extraction_estimated);
        assert_eq!(plan.extracted_bytes, Some(60 * GIB));
        assert_eq!(plan.requirements.len(), 1);
        let requirement = &plan.requirements[0];
        assert_eq!(requirement.raw_bytes, 100 * GIB);
        assert_eq!(requirement.bytes, with_margin(100 * GIB));
        assert_eq!(requirement.label, "archive + extracted files");
    }

    #[test]
    fn install_volume_staging_counts_the_archive_on_the_install_volume() {
        let root = std::env::temp_dir().join(format!(
            "ludex-space-staging-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let install = root.join("games");
        let staging = install.join(".ludex-tmp");
        // #226: `installVolume` puts the archive next to the extracted tree, so
        // the peak is one requirement on one volume, not two.
        let plan = SpacePlan::build(Some(40 * GIB), None, &install, &staging, false, false);
        assert_eq!(plan.requirements.len(), 1);
        assert_eq!(plan.requirements[0].raw_bytes, 100 * GIB);
        assert_eq!(plan.requirements[0].label, "archive + extracted files");
        assert_eq!(plan.requirements[0].volume.path, root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stream_installs_never_reserve_the_archive() {
        let root = std::env::current_dir().unwrap();
        let plan = SpacePlan::build(Some(40 * GIB), None, &root, &root, true, false);
        assert_eq!(plan.archive_bytes, Some(40 * GIB));
        assert_eq!(plan.requirements.len(), 1);
        // Only the extracted tree (estimated at 1.5×) is written.
        assert_eq!(plan.requirements[0].raw_bytes, 60 * GIB);
        assert_eq!(plan.requirements[0].label, "extracted files");
    }

    #[test]
    fn an_unknown_size_reserves_nothing() {
        let root = std::env::current_dir().unwrap();
        let plan = SpacePlan::build(None, None, &root, &root, false, false);
        assert!(plan.is_empty());
        let ledger = SpaceLedger::new();
        let capacity = FakeCapacity::new(&[("/", 0)]);
        // Nothing to check: the transfer preflight and the write error take over.
        assert!(ledger.try_reserve("job-a", &plan, &capacity).is_granted());
    }

    #[test]
    fn an_unprobeable_volume_does_not_block_every_job() {
        let ledger = SpaceLedger::new();
        // An unplugged drive answers no free space at all.
        let capacity = FakeCapacity::new(&[]);
        let plan = plan_one("/mnt/unplugged", 40 * GIB);
        let reservation = ledger.try_reserve("job-a", &plan, &capacity);
        assert!(reservation.is_granted(), "{reservation:?}");
        assert_eq!(ledger.job_bytes("job-a"), 0);
    }
}
