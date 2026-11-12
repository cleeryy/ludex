//! Installed-binary self-check (#186).
//!
//! A Windows update is handed to the NSIS installer while Ludex is still
//! running; the plugin exits and the installer replaces `ludex.exe` afterwards.
//! Nothing in that hand-off proves that the binary which starts next is the one
//! that was installed: a per-machine copy, a locked file, or a shortcut that
//! points at another folder all leave the old build running while the web
//! assets look updated. The symptom is a new-looking shell with old native
//! code (launch, config sync, performance fixes) behind it.
//!
//! This module makes the result observable:
//!
//! - every build carries a literal `LUDEX_BUILD_COMMIT=<sha>` marker stamped
//!   by `build.rs`, so any `ludex.exe` on disk can identify itself;
//! - `updater::install` records the version it is about to install in
//!   `{appDataDir}/install-state.json` before the installer runs;
//! - the next start compares that version with the running one and stores a
//!   typed [`InstallMismatch`] instead of reporting success;
//! - the usual Windows install roots are scanned for other copies, and the
//!   startup block (`{appDataDir}/logs/ludex.log`) names the running exe, its
//!   version, its build commit and every duplicate with its own commit.
//!
//! The decisive helpers ([`install_roots_from`], [`scan_install_roots`],
//! [`read_build_stamp`], [`same_version`], [`resolve_pending`]) are pure or
//! plain filesystem code and are unit-tested on every platform. What still
//! needs a real Windows machine is listed in `docs/RELEASING.md` under
//! "Windows install layout (updates, #186)".

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::state::AppState;
use crate::util::now_rfc3339;

/// Commit this binary was built from. `build.rs` always sets the variable;
/// `unknown` only appears in exotic builds that bypass the build script.
pub const BUILD_COMMIT: &str = match option_env!("LUDEX_BUILD_COMMIT") {
    Some(commit) => commit,
    None => "unknown",
};
/// Prefix of the marker embedded in the executable (see `build.rs`).
pub const BUILD_STAMP_PREFIX: &str = "LUDEX_BUILD_COMMIT=";
/// The marker itself, newline-terminated so a neighbouring string literal can
/// never extend the commit. Referenced in [`collect`] so LTO and `strip` keep
/// it in the binary.
const BUILD_STAMP: &str = concat!("LUDEX_BUILD_COMMIT=", env!("LUDEX_BUILD_COMMIT"), "\n");
/// Never scan more than this much of a candidate executable.
const STAMP_SCAN_LIMIT: u64 = 96 * 1024 * 1024;
/// The startup block is appended; past this size the file is restarted so it
/// cannot grow without bound on a machine that runs Ludex for years.
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
/// Machine-readable install state, kept out of `settings.json` so a settings
/// save from a stale tab can never drop it.
const STATE_FILE: &str = "install-state.json";
/// Startup log the issue asks for.
const LOG_FILE: &str = "logs/ludex.log";

/// One `ludex.exe` found under a known install root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallCandidate {
    /// Full path of the executable.
    pub path: String,
    /// Root that was scanned.
    pub root: String,
    pub size_bytes: Option<u64>,
    /// Modification time, RFC 3339 UTC.
    pub modified_at: Option<String>,
    /// Build commit read from the executable's marker. `None` means the file is
    /// unreadable or predates the marker (an old build).
    pub commit: Option<String>,
    /// True when this copy is not the running build. An unreadable or missing
    /// marker counts as different: such a copy cannot be the current one.
    pub differs: bool,
}

/// A recorded update that did not produce the version it installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallMismatch {
    /// Version that was running when the mismatch was detected.
    pub running_version: String,
    /// Version the updater reported as installed.
    pub expected_version: String,
    /// Executable that was running.
    pub exe: Option<String>,
    /// Detection time, RFC 3339 UTC.
    pub detected_at: String,
}

/// Everything the Settings/update UI needs about the installed binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallHealth {
    /// Path of the running executable, when the OS reports one.
    pub running_exe: Option<String>,
    pub running_version: String,
    pub build_commit: String,
    /// `windows`, `linux`, `macos` or another `std::env::consts::OS` value.
    pub platform: String,
    /// `{appDataDir}/logs/ludex.log` (whether or not it exists yet).
    pub log_path: Option<String>,
    /// Roots that were scanned for other copies (empty outside Windows).
    pub install_roots: Vec<String>,
    /// Other `ludex.exe` copies in the usual roots.
    pub duplicates: Vec<InstallCandidate>,
    /// Version the updater said it installed, until the next start verifies it.
    pub pending_version: Option<String>,
    /// Last update that did not take effect, until it is dismissed or a newer
    /// build runs.
    pub mismatch: Option<InstallMismatch>,
}

/// What the pending-version check concluded on this start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingOutcome {
    /// No update was pending.
    None,
    /// The installed version is the running one.
    Verified { version: String },
    /// The updater claimed a version that is not running.
    Mismatch(InstallMismatch),
}

/// Persisted update bookkeeping (not part of `LocalSettings`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InstallState {
    /// Version recorded by `updater::install` right before the installer ran.
    pub pending_version: Option<String>,
    pub pending_at: Option<String>,
    /// Last detected mismatch, kept until dismissed or superseded.
    pub last_mismatch: Option<InstallMismatch>,
}

/* ------------------------------ pure helpers ----------------------------- */

/// Typical install roots, in scan order and without duplicates.
///
/// Tauri's per-user NSIS mode installs into `%LOCALAPPDATA%\{productName}`;
/// `%LOCALAPPDATA%\Programs\Ludex` covers the other common per-user layout and
/// the two `Program Files` trees cover a per-machine install made by an older
/// or hand-edited installer.
#[must_use]
pub fn install_roots_from(
    local_app_data: Option<&Path>,
    program_files: Option<&Path>,
    program_files_x86: Option<&Path>,
) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(local) = local_app_data {
        roots.push(local.join("Ludex"));
        roots.push(local.join("Programs").join("Ludex"));
    }
    if let Some(program_files) = program_files {
        roots.push(program_files.join("Ludex"));
    }
    if let Some(program_files_x86) = program_files_x86 {
        roots.push(program_files_x86.join("Ludex"));
    }
    dedupe_paths(roots)
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut unique: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for path in paths {
        let text = path.to_string_lossy();
        if !unique
            .iter()
            .any(|existing| existing.to_string_lossy().eq_ignore_ascii_case(&text))
        {
            unique.push(path);
        }
    }
    unique
}

/// Install roots read from the real environment. Windows only: the other
/// platforms have no `ludex.exe` in these locations.
#[must_use]
pub fn platform_install_roots() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let program_files = std::env::var_os("PROGRAMFILES").map(PathBuf::from);
        let program_files_x86 = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from);
        return install_roots_from(
            local.as_deref(),
            program_files.as_deref(),
            program_files_x86.as_deref(),
        );
    }
    #[cfg(not(windows))]
    {
        // No Win32 install roots exist outside Windows. The call keeps the
        // shared helper exercised in every build instead of behind cfg(test).
        install_roots_from(None, None, None)
    }
}

/// First `ludex.exe` directly inside `root`, case-insensitively.
#[must_use]
pub fn find_ludex_exe(root: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().eq_ignore_ascii_case("ludex.exe") {
            continue;
        }
        let path = entry.path();
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Build commit stored inside an executable, if it carries the marker.
///
/// Only the first 96 MiB are read. The search walks every occurrence of the
/// prefix: the prefix literal itself is part of this binary, so a bare
/// occurrence followed by unrelated bytes must not hide the real marker (see
/// [`stamp_value`]).
#[must_use]
pub fn read_build_stamp(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(STAMP_SCAN_LIMIT).read_to_end(&mut bytes).ok()?;
    find_build_stamp(&bytes)
}

fn find_build_stamp(bytes: &[u8]) -> Option<String> {
    let marker = BUILD_STAMP_PREFIX.as_bytes();
    let mut from = 0usize;
    while let Some(relative) = find_subslice(&bytes[from..], marker) {
        let start = from + relative;
        let value_start = start + marker.len();
        if let Some(commit) = stamp_value(&bytes[value_start..]) {
            return Some(commit);
        }
        from = start + 1;
    }
    None
}

/// Read the value of one marker occurrence.
///
/// Commits are hexadecimal (`git rev-parse` / the CI SHA) or the literal
/// `unknown`; anything else is a stray prefix, not a build stamp.
fn stamp_value(tail: &[u8]) -> Option<String> {
    let end = tail
        .iter()
        .position(|byte| !is_stamp_byte(*byte))
        .unwrap_or(tail.len());
    if end == 0 {
        return None;
    }
    let value = std::str::from_utf8(&tail[..end]).ok()?;
    if value.eq_ignore_ascii_case("unknown") {
        return Some(value.to_string());
    }
    let hex = end <= 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
    hex.then(|| value.to_string())
}

fn is_stamp_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Scan every root for a `ludex.exe`, recording metadata and build commit.
#[must_use]
pub fn scan_install_roots(roots: &[PathBuf], running_commit: &str) -> Vec<InstallCandidate> {
    let mut found = Vec::with_capacity(roots.len());
    for root in roots {
        let Some(exe) = find_ludex_exe(root) else {
            continue;
        };
        let metadata = fs::metadata(&exe).ok();
        let modified_at = metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok())
            .map(|time| {
                let time: chrono::DateTime<chrono::Utc> = time.into();
                time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            });
        let commit = read_build_stamp(&exe);
        found.push(InstallCandidate {
            path: exe.to_string_lossy().to_string(),
            root: root.to_string_lossy().to_string(),
            size_bytes: metadata.as_ref().map(std::fs::Metadata::len),
            modified_at,
            differs: commit.as_deref() != Some(running_commit),
            commit,
        });
    }
    found
}

/// Candidates that are not the running executable.
#[must_use]
pub fn other_installs(
    candidates: Vec<InstallCandidate>,
    running_exe: Option<&Path>,
) -> Vec<InstallCandidate> {
    let running = running_exe.map(canonical_or_self);
    candidates
        .into_iter()
        .filter(|candidate| {
            let path = canonical_or_self(Path::new(&candidate.path));
            running
                .as_ref()
                .is_none_or(|running| !same_path(running, &path))
        })
        .collect()
}

fn canonical_or_self(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || left
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
}

/// Version without a leading `v` and surrounding whitespace.
#[must_use]
pub fn normalize_version(value: &str) -> &str {
    value.trim().trim_start_matches('v')
}

/// True when both strings name the same version (`v0.4.1` == `0.4.1`).
#[must_use]
pub fn same_version(left: &str, right: &str) -> bool {
    let left = normalize_version(left);
    let right = normalize_version(right);
    !left.is_empty() && left.eq_ignore_ascii_case(right)
}

/// Compare the version recorded by the updater with the running one, and keep
/// the state consistent:
///
/// - a pending version resolves to [`PendingOutcome::Verified`] or a stored
///   [`InstallMismatch`];
/// - an old mismatch is dropped once the running version moved on, so a later
///   successful install clears the warning without a dismiss button.
#[must_use]
pub fn resolve_pending(
    state: &mut InstallState,
    running_version: &str,
    exe: Option<&str>,
    now: &str,
) -> PendingOutcome {
    if let Some(previous) = state.last_mismatch.as_ref() {
        if !same_version(running_version, &previous.running_version) {
            state.last_mismatch = None;
        }
    }
    let Some(expected) = state
        .pending_version
        .take()
        .filter(|version| !normalize_version(version).is_empty())
    else {
        return PendingOutcome::None;
    };
    state.pending_at = None;
    if same_version(running_version, &expected) {
        return PendingOutcome::Verified { version: expected };
    }
    let mismatch = InstallMismatch {
        running_version: running_version.to_string(),
        expected_version: expected,
        exe: exe.map(str::to_string),
        detected_at: now.to_string(),
    };
    state.last_mismatch = Some(mismatch.clone());
    PendingOutcome::Mismatch(mismatch)
}

/* ------------------------------- persistence ----------------------------- */

/// Path of the install-state file inside the app data folder.
#[must_use]
pub fn state_path(app_data: &Path) -> PathBuf {
    app_data.join(STATE_FILE)
}

/// Read the install state; a missing or invalid file means "nothing recorded".
#[must_use]
pub fn load_state(app_data: &Path) -> InstallState {
    fs::read_to_string(state_path(app_data))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Atomic write: temp file in the same folder, then rename.
pub fn save_state(app_data: &Path, state: &InstallState) -> Result<(), String> {
    fs::create_dir_all(app_data).map_err(|error| {
        format!(
            "Could not create the app data folder {}: {error}",
            app_data.display()
        )
    })?;
    let path = state_path(app_data);
    let temporary = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(state)
        .map_err(|error| format!("Could not encode the install state: {error}"))?;
    fs::write(&temporary, body).map_err(|error| {
        format!(
            "Could not write the install state {}: {error}",
            temporary.display()
        )
    })?;
    fs::rename(&temporary, &path).map_err(|error| {
        format!(
            "Could not replace the install state {}: {error}",
            path.display()
        )
    })
}

/// Record the version the updater is about to install (#186).
///
/// Called before the download starts: on Windows the plugin exits the process
/// as soon as the installer is handed to `ShellExecuteW`, so this is the last
/// reliable point to persist the expectation.
pub fn record_pending_update(app: &AppHandle, version: &str) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let mut install_state = load_state(&state.app_data);
    install_state.pending_version = Some(normalize_version(version).to_string());
    install_state.pending_at = Some(now_rfc3339());
    if let Err(error) = save_state(&state.app_data, &install_state) {
        tracing::warn!(error = %error, "could not record the pending update version");
    }
}

/// Drop the pending version after a failed download or install.
pub fn clear_pending_update(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let mut install_state = load_state(&state.app_data);
    if install_state.pending_version.take().is_none() {
        return;
    }
    install_state.pending_at = None;
    if let Err(error) = save_state(&state.app_data, &install_state) {
        tracing::warn!(error = %error, "could not clear the pending update version");
    }
}

/* ------------------------------ app wiring ------------------------------- */

/// Snapshot of the installed binary and the update bookkeeping.
#[must_use]
pub fn collect(app: &AppHandle) -> InstallHealth {
    let app_data = app
        .try_state::<AppState>()
        .map(|state| state.app_data.clone());
    let running_exe = std::env::current_exe().ok();
    let running_version = app.package_info().version.to_string();
    // Keep the marker bytes in the binary: a stale copy is only identifiable
    // if its own file contains this literal. `black_box` stops LTO/strip from
    // dropping the unused string.
    let _ = std::hint::black_box(BUILD_STAMP);

    let roots = platform_install_roots();
    // The roots and the running commit do not change during one process, and
    // both the startup check and the UI command ask for the same scan.
    static CANDIDATES: std::sync::OnceLock<Vec<InstallCandidate>> = std::sync::OnceLock::new();
    let candidates = CANDIDATES
        .get_or_init(|| scan_install_roots(&roots, BUILD_COMMIT))
        .clone();
    let duplicates = other_installs(candidates, running_exe.as_deref());
    let install_state = app_data.as_deref().map(load_state).unwrap_or_default();

    InstallHealth {
        running_exe: running_exe.map(|path| path.to_string_lossy().to_string()),
        running_version,
        build_commit: BUILD_COMMIT.to_string(),
        platform: std::env::consts::OS.to_string(),
        log_path: app_data
            .as_deref()
            .map(|dir| dir.join(LOG_FILE).to_string_lossy().to_string()),
        install_roots: roots
            .iter()
            .map(|root| root.to_string_lossy().to_string())
            .collect(),
        duplicates,
        pending_version: install_state.pending_version,
        mismatch: install_state.last_mismatch,
    }
}

/// Startup self-check: resolve the pending update, scan for duplicates and
/// append one block to `{appDataDir}/logs/ludex.log`.
///
/// Blocking filesystem work; call it from a blocking task, never from the
/// window setup path.
pub fn startup_check(app: &AppHandle) -> InstallHealth {
    let mut health = collect(app);
    let Some(state) = app.try_state::<AppState>() else {
        return health;
    };
    let mut install_state = load_state(&state.app_data);
    let outcome = resolve_pending(
        &mut install_state,
        &health.running_version,
        health.running_exe.as_deref(),
        &now_rfc3339(),
    );
    if let Err(error) = save_state(&state.app_data, &install_state) {
        tracing::warn!(error = %error, "could not save the install state");
    }
    health.pending_version = install_state.pending_version.clone();
    health.mismatch = install_state
        .last_mismatch
        .clone()
        .filter(|mismatch| same_version(&health.running_version, &mismatch.running_version));

    tracing::info!(
        exe = health.running_exe.as_deref(),
        version = %health.running_version,
        commit = %health.build_commit,
        duplicates = health.duplicates.len(),
        "install self-check"
    );
    for duplicate in &health.duplicates {
        tracing::warn!(
            path = %duplicate.path,
            commit = duplicate.commit.as_deref(),
            differs = duplicate.differs,
            "another ludex.exe exists in an install root"
        );
    }
    match &outcome {
        PendingOutcome::Verified { version } => {
            tracing::info!(version = %version, "the installed update is running");
        }
        PendingOutcome::Mismatch(mismatch) => {
            tracing::error!(
                running = %mismatch.running_version,
                expected = %mismatch.expected_version,
                exe = mismatch.exe.as_deref(),
                "the update did not replace the running executable"
            );
        }
        PendingOutcome::None => {}
    }

    if let Some(app_data) = app
        .try_state::<AppState>()
        .map(|state| state.app_data.clone())
    {
        write_startup_log(&app_data, &health, &outcome);
    }
    health
}

/// Append the startup block. Best effort: a log that cannot be written never
/// blocks the app.
fn write_startup_log(app_data: &Path, health: &InstallHealth, outcome: &PendingOutcome) {
    let directory = app_data.join("logs");
    if fs::create_dir_all(&directory).is_err() {
        return;
    }
    let path = directory.join("ludex.log");
    if fs::metadata(&path)
        .map(|metadata| metadata.len() > MAX_LOG_BYTES)
        .unwrap_or(false)
    {
        let _ = fs::remove_file(&path);
    }
    let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let now = now_rfc3339();
    let mut lines: Vec<String> = Vec::with_capacity(4 + health.duplicates.len());
    lines.push(format!(
        "{now} INFO ludex version={} commit={} platform={} exe=\"{}\"",
        health.running_version,
        health.build_commit,
        health.platform,
        health.running_exe.as_deref().unwrap_or("unknown")
    ));
    if !health.install_roots.is_empty() {
        lines.push(format!(
            "{now} INFO scanned install roots: {}",
            health.install_roots.join(", ")
        ));
    }
    for duplicate in &health.duplicates {
        lines.push(format!(
            "{now} WARN other install: path=\"{}\" commit={} differs={} size={} modified={}",
            duplicate.path,
            duplicate.commit.as_deref().unwrap_or("unknown"),
            duplicate.differs,
            duplicate
                .size_bytes
                .map_or_else(|| "unknown".to_string(), |size| size.to_string()),
            duplicate.modified_at.as_deref().unwrap_or("unknown")
        ));
    }
    match outcome {
        PendingOutcome::Verified { version } => {
            lines.push(format!(
                "{now} INFO update verified: running version {version} matches the installed one"
            ));
        }
        PendingOutcome::Mismatch(mismatch) => {
            lines.push(format!(
                "{now} ERROR update mismatch: running={} expected={} exe=\"{}\"",
                mismatch.running_version,
                mismatch.expected_version,
                mismatch.exe.as_deref().unwrap_or("unknown")
            ));
        }
        PendingOutcome::None => {}
    }
    for line in lines {
        let _ = writeln!(file, "{line}");
    }
}

/* ------------------------------ commands --------------------------------- */

/// Where Ludex runs from, which build it is, and whether another copy exists.
///
/// The update center and Settings call this after a restart: a stored
/// `mismatch` means the updater installed a version that is not running, and
/// `duplicates` names copies that may be started instead of the updated one.
#[tauri::command]
pub fn update_install_health(app: AppHandle) -> InstallHealth {
    collect(&app)
}

/// Clear the stored mismatch after the user acted on it.
///
/// A later successful update clears it automatically; this is the explicit
/// escape hatch (for example after reinstalling the same version by hand).
#[tauri::command]
pub fn dismiss_update_mismatch(app: AppHandle) -> Result<InstallHealth, String> {
    let state = app.state::<AppState>();
    let mut install_state = load_state(&state.app_data);
    if install_state.last_mismatch.take().is_some() {
        save_state(&state.app_data, &install_state)?;
        tracing::info!("update mismatch dismissed");
    }
    Ok(collect(&app))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "ludex-install-health-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn candidate(path: &Path, root: &Path, commit: Option<&str>) -> InstallCandidate {
        InstallCandidate {
            path: path.to_string_lossy().to_string(),
            root: root.to_string_lossy().to_string(),
            size_bytes: None,
            modified_at: None,
            commit: commit.map(str::to_string),
            differs: commit != Some(BUILD_COMMIT),
        }
    }

    #[test]
    fn install_roots_are_ordered_and_deduplicated() {
        let local = PathBuf::from("/local");
        let programs = PathBuf::from("/programs");
        let roots = install_roots_from(Some(&local), Some(&programs), Some(&programs));
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/local/Ludex"),
                PathBuf::from("/local/Programs/Ludex"),
                PathBuf::from("/programs/Ludex"),
            ]
        );
        // Windows paths are case-insensitive: a differently cased duplicate is
        // still the same root.
        let mixed = install_roots_from(
            Some(&local),
            Some(Path::new("/PROGRAMS")),
            Some(Path::new("/programs")),
        );
        assert_eq!(mixed.len(), 3);
    }

    #[test]
    fn install_roots_tolerate_missing_environment() {
        assert!(install_roots_from(None, None, None).is_empty());
        let roots = install_roots_from(Some(Path::new("/local")), None, None);
        assert_eq!(roots.len(), 2);
    }

    #[test]
    fn build_marker_can_be_read_back() {
        let root = temp_root("marker");
        fs::create_dir_all(&root).unwrap();
        let exe = root.join("ludex.exe");
        fs::write(
            &exe,
            b"\x7fELF pad pad LUDEX_BUILD_COMMIT=0123456789abcdef more bytes",
        )
        .unwrap();
        assert_eq!(read_build_stamp(&exe).as_deref(), Some("0123456789abcdef"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn build_marker_in_a_large_file_is_found() {
        let root = temp_root("large");
        fs::create_dir_all(&root).unwrap();
        let exe = root.join("ludex.exe");
        let mut bytes = vec![b'x'; 4 * 1024 * 1024];
        bytes.extend_from_slice(BUILD_STAMP_PREFIX.as_bytes());
        bytes.extend_from_slice(b"fedcba987654");
        fs::write(&exe, bytes).unwrap();
        assert_eq!(read_build_stamp(&exe).as_deref(), Some("fedcba987654"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_stray_prefix_does_not_hide_the_real_marker() {
        // The prefix literal is part of every Ludex binary (this module uses
        // it), usually followed by unrelated bytes. The scan must skip it and
        // keep looking instead of reporting "no marker".
        let root = temp_root("stray");
        fs::create_dir_all(&root).unwrap();
        let exe = root.join("ludex.exe");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(BUILD_STAMP_PREFIX.as_bytes());
        bytes.extend_from_slice(b"\x00\x00 an unrelated string ");
        bytes.extend_from_slice(BUILD_STAMP_PREFIX.as_bytes());
        bytes.extend_from_slice(b"0123456789abcdef\n");
        fs::write(&exe, bytes).unwrap();
        assert_eq!(read_build_stamp(&exe).as_deref(), Some("0123456789abcdef"));
        // Non-hex garbage right after the only prefix is not a build stamp.
        fs::write(&exe, b"LUDEX_BUILD_COMMIT=not a commit").unwrap();
        assert_eq!(read_build_stamp(&exe), None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn files_without_the_marker_are_not_mistaken_for_builds() {
        let root = temp_root("nomarker");
        fs::create_dir_all(&root).unwrap();
        let exe = root.join("ludex.exe");
        fs::write(&exe, b"LUDEX_BUILD_COMMIT= (empty value)").unwrap();
        assert_eq!(read_build_stamp(&exe), None);
        fs::write(&exe, b"an old build without the marker").unwrap();
        assert_eq!(read_build_stamp(&exe), None);
        assert_eq!(read_build_stamp(&root.join("missing.exe")), None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scanning_finds_the_exe_case_insensitively() {
        let root = temp_root("scan");
        let first = root.join("one");
        let second = root.join("two");
        let empty = root.join("empty");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::create_dir_all(&empty).unwrap();
        fs::write(first.join("Ludex.EXE"), b"x LUDEX_BUILD_COMMIT=aaaa").unwrap();
        fs::write(second.join("ludex.exe"), b"x LUDEX_BUILD_COMMIT=bbbb").unwrap();

        let candidates = scan_install_roots(
            &[first.clone(), empty.clone(), root.join("missing")],
            "aaaa",
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].commit.as_deref(), Some("aaaa"));
        assert!(!candidates[0].differs);
        assert_eq!(candidates[0].size_bytes, Some(25));

        let candidates = scan_install_roots(&[first, second], "aaaa");
        assert_eq!(candidates.len(), 2);
        assert!(candidates[1].differs);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn other_installs_ignore_the_running_executable() {
        let root = temp_root("duplicates");
        fs::create_dir_all(&root).unwrap();
        let running = root.join("ludex.exe");
        let other = root.join("other.exe");
        fs::write(&running, b"running").unwrap();
        fs::write(&other, b"other").unwrap();

        let duplicates = other_installs(
            vec![
                candidate(&running, &root, None),
                candidate(&other, &root, Some("deadbeef")),
            ],
            Some(&running),
        );
        assert_eq!(duplicates.len(), 1);
        assert_eq!(duplicates[0].path, other.to_string_lossy());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn version_comparison_ignores_a_leading_v_and_spacing() {
        assert!(same_version("0.4.1", "0.4.1"));
        assert!(same_version("v0.4.1", "0.4.1"));
        assert!(same_version(" 0.4.1 ", "v0.4.1"));
        assert!(same_version("0.4.1-edge.7", "v0.4.1-edge.7"));
        // A prerelease is not the release it is based on.
        assert!(!same_version("0.4.1", "0.4.1-edge.7"));
        assert!(!same_version("0.4.1", "0.4.2"));
        assert!(!same_version("", ""));
        assert!(!same_version("  ", "0.4.1"));
    }

    #[test]
    fn pending_update_resolves_to_verified() {
        let mut state = InstallState {
            pending_version: Some("v0.4.2".to_string()),
            pending_at: Some("2026-01-01T00:00:00Z".to_string()),
            last_mismatch: None,
        };
        let outcome = resolve_pending(
            &mut state,
            "0.4.2",
            Some("C:\\Users\\me\\AppData\\Local\\Ludex\\ludex.exe"),
            "2026-01-02T00:00:00Z",
        );
        assert_eq!(
            outcome,
            PendingOutcome::Verified {
                version: "v0.4.2".to_string()
            }
        );
        assert_eq!(state.pending_version, None);
        assert_eq!(state.pending_at, None);
        assert_eq!(state.last_mismatch, None);
    }

    #[test]
    fn pending_update_resolves_to_a_typed_mismatch() {
        let mut state = InstallState {
            pending_version: Some("0.4.2".to_string()),
            pending_at: Some("2026-01-01T00:00:00Z".to_string()),
            last_mismatch: None,
        };
        let mismatch = match resolve_pending(
            &mut state,
            "0.4.1",
            Some("C:\\Program Files\\Ludex\\ludex.exe"),
            "2026-01-02T00:00:00Z",
        ) {
            PendingOutcome::Mismatch(mismatch) => mismatch,
            other => panic!("expected a mismatch, got {other:?}"),
        };
        assert_eq!(mismatch.running_version, "0.4.1");
        assert_eq!(mismatch.expected_version, "0.4.2");
        assert_eq!(
            mismatch.exe.as_deref(),
            Some("C:\\Program Files\\Ludex\\ludex.exe")
        );
        // The mismatch stays recorded so the UI can report it after the start.
        assert_eq!(state.last_mismatch.as_ref(), Some(&mismatch));
        assert_eq!(state.pending_version, None);
    }

    #[test]
    fn a_newer_running_build_drops_an_old_mismatch() {
        let mut state = InstallState {
            pending_version: None,
            pending_at: None,
            last_mismatch: Some(InstallMismatch {
                running_version: "0.4.1".to_string(),
                expected_version: "0.4.2".to_string(),
                exe: None,
                detected_at: "2026-01-02T00:00:00Z".to_string(),
            }),
        };
        // Still the stale version: the warning stays.
        assert_eq!(
            resolve_pending(&mut state, "0.4.1", None, "2026-01-03T00:00:00Z"),
            PendingOutcome::None
        );
        assert!(state.last_mismatch.is_some());
        // A manual install of a different version resolves it.
        assert_eq!(
            resolve_pending(&mut state, "0.4.2", None, "2026-01-03T00:00:00Z"),
            PendingOutcome::None
        );
        assert_eq!(state.last_mismatch, None);
    }

    #[test]
    fn blank_pending_versions_are_ignored() {
        let mut state = InstallState {
            pending_version: Some("  ".to_string()),
            pending_at: Some("2026-01-01T00:00:00Z".to_string()),
            last_mismatch: None,
        };
        assert_eq!(
            resolve_pending(&mut state, "0.4.1", None, "2026-01-02T00:00:00Z"),
            PendingOutcome::None
        );
        assert_eq!(state.pending_version, None);
        assert_eq!(state.last_mismatch, None);
    }

    #[test]
    fn install_state_round_trips() {
        let root = temp_root("state");
        let state = InstallState {
            pending_version: Some("0.4.2".to_string()),
            pending_at: Some("2026-01-01T00:00:00Z".to_string()),
            last_mismatch: Some(InstallMismatch {
                running_version: "0.4.1".to_string(),
                expected_version: "0.4.2".to_string(),
                exe: Some("C:\\Ludex\\ludex.exe".to_string()),
                detected_at: "2026-01-02T00:00:00Z".to_string(),
            }),
        };
        save_state(&root, &state).unwrap();
        assert_eq!(load_state(&root), state);
        let raw = fs::read_to_string(state_path(&root)).unwrap();
        assert!(raw.contains("\"pendingVersion\""));
        assert!(raw.contains("\"lastMismatch\""));
        let _ = fs::remove_dir_all(&root);
    }
}
