//! Silent installer automation for games shipped behind a `setup.exe` (#7).
//!
//! Many DRM-free releases are archives of an installer instead of the game
//! itself. Ludex can run that installer with the usual silent switches, watch
//! its exit code and timeout, then record the game executable it produced.
//!
//! The process handling is split behind [`Runner`] so the attempt selection,
//! exit-code policy, timeout and cancellation are unit tested on Linux without
//! executing anything. [`SystemRunner`] is the only part that spawns a process
//! and the `run_installer` command refuses to use it outside Windows.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

/// Silent switch sets tried in order when the user did not configure any.
///
/// Covers NSIS (`/S`, most DRM-free releases), Inno Setup (`/SILENT`,
/// `/VERYSILENT`), InstallShield and Squirrel/electron-builder (`--silent`).
pub fn default_attempts() -> Vec<Vec<String>> {
    vec![
        vec!["/S".to_string()],
        vec!["/silent".to_string()],
        vec![
            "/VERYSILENT".to_string(),
            "/SUPPRESSMSGBOXES".to_string(),
            "/NORESTART".to_string(),
        ],
        vec!["--silent".to_string()],
    ]
}

/// Normalise a configured attempt list: trim, drop blanks, remove duplicates.
/// An empty result falls back to [`default_attempts`].
pub fn plan_attempts(configured: &[Vec<String>]) -> Vec<Vec<String>> {
    let mut attempts: Vec<Vec<String>> = Vec::new();
    for set in configured {
        let cleaned: Vec<String> = set
            .iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect();
        if cleaned.is_empty() || attempts.contains(&cleaned) {
            continue;
        }
        attempts.push(cleaned);
    }
    if attempts.is_empty() {
        return default_attempts();
    }
    attempts
}

/// Exit codes that count as "the installer did its job". 3010 means the
/// install succeeded and Windows wants a reboot (Inno/InstallShield/MSI).
pub fn exit_code_is_success(code: Option<i32>) -> bool {
    matches!(code, Some(0) | Some(3010))
}

/// One installer invocation, independent of the platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub executable: PathBuf,
    /// Arguments that always come first: the installer path itself, or
    /// `run <installer>` for a Proton script. Silent switches follow.
    pub pre_args: Vec<String>,
    pub args: Vec<String>,
    pub working_dir: PathBuf,
    /// Extra environment for a Linux compatibility launch (`WINEPREFIX`, …).
    pub env: Vec<(String, String)>,
    /// Capture stdout/stderr to this file instead of discarding them.
    pub log_path: Option<PathBuf>,
    /// The user chose to run setup.exe interactively and watch it. An
    /// interactive installer keeps its own console window (#181); a silent one
    /// runs hidden. Set explicitly by `run_installer`, never inferred.
    pub interactive: bool,
}

impl Invocation {
    pub fn direct(executable: PathBuf, working_dir: PathBuf) -> Self {
        Self {
            executable,
            pre_args: Vec::new(),
            args: Vec::new(),
            working_dir,
            env: Vec::new(),
            log_path: None,
            // Silent by default: automation never shows a window unless the
            // caller opts in.
            interactive: false,
        }
    }
}

/// Wrap a UMU/Proton/Wine launch plan as an installer invocation.
pub fn compat_invocation(
    plan: &crate::linux_gaming::LaunchPlan,
    working_dir: &Path,
    log_path: Option<PathBuf>,
) -> Invocation {
    Invocation {
        executable: plan.program.clone(),
        pre_args: plan.args.clone(),
        args: Vec::new(),
        working_dir: working_dir.to_path_buf(),
        env: plan.env_pairs(),
        log_path,
        // Linux has no console-window concept; the game installer runs through
        // the compatibility tool regardless.
        interactive: false,
    }
}

/// Outcome of one attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub exit_code: Option<i32>,
    /// The process was killed because it exceeded the attempt timeout.
    pub timed_out: bool,
    /// The process was killed because the user cancelled the run.
    pub cancelled: bool,
}

/// One reported attempt, serialised for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptReport {
    pub args: Vec<String>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// Result of running every attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanReport {
    pub success: bool,
    pub cancelled: bool,
    pub attempts: Vec<AttemptReport>,
}

impl PlanReport {
    /// Exit code of the last attempt that actually ran.
    pub fn last_exit_code(&self) -> Option<i32> {
        self.attempts
            .iter()
            .rev()
            .find_map(|attempt| attempt.exit_code)
    }

    pub fn timed_out(&self) -> bool {
        self.attempts.iter().any(|attempt| attempt.timed_out)
    }
}

/// Abstraction over "start a process and wait for it", injectable in tests.
pub trait Runner: Send + Sync {
    fn run(
        &self,
        invocation: &Invocation,
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> Result<RunOutcome, String>;
}

/// Real process runner. `timeout` of zero waits forever (interactive runs).
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(
        &self,
        invocation: &Invocation,
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> Result<RunOutcome, String> {
        let mut command = Command::new(&invocation.executable);
        command
            .args(&invocation.pre_args)
            .args(&invocation.args)
            .current_dir(&invocation.working_dir)
            .envs(invocation.env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null());
        // #181: the interactive/silent split is explicit on the invocation.
        // The user-visible run keeps its own console window; every silent
        // automation attempt runs hidden.
        if !invocation.interactive {
            crate::process::hide_console(&mut command);
        }

        match &invocation.log_path {
            Some(log) => {
                if let Some(parent) = log.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(log)
                    .map_err(|error| {
                        format!(
                            "Could not create the installer log {}: {error}",
                            log.display()
                        )
                    })?;
                let errors = file
                    .try_clone()
                    .map_err(|error| format!("Could not open the installer log: {error}"))?;
                command
                    .stdout(Stdio::from(file))
                    .stderr(Stdio::from(errors));
            }
            None => {
                command.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }

        let mut child = command.spawn().map_err(|error| {
            format!(
                "Could not start {}: {error}",
                invocation.executable.display()
            )
        })?;

        let started = Instant::now();
        loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(RunOutcome {
                    exit_code: None,
                    timed_out: false,
                    cancelled: true,
                });
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Ok(RunOutcome {
                        exit_code: status.code(),
                        timed_out: false,
                        cancelled: false,
                    })
                }
                Ok(None) => {}
                Err(error) => return Err(format!("Could not wait for the installer: {error}")),
            }
            if !timeout.is_zero() && started.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(RunOutcome {
                    exit_code: None,
                    timed_out: true,
                    cancelled: false,
                });
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// Run the attempts in order until one succeeds, the user cancels, or a
/// process cannot even be started.
pub fn run_plan(
    runner: &dyn Runner,
    base: &Invocation,
    attempts: &[Vec<String>],
    timeout: Duration,
    cancel: &AtomicBool,
) -> PlanReport {
    let mut report = PlanReport {
        success: false,
        cancelled: false,
        attempts: Vec::new(),
    };

    for args in attempts {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }

        let invocation = Invocation {
            executable: base.executable.clone(),
            pre_args: base.pre_args.clone(),
            args: args.clone(),
            working_dir: base.working_dir.clone(),
            env: base.env.clone(),
            log_path: base.log_path.clone(),
            interactive: base.interactive,
        };
        let started = Instant::now();
        let outcome = runner.run(&invocation, timeout, cancel);
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

        match outcome {
            Ok(outcome) => {
                let success = !outcome.timed_out
                    && !outcome.cancelled
                    && exit_code_is_success(outcome.exit_code);
                report.attempts.push(AttemptReport {
                    args: args.clone(),
                    exit_code: outcome.exit_code,
                    timed_out: outcome.timed_out,
                    cancelled: outcome.cancelled,
                    error: None,
                    duration_ms,
                });
                if outcome.cancelled {
                    report.cancelled = true;
                    break;
                }
                if outcome.timed_out {
                    // Try the next switch set: some installers ignore an
                    // option they do not know and wait for input.
                    continue;
                }
                if success {
                    report.success = true;
                    break;
                }
            }
            Err(error) => {
                // The executable itself is wrong or not runnable; other switch
                // sets cannot fix that.
                report.attempts.push(AttemptReport {
                    args: args.clone(),
                    exit_code: None,
                    timed_out: false,
                    cancelled: false,
                    error: Some(error),
                    duration_ms,
                });
                break;
            }
        }
    }

    report
}

/// True when a file name looks like an installer rather than the game.
pub fn is_installer_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if !lower.ends_with(".exe") {
        return false;
    }
    let stem = lower.trim_end_matches(".exe");
    if stem.starts_with("unins") || stem.contains("uninstall") {
        return false;
    }
    stem.contains("setup") || stem.contains("install") || stem == "setup" || stem.contains("redist")
}

/// Installer-looking executables inside an install folder, relative to it.
///
/// Scans the root and one level of subdirectories, sorted and capped so the UI
/// can offer a short list.
pub fn detect_installers(root: &Path) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut push = |path: &Path| {
        if found.len() >= 20 {
            return;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        if !is_installer_name(name) {
            return;
        }
        if let Ok(relative) = path.strip_prefix(root) {
            found.push(relative.to_string_lossy().replace('\\', "/"));
        }
    };

    let Ok(entries) = std::fs::read_dir(root) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Ok(children) = std::fs::read_dir(&path) {
                for child in children.flatten() {
                    push(&child.path());
                }
            }
            continue;
        }
        push(&path);
    }
    found.sort();
    found
}

/// Resolve an installer executable inside `install_root`, refusing escapes.
pub fn resolve_installer(install_root: &Path, requested: &str) -> Result<PathBuf, String> {
    let candidate = PathBuf::from(requested);
    let full = if candidate.is_absolute() {
        candidate
    } else {
        install_root.join(candidate)
    };
    if !full.is_file() {
        return Err(format!("The installer does not exist: {}", full.display()));
    }
    if !crate::paths::is_inside(install_root, &full) {
        return Err(format!(
            "Refusing to run {} because it is outside the install folder.",
            full.display()
        ));
    }
    Ok(full)
}

/// Find the game binary an installer left behind.
///
/// The install folder is checked first (native installers), then the game's
/// Wine prefix (`drive_c`): a Windows setup program installs under
/// `Program Files` inside the prefix, not in the folder holding `setup.exe`.
/// System directories and installers/uninstallers are skipped.
pub fn detect_installed_executable(
    install_root: &Path,
    prefix: Option<&Path>,
    title: &str,
) -> Option<PathBuf> {
    if let Some(found) = crate::bridge::pick_executable(install_root, title) {
        return Some(found);
    }
    scan_prefix_programs(prefix?, title).into_iter().next()
}

/// Programs inside a Wine prefix, best first (title match, then path).
fn scan_prefix_programs(prefix: &Path, title: &str) -> Vec<PathBuf> {
    let drive = prefix.join("drive_c");
    if !drive.is_dir() {
        return Vec::new();
    }

    let wanted = crate::steam::normalize_title(title);
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(drive.clone(), 0)];
    let mut visited = 0usize;

    while let Some((directory, depth)) = stack.pop() {
        if depth > 6 || visited > 20_000 || candidates.len() >= 200 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                // Windows system folders never hold the game binary.
                if depth == 0 && matches!(name.as_str(), "windows" | "programdata") {
                    continue;
                }
                stack.push((path, depth + 1));
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let lower = name.to_ascii_lowercase();
            if !lower.ends_with(".exe") || is_installer_name(&name) {
                continue;
            }
            if lower.starts_with("unins") || lower.contains("uninstall") {
                continue;
            }
            candidates.push(path);
        }
    }

    candidates.sort_by(|left, right| {
        let score = |path: &PathBuf| {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(crate::steam::normalize_title)
                .unwrap_or_default();
            if !wanted.is_empty() && stem.contains(&wanted) {
                0
            } else {
                1
            }
        };
        score(left)
            .cmp(&score(right))
            .then_with(|| left.to_string_lossy().cmp(&right.to_string_lossy()))
    });
    candidates
}

/* ------------------------- repack payloads (#214) ------------------------ */

/// Folder names that only hold prerequisites, never the setup that installs
/// the game.
const PREREQUISITE_FOLDERS: &[&str] = &[
    "_commonredist",
    "redist",
    "directx",
    "dotnet",
    "vcredist",
    "support",
    "tools",
];

/// File stems that identify a prerequisite installer or an uninstaller.
const PREREQUISITE_STEMS: &[&str] = &[
    "unins",
    "uninstall",
    "vcredist",
    "vc_redist",
    "dxsetup",
    "dxwebsetup",
    "dotnet",
    "oalinst",
    "redist",
    "crashhandler",
    "unitycrashhandler",
    "ue4prereq",
    "prereq",
];

/// Directories that mark a repack payload next to `setup.exe`.
const REPACK_MARKER_DIRS: &[&str] = &["data", "_commonredist", "redist", "directx", "dotnet"];
/// Files that mark a repack payload next to `setup.exe`.
const REPACK_MARKER_FILES: &[&str] = &["unarc.dll", "isdone.dll"];
/// Extensions counted as repack payload parts (`.bin`, `.arc`, `.cab`).
const REPACK_MARKER_EXTENSIONS: &[&str] = &["bin", "arc", "cab"];

/// True when a file name is an explicit setup program (`setup.exe`,
/// `install.exe`, `*setup*.exe`, …), never a prerequisite or uninstaller.
pub fn is_setup_program(name: &str) -> bool {
    setup_score(name).is_some()
}

/// True for a redistributable or uninstaller that must never be run as the
/// game installer.
pub fn is_prerequisite_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".exe").unwrap_or(&lower);
    PREREQUISITE_STEMS
        .iter()
        .any(|prefix| stem.starts_with(prefix))
}

/// Score a setup file name, `None` when it is not a setup program.
fn setup_score(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".exe")?;
    if is_prerequisite_name(&lower) {
        return None;
    }
    if stem == "setup" {
        Some(300)
    } else if stem == "install" {
        Some(280)
    } else if stem.starts_with("install") {
        Some(240)
    } else if stem.contains("setup") {
        Some(200)
    } else if stem.contains("install") {
        Some(160)
    } else {
        None
    }
}

/// A repack payload: the folder holding `setup.exe` and its data files (#214).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepackLayout {
    /// Folder holding the setup program, relative to the install folder
    /// (`""` when the setup sits at the root).
    pub payload_dir: String,
    /// Best setup program, relative to the install folder.
    pub setup: String,
    /// Every setup program found, best first, relative to the install folder.
    pub candidates: Vec<String>,
    /// Payload markers found next to the setup program (`data/`, `unarc.dll`,
    /// `2 .bin`).
    pub markers: Vec<String>,
}

/// What the guided repack flow should do for an install (#214).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepackState {
    /// True when the install has no executable and the folder holds a setup.
    pub needed: bool,
    /// True when an executable is already recorded: the setup is not offered
    /// again automatically.
    pub ready: bool,
    /// Best setup program, relative to the install folder.
    pub setup: Option<String>,
    /// Setup programs found, best first.
    pub candidates: Vec<String>,
    /// Repack markers found next to the setup program.
    pub markers: Vec<String>,
    /// Game programs found by a post-install rescan (pre-existing installs).
    pub executable_candidates: Vec<String>,
    pub message: String,
}

/// What the guided flow records after the interactive setup exits (#214).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostInstallDecision {
    /// Executable to record without asking: only a single candidate after a
    /// successful exit.
    pub record: Option<String>,
    /// True when the UI must show the candidate list (several programs, or a
    /// failed setup that may still have written files).
    pub needs_choice: bool,
}

/// Detect a repack payload inside an extracted archive.
///
/// The archive root and its deepest single-folder wrapper are checked, so
/// `Game/setup.exe` and `Game/Repack/setup.exe` are both found. Prerequisite
/// installers and uninstallers are never returned.
pub fn detect_repack(root: &Path) -> Option<RepackLayout> {
    if !root.is_dir() {
        return None;
    }
    let payload = deepest_payload_root(root);
    let mut candidates: Vec<(u32, String)> = Vec::new();
    collect_setup_candidates(&payload, root, 0, &mut candidates);
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    let setup = candidates.first()?.1.clone();
    let payload_dir = relative_to(root, &payload);
    let markers = repack_markers(&payload);
    Some(RepackLayout {
        payload_dir,
        setup,
        candidates: candidates.into_iter().map(|(_, path)| path).collect(),
        markers,
    })
}

/// Descend through single-folder wrappers, stopping at the level that holds a
/// setup program. `Game/Repack/setup.exe` resolves to `Game/Repack`.
fn deepest_payload_root(root: &Path) -> PathBuf {
    let mut current = root.to_path_buf();
    for _ in 0..3 {
        if has_setup_file(&current) {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&current) else {
            break;
        };
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut files = 0usize;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if entry.path().is_dir() {
                if PREREQUISITE_FOLDERS
                    .iter()
                    .any(|folder| name.eq_ignore_ascii_case(folder))
                {
                    continue;
                }
                dirs.push(entry.path());
            } else {
                files += 1;
            }
        }
        if files > 0 || dirs.len() != 1 {
            break;
        }
        current = dirs.remove(0);
    }
    current
}

/// True when `dir` directly contains a setup program.
fn has_setup_file(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        path.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_setup_program)
    })
}

/// Collect setup programs in `dir` and, at the top level only, one level of
/// subdirectories. Prerequisite folders are skipped.
fn collect_setup_candidates(dir: &Path, root: &Path, depth: usize, out: &mut Vec<(u32, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if PREREQUISITE_FOLDERS
                .iter()
                .any(|folder| name.eq_ignore_ascii_case(folder))
            {
                continue;
            }
            subdirs.push(path);
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(score) = setup_score(&name) else {
            continue;
        };
        let penalty = u32::try_from(depth).unwrap_or(u32::MAX).saturating_mul(10);
        out.push((score.saturating_sub(penalty), relative_to(root, &path)));
    }
    if depth == 0 {
        subdirs.sort();
        for child in subdirs {
            collect_setup_candidates(&child, root, 1, out);
        }
    }
}

/// Payload markers next to the setup program, for the guided flow copy.
fn repack_markers(dir: &Path) -> Vec<String> {
    let mut markers: Vec<String> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return markers;
    };
    let mut counts: Vec<(&str, usize)> = REPACK_MARKER_EXTENSIONS
        .iter()
        .map(|extension| (*extension, 0usize))
        .collect();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let lower = name.to_ascii_lowercase();
        let path = entry.path();
        if path.is_dir() {
            if REPACK_MARKER_DIRS.iter().any(|marker| lower == *marker) {
                markers.push(format!("{name}/"));
            }
            continue;
        }
        if REPACK_MARKER_FILES.contains(&lower.as_str()) {
            markers.push(name);
            continue;
        }
        let extension = lower.rsplit_once('.').map(|(_, extension)| extension);
        if let Some((_, count)) = counts
            .iter_mut()
            .find(|(known, _)| Some(*known) == extension)
        {
            *count += 1;
        }
    }
    for (extension, count) in counts {
        if count > 0 {
            markers.push(format!("{count} .{extension}"));
        }
    }
    markers.sort();
    markers.truncate(8);
    markers
}

/// Relative, forward-slash path for the UI; `""` for the folder itself.
fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Ranked game programs after a setup program ran, relative to `root` when
/// they are inside it, absolute for the Wine prefix. Installers, prerequisites
/// and uninstallers never appear.
pub fn discover_installed_executables(
    root: &Path,
    prefix: Option<&Path>,
    title: &str,
    limit: usize,
) -> Vec<String> {
    let limit = limit.max(1);
    let mut found: Vec<String> = Vec::new();
    let discovery =
        crate::bridge::discover_executables(root, title, crate::bridge::DEFAULT_SCAN_DEPTH);
    for candidate in &discovery.candidates {
        push_unique(&mut found, relative_to(root, &candidate.path));
        if found.len() >= limit {
            return found;
        }
    }
    if let Some(prefix) = prefix {
        for path in scan_prefix_programs(prefix, title) {
            push_unique(&mut found, path.to_string_lossy().replace('\\', "/"));
            if found.len() >= limit {
                break;
            }
        }
    }
    found.truncate(limit);
    found
}

fn push_unique(found: &mut Vec<String>, value: String) {
    if !value.is_empty() && !found.contains(&value) {
        found.push(value);
    }
}

/// Resolve a candidate from [`discover_installed_executables`] for storage.
pub fn executable_full_path(root: &Path, candidate: &str) -> PathBuf {
    let path = Path::new(candidate);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

/// What to do after the guided setup exits (#214).
///
/// A single program is recorded only when the setup succeeded; a failed setup
/// keeps the payload and always shows the list so the user can retry or pick
/// one manually.
pub fn post_install_decision(exit_ok: bool, candidates: &[String]) -> PostInstallDecision {
    if candidates.is_empty() {
        return PostInstallDecision {
            record: None,
            needs_choice: false,
        };
    }
    if exit_ok {
        if let [only] = candidates {
            return PostInstallDecision {
                record: Some(only.clone()),
                needs_choice: false,
            };
        }
    }
    PostInstallDecision {
        record: None,
        needs_choice: true,
    }
}

/// Guided repack state for an install (#214).
///
/// An install that already has an executable is `ready` and never offers the
/// setup again. A folder with no setup and no executable is neither: the UI
/// falls back to the executable picker.
pub fn repack_state(
    root: &Path,
    prefix: Option<&Path>,
    executable: Option<&str>,
    title: &str,
) -> RepackState {
    let ready = executable.is_some_and(|value| !value.trim().is_empty());
    if ready {
        return RepackState {
            needed: false,
            ready: true,
            setup: None,
            candidates: Vec::new(),
            markers: Vec::new(),
            executable_candidates: Vec::new(),
            message: "This install already has a game executable; the setup program is not offered again."
                .to_string(),
        };
    }

    match detect_repack(root) {
        Some(layout) => RepackState {
            needed: true,
            ready: false,
            setup: Some(layout.setup),
            candidates: layout.candidates,
            markers: layout.markers,
            executable_candidates: discover_installed_executables(root, prefix, title, 8),
            message:
                "This archive contains a setup program. Run it once to install the game files."
                    .to_string(),
        },
        None => RepackState {
            needed: false,
            ready: false,
            setup: None,
            candidates: Vec::new(),
            markers: Vec::new(),
            executable_candidates: discover_installed_executables(root, prefix, title, 8),
            message: "No setup program was found in this install folder.".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockRunner {
        outcomes: Mutex<Vec<Result<RunOutcome, String>>>,
        calls: Mutex<Vec<Invocation>>,
    }

    impl MockRunner {
        fn new(outcomes: Vec<Result<RunOutcome, String>>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<Invocation> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Runner for MockRunner {
        fn run(
            &self,
            invocation: &Invocation,
            _timeout: Duration,
            _cancel: &AtomicBool,
        ) -> Result<RunOutcome, String> {
            self.calls.lock().unwrap().push(invocation.clone());
            self.outcomes.lock().unwrap().remove(0)
        }
    }

    fn base() -> Invocation {
        Invocation::direct(
            PathBuf::from("/games/hades/setup.exe"),
            PathBuf::from("/games/hades"),
        )
    }

    fn ok(code: Option<i32>) -> Result<RunOutcome, String> {
        Ok(RunOutcome {
            exit_code: code,
            timed_out: false,
            cancelled: false,
        })
    }

    #[test]
    fn default_plan_covers_the_common_installers() {
        let attempts = plan_attempts(&[]);
        assert_eq!(attempts.len(), 4);
        assert_eq!(attempts[0], vec!["/S".to_string()]);
        assert_eq!(attempts[1], vec!["/silent".to_string()]);
        assert_eq!(
            attempts[2],
            vec![
                "/VERYSILENT".to_string(),
                "/SUPPRESSMSGBOXES".to_string(),
                "/NORESTART".to_string()
            ]
        );
        assert_eq!(attempts[3], vec!["--silent".to_string()]);
    }

    #[test]
    fn configured_plan_is_cleaned_and_deduplicated() {
        let attempts = plan_attempts(&[
            vec!["  /S ".into(), "".into()],
            vec![],
            vec!["/S".into()],
            vec!["--quiet".into()],
        ]);
        assert_eq!(
            attempts,
            vec![vec!["/S".to_string()], vec!["--quiet".to_string()]]
        );
    }

    #[test]
    fn first_successful_exit_code_wins() {
        let runner = MockRunner::new(vec![ok(Some(1)), ok(Some(0)), ok(Some(0))]);
        let cancel = AtomicBool::new(false);
        let report = run_plan(
            &runner,
            &base(),
            &plan_attempts(&[]),
            Duration::from_secs(10),
            &cancel,
        );

        assert!(report.success);
        assert!(!report.cancelled);
        assert_eq!(report.attempts.len(), 2);
        assert_eq!(report.last_exit_code(), Some(0));
        assert_eq!(runner.calls().len(), 2);
        assert_eq!(runner.calls()[0].args, vec!["/S".to_string()]);
        assert_eq!(runner.calls()[1].args, vec!["/silent".to_string()]);
    }

    #[test]
    fn a_reboot_exit_code_still_counts_as_installed() {
        let runner = MockRunner::new(vec![ok(Some(3010))]);
        let cancel = AtomicBool::new(false);
        let report = run_plan(
            &runner,
            &base(),
            &plan_attempts(&[]),
            Duration::from_secs(10),
            &cancel,
        );
        assert!(report.success);
        assert_eq!(report.attempts.len(), 1);
    }

    #[test]
    fn a_timeout_moves_on_to_the_next_switch_set() {
        let runner = MockRunner::new(vec![
            Ok(RunOutcome {
                exit_code: None,
                timed_out: true,
                cancelled: false,
            }),
            ok(Some(0)),
        ]);
        let cancel = AtomicBool::new(false);
        let report = run_plan(
            &runner,
            &base(),
            &plan_attempts(&[]),
            Duration::from_secs(2),
            &cancel,
        );

        assert!(report.success);
        assert!(report.timed_out());
        assert_eq!(report.attempts.len(), 2);
        assert!(report.attempts[0].timed_out);
    }

    #[test]
    fn running_out_of_attempts_reports_failure() {
        let runner = MockRunner::new(vec![ok(Some(2)), ok(Some(1603))]);
        let cancel = AtomicBool::new(false);
        let report = run_plan(
            &runner,
            &base(),
            &[vec!["/S".into()], vec!["/silent".into()]],
            Duration::from_secs(5),
            &cancel,
        );

        assert!(!report.success);
        assert_eq!(report.attempts.len(), 2);
        assert_eq!(report.last_exit_code(), Some(1603));
    }

    #[test]
    fn cancelling_before_the_first_attempt_runs_nothing() {
        let runner = MockRunner::new(vec![ok(Some(0))]);
        let cancel = AtomicBool::new(true);
        let report = run_plan(
            &runner,
            &base(),
            &plan_attempts(&[]),
            Duration::from_secs(5),
            &cancel,
        );

        assert!(!report.success);
        assert!(report.cancelled);
        assert!(report.attempts.is_empty());
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn cancel_during_an_attempt_stops_the_plan() {
        let runner = MockRunner::new(vec![Ok(RunOutcome {
            exit_code: None,
            timed_out: false,
            cancelled: true,
        })]);
        let cancel = AtomicBool::new(false);
        let report = run_plan(
            &runner,
            &base(),
            &plan_attempts(&[]),
            Duration::from_secs(5),
            &cancel,
        );

        assert!(report.cancelled);
        assert_eq!(report.attempts.len(), 1);
    }

    #[test]
    fn a_spawn_error_stops_the_plan() {
        let runner = MockRunner::new(vec![Err("Could not start setup.exe: denied".into())]);
        let cancel = AtomicBool::new(false);
        let report = run_plan(
            &runner,
            &base(),
            &plan_attempts(&[]),
            Duration::from_secs(5),
            &cancel,
        );

        assert!(!report.success);
        assert_eq!(report.attempts.len(), 1);
        assert!(report.attempts[0].error.is_some());
    }

    #[test]
    fn interactive_run_sends_no_arguments() {
        let runner = MockRunner::new(vec![ok(Some(0))]);
        let cancel = AtomicBool::new(false);
        let report = run_plan(&runner, &base(), &[Vec::new()], Duration::ZERO, &cancel);

        assert!(report.success);
        assert_eq!(runner.calls().len(), 1);
        assert!(runner.calls()[0].args.is_empty());
    }

    #[test]
    fn the_window_decision_is_explicit_on_every_attempt() {
        // Silent automation: every invocation stays hidden (#181).
        let runner = MockRunner::new(vec![ok(Some(1)), ok(Some(0))]);
        let cancel = AtomicBool::new(false);
        let base = base();
        assert!(!base.interactive);
        run_plan(
            &runner,
            &base,
            &[vec!["/S".into()], vec!["/silent".into()]],
            Duration::from_secs(5),
            &cancel,
        );
        assert!(runner.calls().iter().all(|call| !call.interactive));

        // The user chose to watch setup.exe: the flag reaches every attempt
        // instead of being inferred from the timeout.
        let runner = MockRunner::new(vec![ok(Some(1)), ok(Some(0))]);
        let mut base = base;
        base.interactive = true;
        run_plan(
            &runner,
            &base,
            &[vec!["/S".into()], vec!["/silent".into()]],
            Duration::ZERO,
            &cancel,
        );
        assert!(runner.calls().iter().all(|call| call.interactive));
    }

    #[test]
    fn compat_runs_keep_the_installer_as_a_pre_argument() {
        let runner = MockRunner::new(vec![ok(Some(1)), ok(Some(0))]);
        let cancel = AtomicBool::new(false);
        let mut invocation = Invocation::direct(
            PathBuf::from("/usr/bin/umu-run"),
            PathBuf::from("/games/hades"),
        );
        invocation.pre_args = vec!["/games/hades/setup.exe".to_string()];
        invocation.env = vec![("WINEPREFIX".into(), "/games/hades/.prefix".into())];
        // The attempt switches are appended after the installer path, not
        // instead of it.
        let report = run_plan(
            &runner,
            &invocation,
            &[vec!["/VERYSILENT".into()], vec!["--silent".into()]],
            Duration::from_secs(5),
            &cancel,
        );

        assert!(report.success);
        let calls = runner.calls();
        assert_eq!(calls[0].pre_args, invocation.pre_args);
        assert_eq!(calls[0].args, vec!["/VERYSILENT".to_string()]);
        assert_eq!(calls[0].env, invocation.env);
        assert_eq!(calls[1].args, vec!["--silent".to_string()]);
    }

    #[test]
    fn compat_invocation_wraps_a_proton_plan() {
        use crate::linux_gaming::{self, LaunchOptions};

        let proton = crate::compat::CompatTool {
            id: "proton:/games/GE-Proton9-20".into(),
            name: "GE-Proton9-20".into(),
            kind: crate::compat::ToolKind::Proton,
            version: None,
            path: "/games/GE-Proton9-20/proton".into(),
            proton_path: Some("/games/GE-Proton9-20".into()),
            source: "compatibilitytools.d".into(),
            steam_root: None,
        };
        let selection =
            linux_gaming::select_tool(&[proton], linux_gaming::LaunchMode::Proton, "").unwrap();
        let plan = linux_gaming::build_launch(
            &selection,
            Path::new("/games/hades/setup.exe"),
            &[],
            Path::new("/games/hades/.prefix"),
            &LaunchOptions::default(),
        )
        .unwrap();
        let invocation = compat_invocation(&plan, Path::new("/games/hades"), None);
        assert_eq!(
            invocation.executable,
            PathBuf::from("/games/GE-Proton9-20/proton")
        );
        assert_eq!(
            invocation.pre_args,
            vec!["run".to_string(), "/games/hades/setup.exe".to_string()]
        );
        assert_eq!(
            invocation
                .env
                .iter()
                .find(|(key, _)| key == "STEAM_COMPAT_DATA_PATH")
                .map(|(_, value)| value.as_str()),
            Some("/games/hades/.prefix")
        );
    }

    #[test]
    fn installed_executable_is_found_inside_the_wine_prefix() {
        let root = std::env::temp_dir().join(format!(
            "ludex-installer-prefix-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        let install = root.join("hades");
        let prefix = install.join(".prefix");
        std::fs::create_dir_all(prefix.join("drive_c/Program Files/Hades")).unwrap();
        std::fs::create_dir_all(prefix.join("drive_c/windows/system32")).unwrap();
        std::fs::write(install.join("setup.exe"), b"x").unwrap();
        std::fs::write(prefix.join("drive_c/Program Files/Hades/Hades.exe"), b"x").unwrap();
        std::fs::write(prefix.join("drive_c/windows/system32/cmd.exe"), b"x").unwrap();
        std::fs::write(
            prefix.join("drive_c/Program Files/Hades/unins000.exe"),
            b"x",
        )
        .unwrap();

        let found = detect_installed_executable(&install, Some(&prefix), "Hades").unwrap();
        assert!(found.ends_with("drive_c/Program Files/Hades/Hades.exe"));
        // Without a prefix only the install folder is searched: the setup
        // program itself is never recorded as the game.
        assert!(detect_installed_executable(&install, None, "Hades").is_none());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn installer_names_are_detected() {
        assert!(is_installer_name("setup.exe"));
        assert!(is_installer_name("Game Setup.exe"));
        assert!(is_installer_name("installer_x64.exe"));
        assert!(is_installer_name("vcredist_x64.exe"));
        assert!(!is_installer_name("unins000.exe"));
        assert!(!is_installer_name("Hades.exe"));
        assert!(!is_installer_name("setup.msi"));
    }

    fn temp_repack_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ludex-repack-{tag}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn repack_detection_covers_the_common_layouts() {
        // Root-level setup with the usual payload markers.
        let root = temp_repack_root("flat");
        std::fs::write(root.join("setup.exe"), b"x").unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("setup-1.bin"), b"x").unwrap();
        let layout = detect_repack(&root).unwrap();
        assert_eq!(layout.payload_dir, "");
        assert_eq!(layout.setup, "setup.exe");
        assert_eq!(layout.candidates, vec!["setup.exe".to_string()]);
        assert!(layout.markers.iter().any(|marker| marker == "data/"));
        assert!(layout.markers.iter().any(|marker| marker == "1 .bin"));
        std::fs::remove_dir_all(&root).ok();

        // A plain game folder is not a repack.
        let plain = temp_repack_root("plain");
        std::fs::write(plain.join("Hades.exe"), b"x").unwrap();
        assert!(detect_repack(&plain).is_none());
        std::fs::remove_dir_all(&plain).ok();

        // Single top-level wrapper: the setup lives one folder down.
        let nested = temp_repack_root("nested");
        std::fs::create_dir_all(nested.join("Hades")).unwrap();
        std::fs::write(nested.join("Hades/setup.exe"), b"x").unwrap();
        std::fs::write(nested.join("Hades/game.bin"), b"x").unwrap();
        let layout = detect_repack(&nested).unwrap();
        assert_eq!(layout.payload_dir, "Hades");
        assert_eq!(layout.setup, "Hades/setup.exe");
        assert!(layout.markers.iter().any(|marker| marker == "1 .bin"));
        std::fs::remove_dir_all(&nested).ok();

        // Nested wrappers: the deepest single folder wins.
        let deep = temp_repack_root("deep");
        std::fs::create_dir_all(deep.join("Hades/Repack")).unwrap();
        std::fs::write(deep.join("Hades/Repack/setup.exe"), b"x").unwrap();
        std::fs::write(deep.join("Hades/Repack/unarc.dll"), b"x").unwrap();
        let layout = detect_repack(&deep).unwrap();
        assert_eq!(layout.payload_dir, "Hades/Repack");
        assert_eq!(layout.setup, "Hades/Repack/setup.exe");
        assert!(layout.markers.iter().any(|marker| marker == "unarc.dll"));
        std::fs::remove_dir_all(&deep).ok();

        // `install.exe` is a setup too.
        let install = temp_repack_root("install");
        std::fs::write(install.join("install.exe"), b"x").unwrap();
        assert_eq!(detect_repack(&install).unwrap().setup, "install.exe");
        std::fs::remove_dir_all(&install).ok();

        // Prerequisites only: never a repack.
        let prereq = temp_repack_root("prereq");
        std::fs::create_dir_all(prereq.join("_CommonRedist")).unwrap();
        std::fs::write(prereq.join("_CommonRedist/vcredist_x64.exe"), b"x").unwrap();
        std::fs::write(prereq.join("unins000.exe"), b"x").unwrap();
        assert!(detect_repack(&prereq).is_none());
        std::fs::remove_dir_all(&prereq).ok();
    }

    #[test]
    fn setup_candidates_are_ranked_and_prerequisites_excluded() {
        let root = temp_repack_root("ranking");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::create_dir_all(root.join("_CommonRedist")).unwrap();
        std::fs::write(root.join("setup.exe"), b"x").unwrap();
        std::fs::write(root.join("install.exe"), b"x").unwrap();
        std::fs::write(root.join("bin/install_helper.exe"), b"x").unwrap();
        std::fs::write(root.join("unins000.exe"), b"x").unwrap();
        std::fs::write(root.join("vcredist_x64.exe"), b"x").unwrap();
        std::fs::write(root.join("_CommonRedist/dxsetup.exe"), b"x").unwrap();

        let layout = detect_repack(&root).unwrap();
        assert_eq!(
            layout.candidates,
            vec![
                "setup.exe".to_string(),
                "install.exe".to_string(),
                "bin/install_helper.exe".to_string(),
            ]
        );
        assert!(!layout
            .candidates
            .iter()
            .any(|candidate| candidate.contains("vcredist")
                || candidate.contains("unins")
                || candidate.contains("dxsetup")));

        assert!(is_setup_program("Game Setup.exe"));
        assert!(is_setup_program("gogsetup.exe"));
        assert!(!is_setup_program("vcredist_x64.exe"));
        assert!(!is_setup_program("unins000.exe"));
        assert!(!is_setup_program("Hades.exe"));
        assert!(is_prerequisite_name("vcredist_x64.exe"));
        assert!(is_prerequisite_name("dxsetup.exe"));
        assert!(!is_prerequisite_name("setup.exe"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn post_install_discovery_skips_installers_and_prerequisites() {
        let root = temp_repack_root("discover");
        std::fs::create_dir_all(root.join("_CommonRedist")).unwrap();
        std::fs::create_dir_all(root.join("Tools")).unwrap();
        std::fs::write(root.join("Hades.exe"), b"x").unwrap();
        std::fs::write(root.join("setup.exe"), b"x").unwrap();
        std::fs::write(root.join("unins000.exe"), b"x").unwrap();
        std::fs::write(root.join("_CommonRedist/vcredist_x64.exe"), b"x").unwrap();
        std::fs::write(root.join("Tools/helper.exe"), b"x").unwrap();

        let found = discover_installed_executables(&root, None, "Hades", 8);
        assert_eq!(found, vec!["Hades.exe".to_string()]);
        assert_eq!(
            executable_full_path(&root, "Hades.exe"),
            root.join("Hades.exe")
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn post_install_decision_records_one_success_and_asks_otherwise() {
        let single = vec!["Hades.exe".to_string()];
        let decision = post_install_decision(true, &single);
        assert_eq!(decision.record.as_deref(), Some("Hades.exe"));
        assert!(!decision.needs_choice);

        // A failed setup keeps the payload and offers the list.
        let failed = post_install_decision(false, &single);
        assert!(failed.record.is_none());
        assert!(failed.needs_choice);

        // Several programs: the user picks one.
        let many = vec!["Hades.exe".to_string(), "HadesLauncher.exe".to_string()];
        let decision = post_install_decision(true, &many);
        assert!(decision.record.is_none());
        assert!(decision.needs_choice);

        // Nothing found: nothing to choose and nothing to record.
        let empty = post_install_decision(true, &[]);
        assert!(empty.record.is_none());
        assert!(!empty.needs_choice);
    }

    #[test]
    fn repack_state_needs_the_setup_then_is_ready_once_an_executable_is_recorded() {
        let root = temp_repack_root("state");
        std::fs::write(root.join("setup.exe"), b"x").unwrap();
        std::fs::write(root.join("Hades.exe"), b"x").unwrap();

        let pending = repack_state(&root, None, None, "Hades");
        assert!(pending.needed);
        assert!(!pending.ready);
        assert_eq!(pending.setup.as_deref(), Some("setup.exe"));
        assert!(pending
            .executable_candidates
            .iter()
            .any(|candidate| candidate == "Hades.exe"));

        // Once an executable is recorded the guided flow never re-offers it.
        let ready = repack_state(&root, None, Some("/games/Hades/Hades.exe"), "Hades");
        assert!(ready.ready);
        assert!(!ready.needed);
        assert!(ready.setup.is_none());
        assert!(ready.candidates.is_empty());

        std::fs::remove_dir_all(&root).ok();

        // Pre-existing install without a setup: no guided flow, just a rescan.
        let preexisting = temp_repack_root("preexisting");
        std::fs::write(preexisting.join("Hades.exe"), b"x").unwrap();
        let state = repack_state(&preexisting, None, None, "Hades");
        assert!(!state.needed);
        assert!(!state.ready);
        assert!(state.setup.is_none());
        assert_eq!(state.executable_candidates, vec!["Hades.exe".to_string()]);
        std::fs::remove_dir_all(&preexisting).ok();
    }

    #[test]
    fn detect_installers_scans_root_and_one_level() {
        let root = std::env::temp_dir().join(format!(
            "ludex-installer-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("setup.exe"), b"x").unwrap();
        std::fs::write(root.join("Hades.exe"), b"x").unwrap();
        std::fs::write(root.join("bin/install-game.exe"), b"x").unwrap();
        std::fs::write(root.join("bin/readme.txt"), b"x").unwrap();

        let found = detect_installers(&root);
        assert_eq!(found, vec!["bin/install-game.exe", "setup.exe"]);

        let resolved = resolve_installer(&root, "bin/install-game.exe").unwrap();
        assert!(resolved.ends_with("bin/install-game.exe"));
        assert!(resolve_installer(&root, "../outside.exe").is_err());

        let _ = std::fs::remove_dir_all(&root);
    }
}
