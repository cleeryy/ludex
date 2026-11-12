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
    let prefix = prefix?;
    let drive = prefix.join("drive_c");
    if !drive.is_dir() {
        return None;
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
    candidates.into_iter().next()
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
