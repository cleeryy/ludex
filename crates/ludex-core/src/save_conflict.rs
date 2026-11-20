//! Save conflict policy for multi-device play (#296).
//!
//! One pure decision function, shared by the server (upload arbitration) and
//! the desktop sync engine so both sides of a sync reach the same verdict. The
//! chain is applied in order and the first match wins:
//!
//! 1. **Same content hash** — nothing happened; the upload is a no-op.
//! 2. **Per-game "always prefer this device"** — the preferred device wins
//!    silently. When neither side is the preferred device the chain continues.
//! 3. **Device identity** — two uploads from the same device are one line of
//!    progress, and a side without an id cannot prove a multi-device
//!    divergence. Both fast-forward instead of inventing a conflict.
//! 4. **Strict descendant** — the larger side is newer by more than the
//!    tolerance and, when both sides carry the game's own counter, the counter
//!    agrees. Its mtime is only trusted when its clock is not skewed.
//! 5. **Clock skew** — a side whose clock is off by more than
//!    [`CLOCK_SKEW_LIMIT_SECS`] may never win on mtime alone.
//! 6. **Otherwise: conflict** — never overwrite. The caller keeps both
//!    snapshots (`save` and [`conflict_label`]) and surfaces the choice.
//!
//! Timestamps are compared in server time: `effective_mtime` subtracts the
//! side's clock offset, which is defined as *device clock minus server clock*
//! at the moment the side was recorded.

use chrono::{DateTime, SecondsFormat, Utc};

/// A device whose clock is further than five minutes from the server may not
/// win a save conflict on mtime alone.
pub const CLOCK_SKEW_LIMIT_SECS: i64 = 300;

/// Freshness tolerance for a strict-descendant comparison. Two sides written a
/// few seconds apart are too close to call a lineage.
pub const MTIME_TOLERANCE_SECS: i64 = 2;

/// One side of a possible conflict, reduced to the facts the policy needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveSide {
    /// Lowercase hex SHA-256 of the archive.
    pub sha256: String,
    /// Archive size in bytes.
    pub size: i64,
    /// Newest save file mtime reported by the device, in device time.
    pub client_mtime: Option<DateTime<Utc>>,
    /// Device clock minus server clock, in seconds.
    pub clock_offset_secs: i64,
    /// The game's own save counter, when a manifest exposes one.
    pub save_seq: Option<i64>,
    /// Opaque device identifier, when known.
    pub device_id: Option<String>,
    /// Server receipt time; the fallback when no mtime was reported.
    pub recorded_at: DateTime<Utc>,
}

impl SaveSide {
    /// Mtime in server time. Falls back to the receipt time when the client
    /// reported nothing, so a bare upload still has a comparable instant.
    #[must_use]
    pub fn effective_mtime(&self) -> DateTime<Utc> {
        match self.client_mtime {
            Some(mtime) => mtime - chrono::Duration::seconds(self.clock_offset_secs),
            None => self.recorded_at,
        }
    }

    /// True when this device's clock is too far from the server to be trusted
    /// for ordering.
    #[must_use]
    pub fn clock_skewed(&self) -> bool {
        self.clock_offset_secs.abs() > CLOCK_SKEW_LIMIT_SECS
    }
}

/// Which side of the comparison a decision or a lineage refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveSideKind {
    /// The side already stored on the server.
    Existing,
    /// The side being uploaded (or held locally by the desktop).
    Candidate,
}

impl SaveSideKind {
    /// Stable spelling used in API payloads.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Existing => "existing",
            Self::Candidate => "candidate",
        }
    }
}

/// Why a save conflict was raised. Stored with the conflict row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictReason {
    /// Both sides changed and neither is a strict descendant of the other.
    BothChanged,
    /// A side's clock is too far from the server to order the saves safely.
    ClockSkew,
    /// The game's own counter contradicts the size/mtime lineage.
    CounterDiverged,
}

impl ConflictReason {
    /// Stable spelling used in API payloads and the database.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BothChanged => "both_changed",
            Self::ClockSkew => "clock_skew",
            Self::CounterDiverged => "counter_diverged",
        }
    }

    /// Parse the stored spelling; unknown values degrade to `both_changed`.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "clock_skew" => Self::ClockSkew,
            "counter_diverged" => Self::CounterDiverged,
            _ => Self::BothChanged,
        }
    }
}

/// Outcome of the policy chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveDecision {
    /// The two sides hold identical bytes.
    Unchanged,
    /// One side is a strict descendant: it wins without asking.
    FastForward(SaveSideKind),
    /// Never overwrite: keep both and surface the choice.
    Conflict(ConflictReason),
}

/// Applies the policy chain to two sides of one save.
///
/// `prefer_device` is the per-game "always prefer this device" override.
#[must_use]
pub fn decide(
    existing: &SaveSide,
    candidate: &SaveSide,
    prefer_device: Option<&str>,
) -> SaveDecision {
    if existing.sha256 == candidate.sha256 {
        return SaveDecision::Unchanged;
    }
    if let Some(preferred) = prefer_device.map(str::trim).filter(|id| !id.is_empty()) {
        let existing_preferred = existing.device_id.as_deref() == Some(preferred);
        let candidate_preferred = candidate.device_id.as_deref() == Some(preferred);
        match (existing_preferred, candidate_preferred) {
            (false, true) => return SaveDecision::FastForward(SaveSideKind::Candidate),
            (true, false) => return SaveDecision::FastForward(SaveSideKind::Existing),
            _ => {}
        }
    }

    // Device identity gates the rest of the chain. Two uploads from the same
    // device are one line of progress, and a side without an id (a manual
    // upload or an older client) cannot prove a multi-device divergence: both
    // cases fast-forward instead of inventing a conflict.
    match (
        existing.device_id.as_deref(),
        candidate.device_id.as_deref(),
    ) {
        (Some(existing_device), Some(candidate_device)) if existing_device != candidate_device => {}
        _ => return SaveDecision::FastForward(SaveSideKind::Candidate),
    }

    let lineage = strict_descendant(existing, candidate);
    let counter = match (existing.save_seq, candidate.save_seq) {
        (Some(old), Some(new)) if old != new => Some(new.cmp(&old)),
        _ => None,
    };
    match lineage {
        Some(SaveSideKind::Candidate) => {
            if counter == Some(std::cmp::Ordering::Less) {
                SaveDecision::Conflict(ConflictReason::CounterDiverged)
            } else {
                SaveDecision::FastForward(SaveSideKind::Candidate)
            }
        }
        Some(SaveSideKind::Existing) => {
            if counter == Some(std::cmp::Ordering::Greater) {
                SaveDecision::Conflict(ConflictReason::CounterDiverged)
            } else {
                SaveDecision::FastForward(SaveSideKind::Existing)
            }
        }
        None if existing.clock_skewed() || candidate.clock_skewed() => {
            SaveDecision::Conflict(ConflictReason::ClockSkew)
        }
        None => SaveDecision::Conflict(ConflictReason::BothChanged),
    }
}

/// Strict descendant test: strictly larger and strictly newer in server time.
///
/// A side with a skewed clock cannot establish a lineage; the caller turns that
/// into a conflict instead of letting the wrong clock win.
fn strict_descendant(existing: &SaveSide, candidate: &SaveSide) -> Option<SaveSideKind> {
    if existing.clock_skewed() || candidate.clock_skewed() {
        return None;
    }
    if candidate.size > existing.size
        && candidate.effective_mtime()
            > existing.effective_mtime() + chrono::Duration::seconds(MTIME_TOLERANCE_SECS)
    {
        return Some(SaveSideKind::Candidate);
    }
    if existing.size > candidate.size
        && existing.effective_mtime()
            > candidate.effective_mtime() + chrono::Duration::seconds(MTIME_TOLERANCE_SECS)
    {
        return Some(SaveSideKind::Existing);
    }
    None
}

/// Label of the side that loses a conflict: `save (conflict <device> <date>)`.
///
/// The loser is relabelled, never deleted. The device is shortened and cleaned
/// so a hostile device id cannot smuggle controls into the UI.
#[must_use]
pub fn conflict_label(device: Option<&str>, at: DateTime<Utc>) -> String {
    let device = device
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown device");
    let device: String = device
        .chars()
        .filter(|character| !character.is_control())
        .take(40)
        .collect();
    let date = at.to_rfc3339_opts(SecondsFormat::Secs, true);
    let date = date.get(..16).unwrap_or(&date).replace('T', " ");
    format!("save (conflict {device} {date})")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(hash: &str, size: i64, mtime: &str, offset: i64, device: &str) -> SaveSide {
        SaveSide {
            sha256: hash.to_string(),
            size,
            client_mtime: Some(
                DateTime::parse_from_rfc3339(mtime)
                    .expect("mtime")
                    .with_timezone(&Utc),
            ),
            clock_offset_secs: offset,
            save_seq: None,
            device_id: Some(device.to_string()),
            recorded_at: DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .expect("recorded")
                .with_timezone(&Utc),
        }
    }

    #[test]
    fn identical_bytes_are_unchanged() {
        let existing = side("aaa", 100, "2026-01-01T12:00:00Z", 0, "laptop");
        let candidate = side("aaa", 100, "2026-01-01T15:00:00Z", 0, "desktop");
        assert_eq!(decide(&existing, &candidate, None), SaveDecision::Unchanged);
    }

    #[test]
    fn three_hour_gap_with_both_sides_modified_is_a_conflict() {
        // Remote played at noon (200 bytes); the desktop played elsewhere and
        // produced a different, smaller save at 15:00. Neither is a descendant.
        let existing = side("remote", 200, "2026-01-01T12:00:00Z", 0, "steam-deck");
        let candidate = side("local", 180, "2026-01-01T15:00:00Z", 0, "laptop");
        assert_eq!(
            decide(&existing, &candidate, None),
            SaveDecision::Conflict(ConflictReason::BothChanged)
        );
    }

    #[test]
    fn a_larger_newer_side_is_a_strict_descendant() {
        let existing = side("old", 200, "2026-01-01T12:00:00Z", 0, "steam-deck");
        let candidate = side("new", 240, "2026-01-01T15:00:00Z", 0, "laptop");
        assert_eq!(
            decide(&existing, &candidate, None),
            SaveDecision::FastForward(SaveSideKind::Candidate)
        );
        assert_eq!(
            decide(&candidate, &existing, None),
            SaveDecision::FastForward(SaveSideKind::Existing)
        );
    }

    #[test]
    fn consecutive_uploads_from_one_device_continue_the_same_line() {
        let existing = side("old", 200, "2026-01-01T12:00:00Z", 0, "laptop");
        let candidate = side("new", 180, "2026-01-01T12:05:00Z", 0, "laptop");
        assert_eq!(
            decide(&existing, &candidate, None),
            SaveDecision::FastForward(SaveSideKind::Candidate)
        );
    }

    #[test]
    fn a_device_two_hours_ahead_does_not_win_silently() {
        // The candidate reports a newer mtime only because its clock is +2 h.
        // Adjusted to server time it is the same instant as the existing side.
        let existing = side("old", 200, "2026-01-01T12:00:00Z", 0, "steam-deck");
        let candidate = side("new", 240, "2026-01-01T14:00:00Z", 7_200, "laptop");
        assert!(candidate.clock_skewed());
        assert_eq!(
            decide(&existing, &candidate, None),
            SaveDecision::Conflict(ConflictReason::ClockSkew)
        );
    }

    #[test]
    fn a_skewed_clock_side_never_supplies_the_lineage() {
        let existing = side("old", 200, "2026-01-01T12:00:00Z", 0, "steam-deck");
        // Even 30 s newer in adjusted time, the skewed candidate cannot win.
        let candidate = side("new", 240, "2026-01-01T14:01:00Z", 7_200, "laptop");
        assert_eq!(
            decide(&existing, &candidate, None),
            SaveDecision::Conflict(ConflictReason::ClockSkew)
        );
    }

    #[test]
    fn the_counter_can_veto_a_size_and_mtime_lineage() {
        let mut existing = side("old", 200, "2026-01-01T12:00:00Z", 0, "steam-deck");
        let mut candidate = side("new", 240, "2026-01-01T15:00:00Z", 0, "laptop");
        existing.save_seq = Some(9);
        candidate.save_seq = Some(7);
        assert_eq!(
            decide(&existing, &candidate, None),
            SaveDecision::Conflict(ConflictReason::CounterDiverged)
        );
        candidate.save_seq = Some(10);
        assert_eq!(
            decide(&existing, &candidate, None),
            SaveDecision::FastForward(SaveSideKind::Candidate)
        );
    }

    #[test]
    fn preferring_a_device_short_circuits_the_chain() {
        let existing = side("a", 200, "2026-01-01T15:00:00Z", 0, "steam-deck");
        // Larger but older: neither side is a strict descendant.
        let candidate = side("b", 220, "2026-01-01T12:00:00Z", 0, "laptop");
        assert_eq!(
            decide(&existing, &candidate, Some("laptop")),
            SaveDecision::FastForward(SaveSideKind::Candidate)
        );
        assert_eq!(
            decide(&existing, &candidate, Some("steam-deck")),
            SaveDecision::FastForward(SaveSideKind::Existing)
        );
        // Neither side is the preferred device: the chain keeps applying.
        assert_eq!(
            decide(&existing, &candidate, Some("console")),
            SaveDecision::Conflict(ConflictReason::BothChanged)
        );
    }

    #[test]
    fn the_loser_label_is_readable_and_safe() {
        let at = DateTime::parse_from_rfc3339("2026-11-19T14:05:00Z")
            .expect("time")
            .with_timezone(&Utc);
        assert_eq!(
            conflict_label(Some("steam-deck"), at),
            "save (conflict steam-deck 2026-11-19 14:05)"
        );
        assert_eq!(
            conflict_label(None, at),
            "save (conflict unknown device 2026-11-19 14:05)"
        );
        let hostile = conflict_label(Some("bad\u{0}device"), at);
        assert!(!hostile.chars().any(char::is_control), "{hostile}");
    }
}
