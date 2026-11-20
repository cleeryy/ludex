//! Auto-revert for failing launch profiles (#298).
//!
//! Ludex already writes a per-launch compatibility log (#280) and documents the
//! exit-code table (#122). This module adds the decision: a launch that never
//! reaches its first frame is a **failure**, and the second consecutive failure
//! with the same profile puts back the last plan that worked for that game.
//!
//! Failure is decided from evidence the desktop already has:
//!
//! * the process tree ended inside [`FIRST_FRAME_BUDGET`] (30 s) — the
//!   documented startup budget — so no first frame was ever painted;
//! * or the wrapper reported one of the known fatal exit codes (`53` / `0x35`,
//!   `0xC0000409`) that only appear before a frame.
//!
//! A launch the player stopped on purpose, or a session that ran past the
//! budget, is never a failure: a game quit after an hour must not cost the
//! setup that worked.
//!
//! State lives in `{appDataDir}/launch-recovery.json`, next to the launch logs:
//! no database migration, and deleting the file only forgets the counters. The
//! file stores the last plan that worked per game (so a later revert survives a
//! restart) and the counters keyed by the SHA-256 of the profile, so changing
//! one option starts a new streak. The profile hash — never the files — is what
//! a report includes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::linux_gaming::{DisplayLaunch, LaunchMode, LaunchOptions, LaunchPlan, ResolvedLaunch};

/// The documented startup budget (#122, docs/LINUX-GAMING.md): a game that has
/// not painted a frame by then is still starting, not failing.
pub const FIRST_FRAME_BUDGET: Duration = Duration::from_secs(30);

/// Consecutive failures with the same profile before the revert.
pub const FAILURES_BEFORE_REVERT: u32 = 2;

/// File name inside the app data folder.
pub const STATE_FILE: &str = "launch-recovery.json";

/// Exit codes that come back from a launch that never reached a frame (#122).
///
/// `53` is `0x35`; UMU and Proton define no such code, so it is the game or its
/// Steam emulator bailing out. `0xC0000409` is the Windows fail-fast status.
pub fn is_fatal_exit(code: Option<u32>) -> bool {
    matches!(code, Some(53) | Some(0xC0000409))
}

/// What one finished session means for the profile that launched it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchOutcome {
    /// A frame was reached: record the plan as last working and reset counters.
    Success,
    /// The launch never reached its first frame.
    Failure,
    /// A deliberate stop: neither a failure nor proof the plan works.
    NotCounted,
}

/// Classify a finished session.
///
/// `stopped` is the flag `stop_game`/force close sets; an intentional stop must
/// never count against the profile.
pub fn classify(elapsed: Duration, exit_code: Option<u32>, stopped: bool) -> LaunchOutcome {
    if stopped {
        return LaunchOutcome::NotCounted;
    }
    if elapsed < FIRST_FRAME_BUDGET || is_fatal_exit(exit_code) {
        LaunchOutcome::Failure
    } else {
        LaunchOutcome::Success
    }
}

/// The plan a launch resolved to, as recorded for diagnostics (#298).
///
/// Values are stored locally only; the bundle redacts paths, tokens and the
/// account name before anything leaves the machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResolvedPlan {
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub prefix: String,
    pub tool_id: String,
    pub tool_name: String,
    /// Where the game was presented (`desktop`, a Gamescope line, a fallback).
    pub display: String,
}

impl ResolvedPlan {
    /// The resolved compatibility plan, before the Gamescope wrapper (#287).
    pub fn from_plan(plan: &LaunchPlan, display: &ResolvedLaunch) -> Self {
        Self {
            program: plan.program.to_string_lossy().to_string(),
            args: plan.args.clone(),
            env: plan.env.clone(),
            prefix: plan.prefix.to_string_lossy().to_string(),
            tool_id: plan.tool_id.clone(),
            tool_name: plan.tool_name.clone(),
            display: display_line(display),
        }
    }

    /// A native launch (or a Gamescope-wrapped one) has no compatibility tool.
    pub fn native(
        program: &Path,
        args: &[String],
        prefix: Option<&Path>,
        display: Option<&ResolvedLaunch>,
    ) -> Self {
        Self {
            program: program.to_string_lossy().to_string(),
            args: args.to_vec(),
            env: BTreeMap::new(),
            prefix: prefix
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_default(),
            tool_id: String::new(),
            tool_name: String::new(),
            display: display
                .map(display_line)
                .unwrap_or_else(|| "desktop".to_string()),
        }
    }
}

/// One human line for the presentation profile, for the launch log and bundle.
pub fn display_line(display: &ResolvedLaunch) -> String {
    match &display.display {
        DisplayLaunch::Desktop => "desktop".to_string(),
        DisplayLaunch::Fallback { reason } => format!("desktop (gamescope not applied: {reason})"),
        DisplayLaunch::Gamescope(command) => format!(
            "gamescope (hdr: {}, adaptive sync: {}, filter: {})",
            command.hdr,
            command.adaptive_sync,
            command.filter.as_deref().unwrap_or("default")
        ),
    }
}

/// One launch setup, with the hash that keys its failure streak.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanSnapshot {
    /// SHA-256 of the identity below; stable across restarts, never reversible.
    pub hash: String,
    /// The options needed to put this plan back.
    pub options: LaunchOptions,
    #[serde(default)]
    pub executable: Option<String>,
    #[serde(default)]
    pub args: Option<String>,
    /// What the launch actually resolved to, when it got that far.
    #[serde(default)]
    pub resolved: Option<ResolvedPlan>,
}

impl PlanSnapshot {
    pub fn new(options: &LaunchOptions, executable: Option<&str>, args: Option<&str>) -> Self {
        Self {
            hash: plan_hash(options, executable, args),
            options: options.clone(),
            executable: executable.map(str::to_string),
            args: args.map(str::to_string),
            resolved: None,
        }
    }

    pub fn with_resolved(mut self, resolved: Option<ResolvedPlan>) -> Self {
        self.resolved = resolved;
        self
    }
}

/// Stable hash of the fields that make a launch profile.
///
/// The environment is included because `DXVK_*`/`WINEDLLOVERRIDES` change the
/// outcome; the hash is one-way, so it is safe in a report.
pub fn plan_hash(options: &LaunchOptions, executable: Option<&str>, args: Option<&str>) -> String {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Identity<'a> {
        mode: LaunchMode,
        tool_id: &'a str,
        prefix_path: &'a str,
        env: &'a BTreeMap<String, String>,
        winetricks_verbs: &'a [String],
        steam_app_id: Option<u32>,
        working_dir: &'a str,
        display: &'a crate::linux_gaming::DisplayProfile,
        executable: Option<&'a str>,
        args: Option<&'a str>,
    }

    let identity = Identity {
        mode: options.mode,
        tool_id: &options.tool_id,
        prefix_path: &options.prefix_path,
        env: &options.env,
        winetricks_verbs: &options.winetricks_verbs,
        steam_app_id: options.steam_app_id,
        working_dir: &options.working_dir,
        display: &options.display,
        executable,
        args,
    };
    let encoded = serde_json::to_vec(&identity).unwrap_or_default();
    let digest = Sha256::digest(encoded);
    hex::encode(digest)
}

/// Last failure, kept for the bundle's report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureRecord {
    /// RFC 3339 UTC.
    pub at: String,
    pub exit_code: Option<u32>,
    pub seconds: u64,
}

/// Per-game recovery state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GameRecovery {
    /// Failures in the current streak, capped at [`FAILURES_BEFORE_REVERT`].
    pub failures: u32,
    /// Profile the streak belongs to; a different hash starts a new streak.
    pub failing_hash: Option<String>,
    /// The last plan that reached a frame. Reverted to on the second failure.
    pub last_working: Option<PlanSnapshot>,
    /// The profile of the most recent launch, for the diagnostics bundle.
    pub last_plan: Option<PlanSnapshot>,
    /// How many times this game was reverted, for the log.
    pub reverts: u32,
    /// The most recent failed launch.
    pub last_failure: Option<FailureRecord>,
}

/// The whole file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecoveryState {
    pub version: u32,
    pub games: BTreeMap<i64, GameRecovery>,
}

impl Default for RecoveryState {
    fn default() -> Self {
        Self {
            version: 1,
            games: BTreeMap::new(),
        }
    }
}

/// What the watcher must do after a failure.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FailureOutcome {
    /// The streak after this failure.
    pub failures: u32,
    /// True when this failure triggered the revert.
    pub reverted: bool,
    /// Options to write back to the game row; the third launch uses them.
    pub restore: Option<LaunchOptions>,
    pub from_hash: Option<String>,
    pub to_hash: Option<String>,
}

impl FailureOutcome {
    pub fn notice(&self, title: &str) -> String {
        format!(
            "{title} kept failing to start, so Ludex put back the last launch setup that worked. The next launch uses it."
        )
    }
}

fn lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// `<appDataDir>/launch-recovery.json`.
pub fn state_path(app_data: &Path) -> PathBuf {
    app_data.join(STATE_FILE)
}

/// Read the state; a missing or corrupt file means "nothing learned yet".
///
/// Never returns an error on purpose: recovery bookkeeping must not block a
/// launch, and a truncated write only loses the counters.
pub fn load(path: &Path) -> RecoveryState {
    let Ok(bytes) = std::fs::read(path) else {
        return RecoveryState::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn save(path: &Path, state: &RecoveryState) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("Could not create {}: {error}", dir.display()))?;
    }
    let encoded = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("Could not encode the launch recovery state: {error}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, encoded)
        .map_err(|error| format!("Could not write {}: {error}", tmp.display()))?;
    if std::fs::rename(&tmp, path).is_err() {
        // Windows refuses to replace an existing file with `rename`.
        let _ = std::fs::remove_file(path);
        std::fs::rename(&tmp, path)
            .map_err(|error| format!("Could not replace {}: {error}", path.display()))?;
    }
    Ok(())
}

fn with_state<T>(path: &Path, change: impl FnOnce(&mut RecoveryState) -> T) -> Result<T, String> {
    let _guard = lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut state = load(path);
    let result = change(&mut state);
    save(path, &state)?;
    Ok(result)
}

/// Record a successful launch: this plan is now the one to revert to.
pub fn record_success(path: &Path, game_id: i64, snapshot: &PlanSnapshot) -> Result<(), String> {
    with_state(path, |state| {
        let entry = state.games.entry(game_id).or_default();
        entry.last_working = Some(snapshot.clone());
        entry.last_plan = Some(snapshot.clone());
        entry.failures = 0;
        entry.failing_hash = None;
    })
}

/// Remember the plan of a launch nobody judged (a deliberate stop).
pub fn record_neutral(path: &Path, game_id: i64, snapshot: &PlanSnapshot) -> Result<(), String> {
    with_state(path, |state| {
        state.games.entry(game_id).or_default().last_plan = Some(snapshot.clone());
    })
}

/// Count a failed launch, reverting to the last working plan on the second
/// consecutive failure with the same profile.
pub fn record_failure(
    path: &Path,
    game_id: i64,
    snapshot: &PlanSnapshot,
    failure: FailureRecord,
) -> Result<FailureOutcome, String> {
    with_state(path, |state| {
        let entry = state.games.entry(game_id).or_default();
        entry.last_plan = Some(snapshot.clone());
        entry.last_failure = Some(failure);
        if entry.failing_hash.as_deref() == Some(snapshot.hash.as_str()) {
            entry.failures = entry.failures.saturating_add(1).min(FAILURES_BEFORE_REVERT);
        } else {
            entry.failing_hash = Some(snapshot.hash.clone());
            entry.failures = 1;
        }

        let mut outcome = FailureOutcome {
            failures: entry.failures,
            ..FailureOutcome::default()
        };
        if entry.failures < FAILURES_BEFORE_REVERT {
            return outcome;
        }
        // Only a different, known-working plan can be restored. A profile that
        // failed twice and never worked has nowhere to go; it keeps its streak
        // so the next attempt with the same profile does not retrigger.
        let Some(working) = entry.last_working.clone() else {
            return outcome;
        };
        if working.hash == snapshot.hash {
            return outcome;
        }
        entry.reverts = entry.reverts.saturating_add(1);
        entry.failures = 0;
        entry.failing_hash = None;
        outcome.reverted = true;
        outcome.failures = 0;
        outcome.from_hash = Some(snapshot.hash.clone());
        outcome.to_hash = Some(working.hash.clone());
        outcome.restore = Some(working.options.clone());
        outcome
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux_gaming::{DisplayMode, DisplayProfile};

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-recovery-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(STATE_FILE)
    }

    fn options(tool: &str) -> LaunchOptions {
        LaunchOptions {
            mode: LaunchMode::Umu,
            tool_id: tool.to_string(),
            prefix_path: String::new(),
            env: BTreeMap::new(),
            winetricks_verbs: Vec::new(),
            steam_app_id: Some(753640),
            working_dir: String::new(),
            display: DisplayProfile {
                mode: DisplayMode::Desktop,
                ..DisplayProfile::default()
            },
        }
    }

    fn failure() -> FailureRecord {
        FailureRecord {
            at: "2026-11-19T00:00:00Z".to_string(),
            exit_code: Some(53),
            seconds: 3,
        }
    }

    fn snapshot(tool: &str) -> PlanSnapshot {
        PlanSnapshot::new(
            &options(tool),
            Some("/games/outer-wilds/OuterWilds.exe"),
            None,
        )
        .with_resolved(Some(ResolvedPlan {
            program: "/usr/bin/umu-run".to_string(),
            tool_name: "UMU".to_string(),
            ..ResolvedPlan::default()
        }))
    }

    #[test]
    fn first_failure_never_reverts() {
        let path = temp_path("first");
        let working = snapshot("working");
        let failing = snapshot("failing");
        record_success(&path, 7, &working).unwrap();

        let outcome = record_failure(&path, 7, &failing, failure()).unwrap();
        assert_eq!(outcome.failures, 1);
        assert!(!outcome.reverted);
        assert!(outcome.restore.is_none());

        let state = load(&path);
        assert_eq!(state.games[&7].failures, 1);
        assert_eq!(
            state.games[&7]
                .last_working
                .as_ref()
                .map(|plan| plan.hash.clone()),
            Some(working.hash)
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn second_failure_with_the_same_profile_reverts() {
        let path = temp_path("second");
        let working = snapshot("working");
        let failing = snapshot("failing");
        record_success(&path, 7, &working).unwrap();

        record_failure(&path, 7, &failing, failure()).unwrap();
        let outcome = record_failure(&path, 7, &failing, failure()).unwrap();

        assert_eq!(outcome.failures, 0);
        assert!(outcome.reverted);
        assert_eq!(outcome.from_hash.as_deref(), Some(failing.hash.as_str()));
        assert_eq!(outcome.to_hash.as_deref(), Some(working.hash.as_str()));
        assert_eq!(outcome.restore, Some(working.options.clone()));
        assert_eq!(
            outcome.notice("Outer Wilds"),
            "Outer Wilds kept failing to start, so Ludex put back the last launch setup that worked. The next launch uses it."
        );

        let state = load(&path);
        assert_eq!(state.games[&7].reverts, 1);
        assert_eq!(state.games[&7].failures, 0);
        assert!(state.games[&7].failing_hash.is_none());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn counters_are_keyed_by_profile() {
        let path = temp_path("profiles");
        let working = snapshot("working");
        record_success(&path, 7, &working).unwrap();

        let first = snapshot("tool-a");
        let second = snapshot("tool-b");
        assert_ne!(first.hash, second.hash);
        record_failure(&path, 7, &first, failure()).unwrap();
        let outcome = record_failure(&path, 7, &second, failure()).unwrap();

        assert_eq!(outcome.failures, 1, "a new profile starts a new streak");
        assert!(!outcome.reverted);
        let state = load(&path);
        assert_eq!(
            state.games[&7].failing_hash.as_deref(),
            Some(second.hash.as_str())
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn third_launch_uses_the_previous_working_plan() {
        let path = temp_path("third");
        let working = snapshot("working");
        let failing = snapshot("failing");
        record_success(&path, 9, &working).unwrap();
        record_failure(&path, 9, &failing, failure()).unwrap();
        let outcome = record_failure(&path, 9, &failing, failure()).unwrap();

        // The caller writes `restore` back; the third launch reads it.
        let restored = outcome.restore.expect("a working plan is restored");
        assert_eq!(restored, working.options);
        let third = PlanSnapshot::new(
            &restored,
            working.executable.as_deref(),
            working.args.as_deref(),
        );
        assert_eq!(third.hash, working.hash);

        // Another failure with the restored profile cannot revert again: it is
        // already the last plan that worked.
        record_failure(&path, 9, &third, failure()).unwrap();
        let outcome = record_failure(&path, 9, &third, failure()).unwrap();
        assert!(!outcome.reverted);
        assert!(outcome.restore.is_none());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn no_working_plan_means_no_revert() {
        let path = temp_path("none");
        let failing = snapshot("failing");
        record_failure(&path, 4, &failing, failure()).unwrap();
        let outcome = record_failure(&path, 4, &failing, failure()).unwrap();
        assert!(!outcome.reverted);
        assert!(outcome.restore.is_none());
        assert_eq!(outcome.failures, FAILURES_BEFORE_REVERT);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn success_records_the_plan_and_resets_the_streak() {
        let path = temp_path("success");
        let failing = snapshot("failing");
        record_failure(&path, 5, &failing, failure()).unwrap();
        record_success(&path, 5, &failing).unwrap();

        let state = load(&path);
        assert_eq!(state.games[&5].failures, 0);
        assert!(state.games[&5].failing_hash.is_none());
        assert_eq!(
            state.games[&5]
                .last_working
                .as_ref()
                .map(|plan| plan.hash.clone()),
            Some(failing.hash)
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn classification_uses_the_documented_budget_and_fatal_codes() {
        assert_eq!(
            classify(Duration::from_secs(3), Some(53), false),
            LaunchOutcome::Failure
        );
        assert_eq!(
            classify(Duration::from_secs(29), None, false),
            LaunchOutcome::Failure,
            "an exit inside the 30 s budget never reached a frame"
        );
        assert_eq!(
            classify(Duration::from_secs(31), Some(0), false),
            LaunchOutcome::Success
        );
        assert_eq!(
            classify(Duration::from_secs(3), Some(127), false),
            LaunchOutcome::Failure
        );
        assert_eq!(
            classify(Duration::from_secs(2), Some(0), true),
            LaunchOutcome::NotCounted,
            "a deliberate stop is not a failed launch"
        );
        assert_eq!(
            classify(Duration::from_secs(120), Some(0xC0000409), false),
            LaunchOutcome::Failure
        );
        assert!(is_fatal_exit(Some(53)));
        assert!(is_fatal_exit(Some(0x35)));
        assert!(is_fatal_exit(Some(0xC0000409)));
        assert!(!is_fatal_exit(Some(1)));
    }

    #[test]
    fn state_file_keeps_one_entry_per_game() {
        let path = temp_path("games");
        record_success(&path, 1, &snapshot("a")).unwrap();
        record_success(&path, 2, &snapshot("b")).unwrap();
        let state = load(&path);
        assert_eq!(state.games.len(), 2);
        assert!(state.games.contains_key(&1));
        assert!(state.games.contains_key(&2));
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn corrupt_state_falls_back_to_default() {
        let path = temp_path("corrupt");
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(load(&path), RecoveryState::default());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn changing_one_option_changes_the_hash() {
        let a = options("tool");
        let mut b = a.clone();
        b.env.insert("DXVK_ASYNC".to_string(), "1".to_string());
        assert_ne!(
            plan_hash(&a, Some("/games/a.exe"), None),
            plan_hash(&b, Some("/games/a.exe"), None)
        );
        assert_ne!(
            plan_hash(&a, Some("/games/a.exe"), None),
            plan_hash(&a, Some("/games/b.exe"), None)
        );
    }
}
