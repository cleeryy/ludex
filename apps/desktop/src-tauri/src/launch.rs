//! Launching installed games, tracking play sessions and syncing playtime.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::db;
use crate::paths;
use crate::process_tree::{ProcessSampler, TreeTracker};
use crate::state::{AppState, ProcessEntry};
use crate::util::{non_empty, now_rfc3339};
use crate::window_focus::{self, FocusOutcome};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchResult {
    pub session_id: String,
    /// Compatibility tool used on Linux (UMU/Proton/Wine), when any.
    pub tool: Option<String>,
    /// How the entry is managed: `ludex`, `adopted` or a launcher id (#129).
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GameExited {
    session_id: String,
    game_id: i64,
    minutes_played: i64,
    seconds_played: i64,
    /// `adopted` for a linked folder, `ludex` for an install, a launcher id for
    /// an import (#129).
    source: String,
    /// Exit code of the root game process as an unsigned 32-bit value, when the
    /// platform reported one. Windows NTSTATUS values keep their bits
    /// (`0xC0000409`), so the renderer can print them in hex (#187).
    exit_code: Option<u32>,
    /// True when the whole process tree ended within
    /// [`IMMEDIATE_EXIT_WINDOW`]: the game crashed or bailed out instead of
    /// running (#187). The renderer shows a danger toast for this.
    immediate_exit: bool,
}

/// The focus outcome of a fresh launch (#219), emitted as `game://focus`.
///
/// Failure is data, never an error: the launch already succeeded. The shell
/// shows this only when `focused` is false, together with the Resume and Force
/// close actions the game page already offers (#191).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GameFocus {
    game_id: i64,
    session_id: String,
    /// True when the game window is now in the foreground.
    focused: bool,
    /// Platform window handle that was focused, when any.
    window: Option<String>,
    /// `noWindow` / `focusRefused` when the window could not be focused.
    reason: Option<String>,
}

/// #298: the second consecutive failure with the same profile reverted the
/// game to the last plan that worked.
///
/// One event per revert, with the two profile hashes and the sentence the
/// shell shows. The options themselves were written back to the game row, so
/// the next Play uses them; the event only explains what changed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GameRecovery {
    game_id: i64,
    title: String,
    reverted: bool,
    from_hash: String,
    to_hash: String,
    message: String,
}

impl GameFocus {
    /// The event for one focus attempt. `Focused` is the success case; both
    /// failures carry a reason and no window guarantee.
    fn new(game_id: i64, session_id: &str, outcome: &FocusOutcome) -> Self {
        let (focused, window, reason) = match outcome {
            FocusOutcome::Focused { window } => (true, Some(window.clone()), None),
            FocusOutcome::NoWindow => (false, None, Some("noWindow".to_string())),
            FocusOutcome::Refused { window } => (
                false,
                Some(window.clone()),
                Some("focusRefused".to_string()),
            ),
        };
        Self {
            game_id,
            session_id: session_id.to_string(),
            focused,
            window,
            reason,
        }
    }
}

/// Whether a successful focus hides Ludex (#219).
///
/// Hiding is what makes the game visible, but a failed focus must leave Ludex
/// on screen so the player can read the message and press Resume. A console
/// session (#172) keeps its shell: the shell *is* the streamed display.
fn should_hide_ludex(focused: bool, console_session: bool) -> bool {
    focused && !console_session
}

/// A game that ends this quickly never reached its first frame in practice
/// (#187); the shell reports it instead of silently closing the session.
pub(crate) const IMMEDIATE_EXIT_WINDOW: Duration = Duration::from_secs(5);

/// Hex form of a process exit code (`0xC0000409`), used in logs and events.
pub(crate) fn exit_code_hex(code: u32) -> String {
    format!("0x{code:08X}")
}

/// The per-game sink for the router (#289): the dedicated Audio output field,
/// then a `PULSE_SINK` environment entry written by the older tier-2 switch
/// (#299), so both surfaces route the same stream instead of fighting.
pub(crate) fn launch_audio_sink(options: &crate::linux_gaming::LaunchOptions) -> String {
    let dedicated = options.audio_sink.trim();
    if !dedicated.is_empty() {
        return dedicated.to_string();
    }
    options
        .env
        .get(crate::audio::PULSE_SINK_ENV)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// Exit code of a game that died inside the immediate-exit window (#187).
///
/// Returns `None` once the window elapsed, or when the platform did not report
/// an exit code (killed by a signal on Unix, for example): a session ended by
/// `stop_game` must not raise the "exited immediately" toast.
pub(crate) fn immediate_exit_code(elapsed: Duration, code: Option<i32>) -> Option<u32> {
    if elapsed >= IMMEDIATE_EXIT_WINDOW {
        return None;
    }
    code.map(|code| code as u32)
}

/// A short, honest explanation for the exit code of an immediate exit (#280).
///
/// Ludex only sees the wrapper's status: it cannot tell a missing DLL from a
/// broken emulator, so the hint names the layer that most likely failed and
/// points at the launch log rather than promising a cause.
pub(crate) fn immediate_exit_hint(code: u32) -> &'static str {
    match code {
        0 => "the compatibility tool exited cleanly before the game started; check the launch log",
        1 => "the environment or the compatibility tool failed before the game started; check the launch log",
        // 0x35 is 53 decimal: umu-run/Proton do not define it, so it is the
        // game or its Steam emulator (OnlineFix/StubDRM) bailing out early.
        53 => "the game or its Steam emulator aborted before the first frame; check GAMEID, the prefix and the repack notes",
        // NTSTATUS STATUS_STACK_BUFFER_OVERRUN: a Windows fail-fast crash.
        0xC0000409 => "the game crashed (Windows fail-fast / stack buffer overrun); check the launch log",
        _ => "the game exited before reaching its first frame; check the launch log",
    }
}

/// Largest `launch-*.log` kept after a compat session (#280).
const LAUNCH_LOG_MAX_BYTES: u64 = 512 * 1024;
/// How many `launch-*.log` files are kept; the run in progress is the newest.
const LAUNCH_LOG_KEEP: usize = 5;
/// File name prefix of a per-launch compatibility log (#280).
const LAUNCH_LOG_PREFIX: &str = "launch-";

/// `<appDataDir>/logs/launch-<game_id>-<session_id>.log` (#280).
///
/// Only the Linux compat spawn calls this today; the attribute keeps a
/// Windows/macOS build free of dead-code warnings.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
fn launch_log_path(app_data: &Path, game_id: i64, session_id: &str) -> PathBuf {
    app_data
        .join(crate::logging::LOG_SUBDIR)
        .join(format!("{LAUNCH_LOG_PREFIX}{game_id}-{session_id}.log"))
}

/// Keep the newest [`LAUNCH_LOG_KEEP`] launch logs and delete the rest (#280).
///
/// A pile of multi-megabyte Proton logs would fill the app data folder; the
/// five newest cover the last few launches a report needs. Failures are silent
/// by design: housekeeping must never fail a session.
fn prune_launch_logs(path: &Path) {
    let Some(dir) = path.parent() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return false;
            };
            name.starts_with(LAUNCH_LOG_PREFIX) && name.ends_with(".log")
        })
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    logs.sort_by_key(|left| std::cmp::Reverse(left.0));
    for (_, stale) in logs.into_iter().skip(LAUNCH_LOG_KEEP) {
        let _ = std::fs::remove_file(stale);
    }
}

/// Truncate a finished launch log to its last [`LAUNCH_LOG_MAX_BYTES`] (#280).
///
/// Called after the process tree exited, so no writer can append while the
/// tail is copied. The kept tail starts on a line boundary and carries a
/// marker naming the truncation.
fn trim_launch_log(path: &Path) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    if metadata.len() <= LAUNCH_LOG_MAX_BYTES {
        return;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let start = bytes.len().saturating_sub(LAUNCH_LOG_MAX_BYTES as usize);
    let tail = String::from_utf8_lossy(&bytes[start..]);
    let tail = match tail.find('\n') {
        Some(index) => &tail[index + 1..],
        None => &*tail,
    };
    let marker = format!(
        "[ludex: log truncated, kept the last {} KiB]\n",
        LAUNCH_LOG_MAX_BYTES / 1024
    );
    if let Err(error) = std::fs::write(path, format!("{marker}{tail}")) {
        tracing::warn!(
            path = %path.display(),
            error = %error,
            "could not truncate the launch log"
        );
    }
}

static SYNCING: AtomicBool = AtomicBool::new(false);

struct SyncFlag;

impl Drop for SyncFlag {
    fn drop(&mut self) {
        SYNCING.store(false, Ordering::SeqCst);
    }
}

pub async fn launch(
    app: &AppHandle,
    game_id: i64,
    executable: Option<String>,
    args: Option<String>,
) -> Result<LaunchResult, String> {
    let state = app.state::<AppState>();

    let (installed, options) = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        let installed = db::get_installed(&connection, game_id)?;
        let mut options = db::get_launch_options(&connection, game_id)?;
        if !db::has_launch_options(&connection, game_id)? {
            let default = state.settings_snapshot().default_launch_mode;
            if default != crate::linux_gaming::LaunchMode::Auto {
                options.mode = default;
            }
        }
        (installed, options)
    };
    let installed =
        installed.ok_or_else(|| format!("Game {game_id} is not installed on this device."))?;

    let stored_install_path = PathBuf::from(&installed.install_path);
    // #187: a stored verbatim path (`\\?\D:\…`) must never reach `Command::new`
    // or `current_dir`: Win32 fail-fasts (0xc0000409) on the verbatim form.
    let install_path = paths::normalize_native_path(&stored_install_path);
    let external_uri = installed
        .launch_uri
        .as_deref()
        .map(str::trim)
        .filter(|uri| !uri.is_empty());
    // Launcher imports (#116) own no files here: the client may not expose an
    // install folder at all (Xbox Start menu entries), so only a real install
    // or an adopted folder has to exist on disk.
    if external_uri.is_none() && !install_path.is_dir() {
        return Err(if installed.source == db::SOURCE_ADOPTED {
            format!(
                "The adopted folder is missing: {}. Re-link it from the game page or remove it from Ludex.",
                install_path.display()
            )
        } else {
            format!(
                "The install folder is missing: {}. Reinstall the game.",
                install_path.display()
            )
        });
    }
    // #290: the first launch of a freshly moved install verifies its files
    // before the game starts. A damaged copy stops here, with Repair offered.
    if external_uri.is_none() && install_path.is_dir() {
        crate::maintenance::verify_moved_install(app, game_id, &installed, &install_path).await?;
    }
    let prefix = match external_uri {
        Some(_) => PathBuf::new(),
        None => crate::linux_gaming::prefix_for(&install_path, &options.prefix_path)?,
    };

    // #148: a game can opt into keeping its language and player name in sync
    // with Ludex. Apply them before the process starts, without a dialog; a
    // config that cannot be written is logged and never blocks the launch.
    if external_uri.is_none() {
        apply_config_sync(app, game_id, &install_path);
    }

    {
        let processes = state
            .processes
            .lock()
            .map_err(|_| "The process list is unavailable; restart Ludex.".to_string())?;
        if processes.values().any(|entry| entry.game_id == game_id) {
            return Err("This game is already running.".to_string());
        }
    }

    let executable_path = match external_uri {
        Some(_) => None,
        None => Some(resolve_executable(
            &install_path,
            Some(&prefix),
            executable.as_deref(),
            installed.executable.as_deref(),
            &installed.title,
        )?),
    };
    // #160: adopted folders often wrap the game in an edition/version folder,
    // so a nested executable runs from its own folder (see the rule on
    // [`launch_working_directory`]). #187: an explicit per-game working
    // directory wins over that rule, and every cwd is normalised before the
    // process starts.
    let rule_working_dir = executable_path
        .as_deref()
        .map(|path| launch_working_directory(&install_path, path, &installed.source));
    let working_dir =
        working_directory_override(&options.working_dir, &install_path).or_else(|| {
            rule_working_dir
                .as_deref()
                .map(paths::normalize_native_path)
        });

    // #187: log exactly what is about to run, and whether any stored path had
    // to be normalised, including when the spawn fails below.
    let normalized = stored_install_path != install_path
        || installed
            .executable
            .as_deref()
            .is_some_and(|raw| paths::normalize_native_path(Path::new(raw)) != Path::new(raw))
        || executable
            .as_deref()
            .is_some_and(|raw| paths::normalize_native_path(Path::new(raw)) != Path::new(raw));
    match executable_path.as_deref() {
        Some(path) => tracing::info!(
            game_id,
            source = %installed.source,
            program = %path.display(),
            working_dir = %working_dir.as_deref().unwrap_or(&install_path).display(),
            normalized,
            args = args.as_deref().unwrap_or(""),
            "launching the game"
        ),
        None => tracing::info!(
            game_id,
            source = %installed.source,
            uri = installed.launch_uri.as_deref().unwrap_or(""),
            "opening the game launcher"
        ),
    }

    // #280: the session id names the compatibility log, so it exists before
    // the process is built.
    let session_id = uuid::Uuid::new_v4().to_string();
    // #297: on Linux the rules engine decides the launch. An imported launcher
    // URI has no compatibility decision, so it is skipped.
    #[cfg(all(unix, not(target_os = "macos")))]
    let auto = if external_uri.is_none() {
        let settings = state.settings_snapshot();
        let scan = crate::compat::scan(&settings);
        Some(
            crate::launch_plan::prepare_for_launch(
                app,
                &installed,
                &options,
                &prefix,
                &scan,
                &crate::util::now_rfc3339(),
            )
            .await?,
        )
    } else {
        None
    };
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    let auto: Option<crate::launch_plan::AutoLaunch> = None;

    // The plan fills only the fields still on their default; the options the
    // recovery snapshot records are therefore the effective ones (#298).
    let options = match &auto {
        Some(auto) => auto.options.clone(),
        None => options,
    };
    if let Some(auto) = &auto {
        for line in crate::launch_plan::log_lines(&auto.plan) {
            tracing::info!(game_id, reused = auto.reused, plan = %line, "launch plan");
        }
        if !auto.prerequisites.applied.is_empty() {
            tracing::info!(
                game_id,
                applied = ?auto.prerequisites.applied,
                "launch plan prerequisites applied"
            );
        }
        if !auto.prerequisites.failed.is_empty() {
            tracing::warn!(
                game_id,
                failed = ?auto.prerequisites.failed,
                "launch plan prerequisite recipe failed; the launch continues"
            );
        }
        if !auto.prerequisites.skipped.is_empty() {
            tracing::warn!(
                game_id,
                skipped = ?auto.prerequisites.skipped,
                "winetricks is missing; the prerequisite recipe was skipped"
            );
        }
    }

    let prepared = prepare_launch(
        app,
        game_id,
        &session_id,
        &installed,
        executable_path.as_deref(),
        working_dir.as_deref(),
        &install_path,
        &prefix,
        args.as_deref(),
        &options,
    )?;
    let tool = prepared.tool;
    let launch_log = prepared.launch_log;
    // #297: the launch log starts with the plan too, so support can reproduce
    // the exact configuration. The header belongs to `prepare_launch` (#298),
    // so the plan section is appended right after it, before the process
    // writes its own output.
    if let (Some(log), Some(auto)) = (launch_log.as_deref(), auto.as_ref()) {
        if let Err(error) = crate::launch_plan::append_log(log, &auto.plan) {
            tracing::warn!(error = %error, "could not write the launch plan into the log");
        }
    }
    // #298: remember the profile this session runs, so the watcher can count a
    // failure against it and revert on the second one.
    let executable_text = executable_path
        .as_deref()
        .map(|path| path.to_string_lossy().to_string());
    let plan = crate::launch_recovery::PlanSnapshot::new(
        &options,
        executable_text.as_deref(),
        args.as_deref(),
    )
    .with_resolved(prepared.resolved);
    let mut command = prepared.command;
    // #293: a game that prepared its shaders reads those caches back on every
    // launch. Applied after the compat plan so the pinned paths win, except
    // where the user set the same variable per game. A launcher URI has no
    // local cache and is left alone.
    let launches_local_game = installed
        .launch_uri
        .as_deref()
        .map(str::trim)
        .is_none_or(str::is_empty);
    if launches_local_game {
        crate::shader_prep::apply_launch_env(app, game_id, &options, &mut command);
    }
    // #180: record the guessed save folders before the session starts. The
    // blocking scan is bounded (per-path caps) and a failure only means the
    // exit hook has no proof, never that the launch is refused.
    let snapshot_app = app.clone();
    match tauri::async_runtime::spawn_blocking(move || {
        crate::commands::capture_save_candidates_snapshot(&snapshot_app, game_id)
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::debug!(error = %error, game_id, "save candidate snapshot failed")
        }
        Err(error) => {
            tracing::debug!(error = %error, game_id, "save candidate snapshot task failed")
        }
    }

    let child = command.spawn().map_err(|e| {
        tracing::error!(
            game_id,
            program = %executable_path
                .as_deref()
                .map(|path| path.display().to_string())
                .or_else(|| installed.launch_uri.clone())
                .unwrap_or_default(),
            working_dir = %working_dir.as_deref().unwrap_or(&install_path).display(),
            normalized,
            error = %e,
            "the game process could not be started"
        );
        match &executable_path {
            Some(path) => format!(
                "Could not launch {} (working directory {}): {e}",
                path.display(),
                working_dir.as_deref().unwrap_or(&install_path).display()
            ),
            None => format!(
                "Could not open {}: {e}",
                installed.launch_uri.as_deref().unwrap_or("the game")
            ),
        }
    })?;
    let pid = child.id();

    // #219: Windows only lets the foreground process hand the foreground over,
    // so let the game take it while it starts. A refusal is not fatal: the
    // focus ladder below retries once the window exists.
    crate::window_focus::grant_foreground(pid);

    let session_id = uuid::Uuid::new_v4().to_string();
    let started_at = chrono::Utc::now();
    let started_stamp = started_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let entry = Arc::new(ProcessEntry {
        child: std::sync::Mutex::new(child),
        pid,
        game_id,
        started_at,
        started: std::time::Instant::now(),
        launch_log,
        plan: Some(plan),
        stopped: std::sync::atomic::AtomicBool::new(false),
        auto_plan: auto.as_ref().map(|auto| Box::new(auto.plan.clone())),
        // #315: `None` unless the per-game opt-in ran and the device exists.
        virtual_pad: prepared.virtual_pad,
    });

    {
        let mut processes = state
            .processes
            .lock()
            .map_err(|_| "The process list is unavailable; restart Ludex.".to_string())?;
        if processes.values().any(|running| running.game_id == game_id) {
            if let Ok(mut child) = entry.child.lock() {
                let _ = child.kill();
            }
            return Err("This game is already running.".to_string());
        }
        processes.insert(session_id.clone(), Arc::clone(&entry));
    }

    if let Err(error) = record_launch(&state, &session_id, game_id, &started_stamp) {
        if let Ok(mut processes) = state.processes.lock() {
            processes.remove(&session_id);
        }
        if let Ok(mut child) = entry.child.lock() {
            let _ = child.kill();
        }
        return Err(error);
    }
    // #298: remember the resolved plan before the session ends, so a bundle
    // collected while the game runs (or after a Ludex crash) still shows what
    // was launched.
    if let Some(plan) = entry.plan.as_ref() {
        if let Err(error) = crate::launch_recovery::record_neutral(&state.app_data, game_id, plan) {
            tracing::debug!(error = %error, game_id, "could not record the launched plan");
        }
    }

    spawn_watcher(
        app.clone(),
        entry,
        session_id.clone(),
        game_id,
        installed.source.clone(),
        installed.title.clone(),
    );
    // #219: bring the game's window forward once it exists. Off-thread and
    // fire-and-forget: a slow or windowless game never delays the launch.
    spawn_focus_watch(app.clone(), game_id, session_id.clone(), pid);
    // #289: the game opens its audio stream after the process starts, so the
    // router waits for it off-thread and moves it to the per-game sink.
    crate::audio::spawn_route(app.clone(), game_id, pid, launch_audio_sink(&options));
    // The tray lists recent games by last launch.
    crate::tray::refresh_menu(app);
    announce_presence(app, game_id, &installed.title, started_at);

    // Opportunistically flush sessions that failed to sync earlier.
    let sync_app = app.clone();
    tauri::async_runtime::spawn(async move {
        sync_pending(sync_app).await;
    });

    Ok(LaunchResult {
        session_id,
        tool,
        source: installed.source,
    })
}

/// Re-apply the language/name a game opted into (#148) before it starts.
/// Best effort: a config that cannot be written must never block a launch.
fn apply_config_sync(app: &AppHandle, game_id: i64, install_path: &Path) {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let outcome = {
        let Ok(connection) = state.db.lock() else {
            tracing::warn!("config sync skipped: the local database lock is poisoned");
            return;
        };
        crate::config_fields::apply_sync(
            &state.app_data,
            install_path,
            &connection,
            game_id,
            &settings.ui_language,
            &settings.player_name,
        )
    };
    if !outcome.applied.is_empty() {
        tracing::info!(
            game_id,
            applied = outcome.applied.len(),
            "config sync applied to the game config"
        );
    }
    for warning in outcome.warnings {
        tracing::warn!(game_id, warning = %warning, "config sync warning");
    }
}

/// The launch command, plus what it resolved to for the recovery store (#298).
struct PreparedLaunch {
    command: Command,
    tool: Option<String>,
    launch_log: Option<PathBuf>,
    resolved: Option<crate::launch_recovery::ResolvedPlan>,
    /// #315: the virtual gamepad started before the spawn, if the game opted in.
    virtual_pad: Option<crate::virtual_pad::VirtualPadBridge>,
}

/// Build the process for a launch: URI handler, native executable, or a Linux
/// compatibility tool around a Windows executable (#89).
///
/// The per-launch compatibility log (#280) and the resolved plan (#298) come
/// back with the command; only a Linux compat spawn writes a log today.
#[cfg(all(unix, not(target_os = "macos")))]
#[allow(clippy::too_many_arguments)]
fn prepare_launch(
    app: &AppHandle,
    game_id: i64,
    session_id: &str,
    installed: &db::InstalledGame,
    executable: Option<&Path>,
    working_dir: Option<&Path>,
    install_path: &Path,
    prefix: &Path,
    args: Option<&str>,
    options: &crate::linux_gaming::LaunchOptions,
) -> Result<PreparedLaunch, String> {
    if let Some(uri) = installed
        .launch_uri
        .as_deref()
        .map(str::trim)
        .filter(|uri| !uri.is_empty())
    {
        return Ok(PreparedLaunch {
            command: build_uri_command(uri)?,
            tool: None,
            launch_log: None,
            resolved: None,
            // #315: a launcher URI is started as-is; the opt-in needs a local
            // executable to wrap, and the UI says so.
            virtual_pad: None,
        });
    }

    let executable =
        executable.ok_or_else(|| "No executable was found for this game.".to_string())?;
    let working_dir = working_dir.unwrap_or(install_path);
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let log_path = launch_log_path(&state.app_data, game_id, session_id);
    linux_prepare_launch(
        executable,
        working_dir,
        prefix,
        args,
        options,
        &settings,
        game_id,
        &log_path,
    )
}

/// Windows and macOS: no compatibility layer.
#[cfg(not(all(unix, not(target_os = "macos"))))]
#[allow(clippy::too_many_arguments)]
fn prepare_launch(
    _app: &AppHandle,
    _game_id: i64,
    _session_id: &str,
    installed: &db::InstalledGame,
    executable: Option<&Path>,
    working_dir: Option<&Path>,
    install_path: &Path,
    _prefix: &Path,
    args: Option<&str>,
    _options: &crate::linux_gaming::LaunchOptions,
) -> Result<PreparedLaunch, String> {
    if let Some(uri) = installed
        .launch_uri
        .as_deref()
        .map(str::trim)
        .filter(|uri| !uri.is_empty())
    {
        return Ok(PreparedLaunch {
            command: build_uri_command(uri)?,
            tool: None,
            launch_log: None,
            resolved: None,
            virtual_pad: None,
        });
    }

    let executable =
        executable.ok_or_else(|| "No executable was found for this game.".to_string())?;
    let working_dir = working_dir.unwrap_or(install_path);
    let resolved = crate::launch_recovery::ResolvedPlan::native(
        executable,
        &args.map(paths::split_args).unwrap_or_default(),
        None,
        None,
    );
    Ok(PreparedLaunch {
        command: build_command(executable, working_dir, args)?,
        tool: None,
        launch_log: None,
        resolved: Some(resolved),
        // #315: the virtual gamepad is Linux-only for now.
        virtual_pad: None,
    })
}

/// Linux: run a Windows executable through the selected compatibility tool.
///
/// The runner receives the resolved executable path in its arguments and the
/// same working directory as a native launch (#160), so a nested adopted
/// executable is found from the Wine/UMU side too.
///
/// #278: inside an AppImage the child environment drops the bundle's Python,
/// loader and Gtk variables and the mount entries in `PATH`. #280: the
/// wrapper's stdout/stderr go to `log_path` instead of `/dev/null`.
#[cfg(all(unix, not(target_os = "macos")))]
#[allow(clippy::too_many_arguments)]
fn linux_prepare_launch(
    executable: &Path,
    working_dir: &Path,
    prefix: &Path,
    args: Option<&str>,
    options: &crate::linux_gaming::LaunchOptions,
    settings: &crate::settings::LocalSettings,
    game_id: i64,
    log_path: &Path,
) -> Result<PreparedLaunch, String> {
    use crate::compat;
    use crate::linux_gaming::{self, DisplayMode, LaunchMode};

    // #187: the tool receives the normalised program and cwd.
    let normalized_executable = paths::normalize_native_path(executable);
    let normalized_working_dir = paths::normalize_native_path(working_dir);
    let executable = normalized_executable.as_path();
    let working_dir = normalized_working_dir.as_path();

    let is_windows_exe = executable
        .extension()
        .map(|extension| extension.eq_ignore_ascii_case("exe"))
        .unwrap_or(false);

    let mode = match options.mode {
        LaunchMode::Auto if is_windows_exe => LaunchMode::Auto,
        LaunchMode::Auto => LaunchMode::Native,
        other => other,
    };

    // #314: the preference is resolved against the devices present *now*, not
    // at save time. An unplugged pad therefore leaves the environment
    // untouched and logs a warning instead of hiding the pads that remain.
    let controllers = crate::controllers::resolve_for_launch(&settings.game_controllers, game_id);
    if let Some(warning) = &controllers.warning {
        tracing::warn!(game_id, warning, "preferred controller not applied");
    }
    if let Some(summary) = &controllers.summary {
        tracing::info!(game_id, summary, "preferred controller applied");
    }
    // #315: when the game opted in, one virtual pad is created before the
    // spawn and every physical joystick is hidden from SDL. Anything that can
    // go wrong (no /dev/uinput, no pad, ioctl refused) keeps the #314
    // environment and adds a warning: the launch itself never fails.
    let virtual_pad_preference =
        crate::virtual_pad::preference(&settings.game_virtual_pads, game_id);
    let virtual_pad = match virtual_pad_preference {
        Some(preference) => {
            let preferred = crate::controllers::preference(&settings.game_controllers, game_id);
            let inventory = crate::controllers::inventory(Path::new("/"));
            crate::virtual_pad::prepare_launch(
                Some(&preference),
                preferred.as_ref(),
                &inventory.devices,
                controllers,
            )
        }
        None => crate::virtual_pad::VirtualPadLaunch::untouched(controllers),
    };
    if let Some(warning) = &virtual_pad.warning {
        tracing::warn!(game_id, warning, "virtual gamepad not applied");
    }
    if let Some(summary) = &virtual_pad.summary {
        tracing::info!(game_id, summary, "virtual gamepad applied");
    }
    let controllers = virtual_pad.controllers;

    // #287: probe Gamescope only when the per-game profile asks for it, so an
    // ordinary launch pays nothing and never depends on the wrapper.
    let gamescope = if options.display.mode == DisplayMode::Gamescope {
        compat::detect_gamescope()
    } else {
        None
    };
    // #292: the output probe only runs when a switch is on. It reads sysfs and
    // the EDID; a failure is `unknown`, never an error.
    let display_capabilities = if options.display.mode == DisplayMode::Gamescope
        && (options.display.adaptive_sync || options.display.hdr)
    {
        Some(crate::display_caps::probe_default())
    } else {
        None
    };

    // Native keeps the historical behaviour: `.sh` through `sh`, `.exe`
    // through Wine when the user explicitly asked for it. Without a display
    // profile the command returned below is byte-identical to a build that
    // never had this feature.
    if mode == LaunchMode::Native {
        let mut base = build_command(executable, working_dir, args)?;
        // #314: native SDL games see the same enumeration as Proton.
        if !controllers.is_empty() {
            base.envs(controllers.env.iter());
        }
        let program = PathBuf::from(base.get_program());
        let base_args: Vec<String> = base
            .get_args()
            .map(|argument| argument.to_string_lossy().to_string())
            .collect();
        let resolved = linux_gaming::resolve_display_with_caps(
            &options.display,
            gamescope.as_ref(),
            display_capabilities.as_ref(),
            &program,
            &base_args,
        );
        if resolved.gamescope().is_none() {
            warn_display_fallback(game_id, &resolved);
            return Ok(PreparedLaunch {
                command: base,
                tool: None,
                launch_log: None,
                resolved: Some(crate::launch_recovery::ResolvedPlan::native(
                    &program, &base_args, None, None,
                )),
                // #315: the bridge started before the command was built; it
                // lives until the session ends.
                virtual_pad: virtual_pad.bridge,
            });
        }

        let mut resolved = resolved;
        // #297: GameMode wraps the entry point, inside Gamescope when one is
        // used. The built-in rules only ask for it on Windows launches.
        apply_gamemode_wrapper(&mut resolved, options.gamemode);
        // #278: Gamescope is a host program even when Ludex is an AppImage.
        let appimage = linux_gaming::appimage_env();
        let mut command = Command::new(&resolved.program);
        command
            .args(&resolved.args)
            .current_dir(working_dir)
            .stdin(Stdio::null());
        apply_appimage_env(&mut command, appimage.as_ref());
        if !controllers.is_empty() {
            command.envs(controllers.env.iter());
        }

        // #280/#287: the wrapper's stderr is the only evidence when gamescope
        // exits before the game, so a wrapped native launch gets a log too.
        let header = launch_log_header(&LaunchLogContext {
            game_id,
            executable,
            working_dir,
            tool: "none (native)",
            program: &program,
            prefix: None,
            game_id_env: None,
            appimage: appimage.as_ref(),
            display: &resolved,
            controllers: &controllers,
        });
        let launch_log = attach_launch_log(&mut command, log_path, &header);
        tracing::info!(
            program = %resolved.program.display(),
            working_dir = %working_dir.display(),
            log = %log_path.display(),
            display = ?resolved.display,
            "launching a native game through gamescope"
        );
        return Ok(PreparedLaunch {
            command,
            tool: None,
            launch_log,
            resolved: Some(crate::launch_recovery::ResolvedPlan::native(
                &resolved.program,
                &resolved.args,
                None,
                Some(&resolved),
            )),
            virtual_pad: virtual_pad.bridge,
        });
    }

    if !is_windows_exe {
        return Err(format!(
            "Proton and UMU need a Windows .exe. {} is not one; switch the launch mode to native.",
            executable.display()
        ));
    }

    let scan = compat::scan(settings);
    let mut selection = linux_gaming::select_tool(&scan.tools, mode, &options.tool_id)?;
    selection.client_install_path = scan.steam_client_path.as_deref().map(PathBuf::from);

    std::fs::create_dir_all(prefix).map_err(|error| {
        format!(
            "Could not create the Wine prefix {}: {error}",
            prefix.display()
        )
    })?;

    let extra = args.map(paths::split_args).unwrap_or_default();
    let plan = linux_gaming::build_launch(&selection, executable, &extra, prefix, options)?;
    // #287: the wrapper sits outside the resolved tool: `gamescope … -- umu-run
    // <exe>` for UMU and Proton-through-UMU, `-- wine <exe>` for Wine.
    let resolved = linux_gaming::resolve_display_with_caps(
        &options.display,
        scan.gamescope.as_ref(),
        display_capabilities.as_ref(),
        &plan.program,
        &plan.args,
    );
    warn_display_fallback(game_id, &resolved);
    let mut resolved = resolved;
    // #297: `gamemoderun umu-run <exe>`, before the Gamescope wrapper when one
    // is active: the plan states GameMode, the launch applies it.
    apply_gamemode_wrapper(&mut resolved, options.gamemode);

    // #278: the tool is a host program even when Ludex is an AppImage.
    let appimage = linux_gaming::appimage_env();

    let mut command = Command::new(&resolved.program);
    // #181 user-visible: the game (here through Wine/Proton) keeps its own
    // window; only internal probes are hidden.
    command.args(&resolved.args).current_dir(working_dir);
    // Before `.envs(plan.env)`: the per-game plan never sets these (they are
    // reserved), but the order keeps the cleanup authoritative.
    apply_appimage_env(&mut command, appimage.as_ref());
    // #314: before the per-game environment, so an explicit SDL_* override in
    // the game options still wins over the generated ignore list.
    if !controllers.is_empty() {
        command.envs(controllers.env.iter());
    }
    command.envs(plan.env.iter()).stdin(Stdio::null());

    // #280: keep what the wrapper prints; a silent `exit status: 53` is
    // undebuggable otherwise. A log that cannot be created is not fatal.
    let header = launch_log_header(&LaunchLogContext {
        game_id,
        executable,
        working_dir,
        tool: &plan.tool_name,
        program: &plan.program,
        prefix: Some(&plan.prefix),
        game_id_env: plan.env.get("GAMEID").map(String::as_str),
        appimage: appimage.as_ref(),
        display: &resolved,
        controllers: &controllers,
    });
    let launch_log = attach_launch_log(&mut command, log_path, &header);

    tracing::info!(
        tool = %plan.tool_name,
        prefix = %plan.prefix.display(),
        working_dir = %working_dir.display(),
        log = %log_path.display(),
        display = ?resolved.display,
        "launching a Windows game through a compatibility tool"
    );
    Ok(PreparedLaunch {
        command,
        tool: Some(plan.tool_name.clone()),
        launch_log,
        resolved: Some(crate::launch_recovery::ResolvedPlan::from_plan(
            &plan, &resolved,
        )),
        virtual_pad: virtual_pad.bridge,
    })
}

/// Build the command of one shader warm start (#293).
///
/// Deliberately the same path as a real launch: the compat tool, the prefix,
/// the working directory and the Gamescope profile are resolved by the same
/// helpers, so a game that starts for the player also starts here. Only the
/// environment differs (the cache paths are pinned to Ludex's folder).
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn prepare_warm_start(
    app: &AppHandle,
    installed: &db::InstalledGame,
    options: &crate::linux_gaming::LaunchOptions,
) -> Result<crate::shader_prep::PrepCommand, String> {
    if installed
        .launch_uri
        .as_deref()
        .map(str::trim)
        .is_some_and(|uri| !uri.is_empty())
    {
        return Err(
            "This entry starts another launcher, so there is no local shader cache to prepare."
                .to_string(),
        );
    }
    let install_path = paths::normalize_native_path(Path::new(&installed.install_path));
    if !install_path.is_dir() {
        return Err(format!(
            "The install folder is missing: {}. Reinstall the game before preparing its shaders.",
            install_path.display()
        ));
    }
    let prefix = crate::linux_gaming::prefix_for(&install_path, &options.prefix_path)?;
    let executable = resolve_executable(
        &install_path,
        Some(&prefix),
        None,
        installed.executable.as_deref(),
        &installed.title,
    )?;
    let working_dir = working_directory_override(&options.working_dir, &install_path)
        .unwrap_or_else(|| launch_working_directory(&install_path, &executable, &installed.source));
    let session_id = format!("prep-{}", uuid::Uuid::new_v4());
    let prepared = prepare_launch(
        app,
        installed.game_id,
        &session_id,
        installed,
        Some(&executable),
        Some(&working_dir),
        &install_path,
        &prefix,
        None,
        options,
    )?;
    Ok(prep_command_from(prepared.command, &install_path))
}

/// Copy the resolved `Command` into the plain-data shape the supervised
/// spawner takes. Only explicitly-set variables are copied; the parent
/// environment is inherited by the child anyway.
#[cfg(all(unix, not(target_os = "macos")))]
fn prep_command_from(
    command: std::process::Command,
    install_path: &Path,
) -> crate::shader_prep::PrepCommand {
    let program = PathBuf::from(command.get_program());
    let args: Vec<String> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect();
    let working_dir = command
        .get_current_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| install_path.to_path_buf());
    let env = command
        .get_envs()
        .filter_map(|(key, value)| {
            Some((
                key.to_string_lossy().to_string(),
                value?.to_string_lossy().to_string(),
            ))
        })
        .collect();
    crate::shader_prep::PrepCommand {
        program,
        args,
        working_dir,
        env,
    }
}

/// Shader preparation only runs on Linux: Windows and macOS keep their driver
/// caches next to the game without a wrapper Ludex can control.
#[cfg(not(all(unix, not(target_os = "macos"))))]
pub(crate) fn prepare_warm_start(
    _app: &AppHandle,
    _installed: &db::InstalledGame,
    _options: &crate::linux_gaming::LaunchOptions,
) -> Result<crate::shader_prep::PrepCommand, String> {
    Err("Shader preparation is only available on Linux.".to_string())
}

/// Log a display profile that was requested but could not be applied. The
/// launch itself still runs: a bad profile never fails a game.
#[cfg(all(unix, not(target_os = "macos")))]
fn warn_display_fallback(game_id: i64, resolved: &crate::linux_gaming::ResolvedLaunch) {
    if let Some(reason) = resolved.fallback_reason() {
        tracing::warn!(game_id, reason, "gamescope profile ignored");
    }
    if let Some(command) = resolved.gamescope() {
        for warning in &command.warnings {
            tracing::warn!(game_id, warning, "gamescope profile downgraded");
        }
    }
}

/// #297: put `gamemoderun` in front of the resolved entry point when the plan
/// asks for GameMode. `options.gamemode` is in-memory only and defaults to
/// false, so an unplanned launch never gains a wrapper.
#[cfg(all(unix, not(target_os = "macos")))]
fn apply_gamemode_wrapper(resolved: &mut crate::linux_gaming::ResolvedLaunch, enabled: bool) {
    if !enabled {
        return;
    }
    let (program, args) = crate::launch_plan::wrap_entry(
        std::mem::take(&mut resolved.program),
        std::mem::take(&mut resolved.args),
        &crate::launch_plan::Wrappers {
            gamemode: true,
            ..crate::launch_plan::Wrappers::default()
        },
        crate::launch_plan::gamemoderun_path(),
    );
    resolved.program = program;
    resolved.args = args;
}

/// #278: drop the AppImage bundle's Python, loader and Gtk variables before a
/// host program (`gamescope`, UMU, Proton, Wine) inherits the environment.
#[cfg(all(unix, not(target_os = "macos")))]
fn apply_appimage_env(command: &mut Command, appimage: Option<&crate::linux_gaming::AppImageEnv>) {
    if let Some(appimage) = appimage {
        for key in &appimage.removed {
            command.env_remove(key);
        }
        if let Some(path) = &appimage.path {
            command.env("PATH", path);
        }
    }
}

/// #280: send the wrapper's stdout/stderr to the launch log. A log that cannot
/// be created is not fatal.
#[cfg(all(unix, not(target_os = "macos")))]
fn attach_launch_log(command: &mut Command, log_path: &Path, header: &str) -> Option<PathBuf> {
    match open_launch_log(log_path, header) {
        Ok((stdout, stderr)) => {
            command.stdout(stdout).stderr(stderr);
            Some(log_path.to_path_buf())
        }
        Err(error) => {
            tracing::warn!(
                path = %log_path.display(),
                error = %error,
                "could not open the launch log; the tool output stays discarded"
            );
            command.stdout(Stdio::null()).stderr(Stdio::null());
            None
        }
    }
}

/// Everything the launch log header names (#280, #287).
#[cfg(all(unix, not(target_os = "macos")))]
struct LaunchLogContext<'a> {
    game_id: i64,
    executable: &'a Path,
    working_dir: &'a Path,
    tool: &'a str,
    program: &'a Path,
    prefix: Option<&'a Path>,
    game_id_env: Option<&'a str>,
    appimage: Option<&'a crate::linux_gaming::AppImageEnv>,
    display: &'a crate::linux_gaming::ResolvedLaunch,
    controllers: &'a crate::controllers::ControllerLaunch,
}

/// Header written before the wrapper takes over the launch log (#280).
#[cfg(all(unix, not(target_os = "macos")))]
fn launch_log_header(context: &LaunchLogContext<'_>) -> String {
    use crate::linux_gaming::DisplayLaunch;

    let mut header = String::new();
    header.push_str("=== Ludex compatibility launch ===\n");
    header.push_str(&format!("started: {}\n", now_rfc3339()));
    header.push_str(&format!("game: {}\n", context.game_id));
    header.push_str(&format!("tool: {}\n", context.tool));
    header.push_str(&format!("program: {}\n", context.program.display()));
    match context.prefix {
        Some(prefix) => header.push_str(&format!("prefix: {}\n", prefix.display())),
        None => header.push_str("prefix: none\n"),
    }
    header.push_str(&format!("executable: {}\n", context.executable.display()));
    header.push_str(&format!("working_dir: {}\n", context.working_dir.display()));
    if let Some(game_id) = context.game_id_env {
        header.push_str(&format!("GAMEID: {game_id}\n"));
    }
    match &context.display.display {
        DisplayLaunch::Desktop => header.push_str("display: desktop (no gamescope profile)\n"),
        DisplayLaunch::Fallback { reason } => {
            header.push_str(&format!(
                "display: desktop (gamescope not applied: {reason})\n"
            ));
        }
        DisplayLaunch::Gamescope(command) => {
            header.push_str(&format!(
                "display: gamescope (hdr: {}, adaptive sync: {}, filter: {})\n",
                command.hdr,
                command.adaptive_sync,
                command.filter.as_deref().unwrap_or("default")
            ));
            header.push_str(&format!(
                "gamescope command: {} {}\n",
                context.display.program.display(),
                context.display.args.join(" ")
            ));
            for warning in &command.warnings {
                header.push_str(&format!("gamescope warning: {warning}\n"));
            }
        }
    }
    match context.appimage {
        Some(appimage) => {
            header.push_str(&format!(
                "appimage env: removed {}\n",
                appimage.removed.join(", ")
            ));
            match &appimage.path {
                Some(path) => header.push_str(&format!(
                    "appimage PATH: {} mount entries dropped -> {path}\n",
                    appimage.dropped_path_entries
                )),
                None => header.push_str("appimage PATH: unchanged (no mount entry found)\n"),
            }
        }
        None => header.push_str("appimage env: none (host install, environment inherited)\n"),
    }
    // #314: the controller decision is the first thing to check when a game
    // binds the wheel instead of the pad.
    if let Some(summary) = &context.controllers.summary {
        header.push_str(&format!("controller: {summary}\n"));
        for (key, value) in &context.controllers.env {
            header.push_str(&format!("controller env: {key}={value}\n"));
        }
    }
    if let Some(warning) = &context.controllers.warning {
        header.push_str(&format!("controller warning: {warning}\n"));
    }
    header.push_str("=== tool output ===\n");
    header
}

/// Create the launch log, write `header` and return two append handles (#280).
#[cfg(all(unix, not(target_os = "macos")))]
fn open_launch_log(path: &Path, header: &str) -> std::io::Result<(Stdio, Stdio)> {
    use std::io::Write;

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    file.write_all(header.as_bytes())?;
    file.flush()?;
    let stderr = file.try_clone()?;
    Ok((Stdio::from(file), Stdio::from(stderr)))
}

fn record_launch(
    state: &AppState,
    session_id: &str,
    game_id: i64,
    started_at: &str,
) -> Result<(), String> {
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    db::insert_session(&connection, session_id, game_id, started_at)?;
    db::touch_launched(&connection, game_id, started_at)?;
    Ok(())
}

/// Deepest folder scan tried when a recorded executable is gone (#160).
///
/// `crate::bridge::exe::discover_executables` clamps to 4..=8; the maximum covers
/// GameVault layouts such as
/// `Installation/<edition>/<version>/binaries/x64/game.exe`.
const STALE_EXECUTABLE_SCAN_DEPTH: usize = crate::bridge::exe::MAX_SCAN_DEPTH;

/// Resolve the program to launch for an install (#160).
///
/// Order: the explicit choice for this launch, then the recorded executable,
/// then a folder scan. A recorded path is rooted at the install folder when it
/// is relative (GameVault/adopted rows can store either form) and must stay
/// inside the install folder or the Wine prefix of the same game; a folder or a
/// path outside the folder is never launched. Only a *missing* path falls
/// through, so moving or renaming the game's folder does not brick Play.
fn resolve_executable(
    install_path: &Path,
    extra_root: Option<&Path>,
    requested: Option<&str>,
    stored: Option<&str>,
    title: &str,
) -> Result<PathBuf, String> {
    // A path chosen for this launch wins, but the Play button also forwards the
    // recorded path here (apps/web `useLaunchGame`), so a stale value must fall
    // through to the record/discovery instead of blocking the launch (#160).
    if let Some(requested) = non_empty(requested) {
        match resolve_recorded_executable(install_path, extra_root, requested) {
            Ok(path) => return Ok(path),
            Err(ExecutableError::Missing(reason)) => tracing::warn!(
                executable = %requested,
                reason = %reason,
                "the requested executable is gone; trying the recorded path and a scan"
            ),
            Err(error) => {
                return Err(format!(
                    "The selected executable cannot be used: {}",
                    error.message()
                ));
            }
        }
    }

    let recorded = non_empty(stored);
    if let Some(recorded) = recorded {
        match resolve_recorded_executable(install_path, extra_root, recorded) {
            Ok(path) => return Ok(path),
            Err(ExecutableError::Missing(reason)) => tracing::warn!(
                executable = %recorded,
                install_path = %install_path.display(),
                reason = %reason,
                "the recorded executable is gone; scanning the folder again"
            ),
            // A path that exists but must not be launched (outside the folder,
            // a directory) is never silently replaced by a scan (#160).
            Err(ExecutableError::Rejected(reason)) => {
                return Err(format!(
                    "The saved executable for this game cannot be used: {reason}. Choose the game executable in the game settings."
                ));
            }
        }
    }

    // A stale (or absent) record falls back to discovery, deep enough for a
    // nested GameVault/edition folder (#160).
    let mut discovery =
        crate::bridge::discover_executables(install_path, title, STALE_EXECUTABLE_SCAN_DEPTH);
    let mut chosen = discovery.chosen.clone();
    if chosen.is_none() {
        if let Some(root) = extra_root {
            // A repack installer (#93) may have placed the game inside the
            // Wine prefix.
            let extra =
                crate::bridge::discover_executables(root, title, STALE_EXECUTABLE_SCAN_DEPTH);
            chosen = extra.chosen.clone();
            discovery.candidates.extend(extra.candidates);
            discovery.notes.extend(extra.notes);
        }
    }
    if let Some(path) = chosen {
        if recorded.is_some() {
            tracing::info!(
                executable = %path.display(),
                "using the discovered executable instead of the stale recorded path"
            );
        }
        return Ok(paths::normalize_native_path(&path));
    }

    Err(missing_executable_message(
        install_path,
        recorded,
        &discovery,
    ))
}

/// Why a recorded/selected executable cannot be used (#160).
#[derive(Debug)]
enum ExecutableError {
    /// The path no longer exists: a folder scan may find the game again.
    Missing(String),
    /// The path exists but must never be launched: outside the install folder
    /// (or the Wine prefix), or a folder instead of a program.
    Rejected(String),
}

impl ExecutableError {
    fn message(&self) -> &str {
        match self {
            ExecutableError::Missing(message) | ExecutableError::Rejected(message) => message,
        }
    }
}

/// Validate one recorded/selected executable path.
///
/// Returns the canonical path so the launch uses exactly what was checked.
/// Relative paths are resolved against the install folder; absolute paths must
/// still be inside it (or inside the game's Wine prefix).
fn resolve_recorded_executable(
    install_path: &Path,
    extra_root: Option<&Path>,
    recorded: &str,
) -> Result<PathBuf, ExecutableError> {
    let candidate = PathBuf::from(recorded.trim());
    let full = if candidate.is_absolute() {
        candidate
    } else {
        install_path.join(candidate)
    };
    let metadata = match std::fs::metadata(&full) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ExecutableError::Missing(format!(
                "{} does not exist",
                full.display()
            )));
        }
        Err(error) => {
            return Err(ExecutableError::Rejected(format!(
                "{} cannot be read: {error}",
                full.display()
            )));
        }
    };
    let canonical = std::fs::canonicalize(&full).map_err(|error| {
        ExecutableError::Rejected(format!("{} cannot be resolved: {error}", full.display()))
    })?;
    let allowed = paths::is_inside(install_path, &canonical)
        || extra_root
            .map(|root| paths::is_inside(root, &canonical))
            .unwrap_or(false);
    if !allowed {
        return Err(ExecutableError::Rejected(format!(
            "{} is outside the install folder",
            canonical.display()
        )));
    }
    if !metadata.is_file() && !is_app_bundle(&full) {
        return Err(ExecutableError::Rejected(format!(
            "{} is a folder, not a file",
            canonical.display()
        )));
    }
    // #187: `fs::canonicalize` returns the `\\?\` verbatim form on Windows.
    // Strip it here so every caller (launch, adoption storage) gets a path
    // `Command::new` can start.
    Ok(paths::normalize_native_path(&canonical))
}

/// True when a path is a program to launch: a regular file, or a macOS `.app`
/// bundle (which is a directory the system opens).
pub(crate) fn is_launchable_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if metadata.is_file() {
        return true;
    }
    is_app_bundle(path)
}

/// True for a macOS `.app` bundle directory.
fn is_app_bundle(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        path.is_dir()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        false
    }
}

/// The error shown when no executable can be resolved (#160).
///
/// It names the recorded path that was tried and lists what the scan found, so
/// the user can pick the right file in the game settings instead of guessing
/// why Play did nothing.
fn missing_executable_message(
    install_path: &Path,
    recorded: Option<&str>,
    discovery: &crate::bridge::Discovery,
) -> String {
    let mut message = match recorded {
        Some(recorded) => format!(
            "The saved executable for this game is missing: {recorded} (looked in {}).",
            install_path.display()
        ),
        None => format!(
            "No executable is recorded for this game in {}.",
            install_path.display()
        ),
    };
    if discovery.candidates.is_empty() {
        message.push_str(&format!(
            " Scanning {STALE_EXECUTABLE_SCAN_DEPTH} folder levels found nothing to run; choose the game executable in the game settings."
        ));
    } else {
        let listed = discovery
            .candidates
            .iter()
            .take(5)
            .map(|candidate| candidate.path.display().to_string())
            .collect::<Vec<String>>()
            .join(", ");
        let more = discovery.candidates.len().saturating_sub(5);
        message.push_str(&format!(
            " Ludex found {} executable(s) but cannot tell which one starts the game: {listed}",
            discovery.candidates.len()
        ));
        if more > 0 {
            message.push_str(&format!(" (and {more} more)"));
        }
        message.push_str(". Choose the right one in the game settings.");
    }
    for note in discovery.notes.iter().take(3) {
        message.push(' ');
        message.push_str(note);
    }
    message
}

/// Working directory for a launched game (#160).
///
/// Deterministic rule:
/// - an **adopted** install whose executable sits in a subfolder runs from the
///   executable's own folder. Adopted GameVault folders wrap the game in a
///   version/edition folder and the game loads its data relative to the binary;
///   running from the linked parent made it exit immediately (Uncharted).
/// - a **root-level** executable runs from the install root, which is also the
///   executable's folder.
/// - `ludex` installs keep the historical install-root behaviour (#129) so a
///   working layout is never moved under a game's feet.
///
/// A per-game override stored in the launch options replaces this rule; see
/// [`working_directory_override`].
pub(crate) fn launch_working_directory(
    install_path: &Path,
    executable: &Path,
    source: &str,
) -> PathBuf {
    let install_path = paths::normalize_native_path(install_path);
    let executable = paths::normalize_native_path(executable);
    if source == db::SOURCE_ADOPTED {
        if let Some(parent) = executable.parent() {
            if !parent.as_os_str().is_empty() && !same_path(parent, &install_path) {
                return parent.to_path_buf();
            }
        }
    }
    install_path
}

/// Resolve the persisted working-directory override (#187).
///
/// An empty value means "use [`launch_working_directory`]". A relative value is
/// resolved against the install folder, then normalised ([`paths::normalize_native_path`]).
/// A value that is not a readable folder is ignored with a warning: a bad
/// setting must never make the game unlaunchable.
pub(crate) fn working_directory_override(raw: &str, install_path: &Path) -> Option<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let candidate = PathBuf::from(raw);
    let full = if candidate.is_absolute() {
        candidate
    } else {
        install_path.join(candidate)
    };
    let normalized = paths::normalize_native_path(&full);
    if normalized.is_dir() {
        Some(normalized)
    } else {
        tracing::warn!(
            working_dir = %normalized.display(),
            "the saved working directory is missing; using the automatic folder"
        );
        None
    }
}

/// Path equality that tolerates one side being canonical and the other not.
fn same_path(left: &Path, right: &Path) -> bool {
    fn canonical(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }
    canonical(left) == canonical(right)
}

fn build_command(
    executable: &Path,
    working_dir: &Path,
    args: Option<&str>,
) -> Result<Command, String> {
    #[cfg(all(unix, not(target_os = "macos")))]
    ensure_executable_permission(executable)?;

    // #187: normalise both the program and the cwd right before the process is
    // created. The normalizer is a parameter so tests can drive the Windows
    // table on any host.
    build_command_with(executable, working_dir, args, paths::normalize_native_path)
}

/// Build the process with an explicit path normalizer.
///
/// `Command::new` and `current_dir` only ever see the output of `normalize`;
/// tests inject `paths::normalize_verbatim_string` to prove the Windows
/// behaviour without a Windows host.
fn build_command_with(
    executable: &Path,
    working_dir: &Path,
    args: Option<&str>,
    normalize: impl Fn(&Path) -> PathBuf,
) -> Result<Command, String> {
    let program = normalize(executable);
    let working_dir = normalize(working_dir);
    let mut command = platform_command(&program)?;
    command.current_dir(&working_dir);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    if let Some(args) = args {
        for argument in paths::split_args(args) {
            command.arg(argument);
        }
    }

    Ok(command)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn ensure_executable_permission(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::metadata(path)
        .map_err(|e| format!("Could not inspect {}: {e}", path.display()))?;
    let mode = metadata.permissions().mode();
    if mode & 0o111 == 0 {
        let mut permissions = metadata.permissions();
        permissions.set_mode(mode | 0o111);
        std::fs::set_permissions(path, permissions)
            .map_err(|e| format!("Could not make {} executable: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(windows)]
fn platform_command(executable: &Path) -> Result<Command, String> {
    // #181 user-visible: the game keeps its own window.
    Ok(Command::new(executable))
}

/// Open an external launch URI (`steam://rungameid/…`).
///
/// The handler (Steam) takes ownership of the game process, so playtime is
/// observed through the handler's short-lived process tree.
fn build_uri_command(uri: &str) -> Result<Command, String> {
    let uri = uri.trim();
    #[cfg(windows)]
    {
        // `start` hands the URI to Steam or another client, which owns its own
        // window; only internal probes are hidden. #181 user-visible
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", uri]);
        Ok(command)
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("open");
        command.arg(uri);
        Ok(command)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if paths::find_on_path("xdg-open").is_none() {
            return Err(
                "xdg-open is required to open external game launchers on Linux.".to_string(),
            );
        }
        let mut command = Command::new("xdg-open");
        command.arg(uri);
        Ok(command)
    }
}

#[cfg(target_os = "macos")]
fn platform_command(executable: &Path) -> Result<Command, String> {
    let is_bundle = executable
        .extension()
        .map(|extension| extension.eq_ignore_ascii_case("app"))
        .unwrap_or(false)
        || executable.is_dir();
    if is_bundle {
        // #181 user-visible: `open` hands the bundle to LaunchServices.
        let mut command = Command::new("open");
        command.arg(executable);
        Ok(command)
    } else {
        // #181 user-visible: the game keeps its own window.
        Ok(Command::new(executable))
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_command(executable: &Path) -> Result<Command, String> {
    let extension = executable
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    match extension.as_str() {
        "exe" => {
            let wine = paths::find_on_path("wine").ok_or_else(|| {
                "Wine is required to run Windows games on Linux. Install wine and try again."
                    .to_string()
            })?;
            // #181 user-visible: the game keeps its own window.
            let mut command = Command::new(wine);
            command.arg(executable);
            Ok(command)
        }
        "sh" => {
            // #181 user-visible: the game keeps its own window.
            let mut command = Command::new("sh");
            command.arg(executable);
            Ok(command)
        }
        // #181 user-visible: the game keeps its own window.
        _ => Ok(Command::new(executable)),
    }
}

/// Publish the running game to Discord (#12).
///
/// The cover URL is fetched from the server when it is reachable; a private or
/// failing URL simply leaves the art out. Nothing here blocks the launch.
fn announce_presence(
    app: &AppHandle,
    game_id: i64,
    title: &str,
    started_at: chrono::DateTime<chrono::Utc>,
) {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    if !settings.discord_presence || settings.discord_client_id.trim().is_empty() {
        return;
    }
    let title = title.to_string();
    let base = settings.server_url.trim_end_matches('/').to_string();
    let token = settings.token.clone();
    let http = state.http.clone();

    tauri::async_runtime::spawn(async move {
        let cover = if base.is_empty() || game_id <= 0 {
            None
        } else {
            fetch_cover(&http, &base, token.as_deref(), game_id).await
        };
        crate::presence::set_payload(Some(crate::presence::PresencePayload::playing(
            &title,
            cover.as_deref(),
            started_at,
        )));
    });
}

async fn fetch_cover(
    http: &reqwest::Client,
    base: &str,
    token: Option<&str>,
    game_id: i64,
) -> Option<String> {
    let mut request = http
        .get(format!("{base}/api/games/{game_id}"))
        .timeout(Duration::from_secs(5));
    if let Some(token) = non_empty(token) {
        request = request.bearer_auth(token);
    }
    let value: serde_json::Value = request.send().await.ok()?.json().await.ok()?;
    value
        .get("coverUrl")
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

/// #219: wait for the game's first top-level window and put it in the
/// foreground, then hide Ludex so the game is visible.
///
/// Runs on its own thread: the launch command has already returned its
/// `LaunchResult`, so a slow game (30 s budget) or a windowless one (launcher,
/// headless, console-only) never delays or fails the launch. Only the launched
/// PID is ever looked at: no other process can be brought forward.
fn spawn_focus_watch(app: AppHandle, game_id: i64, session_id: String, pid: u32) {
    if !window_focus::focus_supported() {
        // macOS (#219): no window-by-PID lookup, so this is a documented no-op
        // rather than a warning on every launch.
        tracing::debug!(
            game_id,
            pid,
            "focus on launch is not supported on this platform; leaving the focus alone"
        );
        return;
    }

    std::thread::spawn(move || {
        let (outcome, elapsed) = window_focus::wait_and_focus(pid);
        let state = app.state::<AppState>();
        let alive = state
            .processes
            .lock()
            .map(|processes| processes.contains_key(&session_id))
            .unwrap_or(false);
        if !alive {
            // The game exited (or crashed) while we waited: hiding Ludex and
            // asking for Resume would point at nothing.
            tracing::debug!(
                game_id,
                pid,
                elapsed_ms = elapsed.as_millis(),
                outcome = ?outcome,
                "the game exited before its window could be focused"
            );
            return;
        }

        let report = GameFocus::new(game_id, &session_id, &outcome);
        if report.focused {
            tracing::info!(
                game_id,
                pid,
                window = report.window.as_deref().unwrap_or_default(),
                elapsed_ms = elapsed.as_millis(),
                "the game window is in the foreground"
            );
            if should_hide_ludex(true, crate::console::active(&app)) {
                crate::tray::hide_window(&app);
            }
        } else {
            tracing::warn!(
                game_id,
                pid,
                reason = report.reason.as_deref().unwrap_or_default(),
                elapsed_ms = elapsed.as_millis(),
                "the game window could not be focused; the shell offers Resume and Force close"
            );
        }
        let _ = app.emit("game://focus", report);
        // The game page flips to Resume from its own poll; refresh nothing else.
    });
}

/// #297: tell the shell that an automatic plan failed, and store the failure
/// next to the plan. Called from the single #298 recovery failure arm, so the
/// decision (and the failure counter) stays in one place.
#[allow(clippy::too_many_arguments)]
fn emit_plan_failure(
    app: &AppHandle,
    state: &AppState,
    game_id: i64,
    session_id: &str,
    auto_plan: Option<&crate::launch_plan::LaunchPlan>,
    exit_code: Option<u32>,
    seconds: u64,
    at: &str,
) {
    let Some(plan) = auto_plan else {
        return;
    };
    if plan.source != crate::launch_plan::PlanSource::Auto {
        return;
    }
    let failure = crate::launch_plan::PlanFailure::from_recovery(
        exit_code,
        seconds,
        crate::launch_recovery::FIRST_FRAME_BUDGET.as_secs() as u32,
        crate::launch_recovery::is_fatal_exit(exit_code),
    );
    tracing::warn!(
        game_id,
        exit_code = %exit_code_hex(failure.exit_code),
        kind = ?failure.kind,
        budget_secs = failure.budget_secs,
        plan = %plan.sentence,
        "the launch plan failed; the recovery path owns the revert"
    );
    if let Err(error) = crate::launch_plan::record_failure(&state.app_data, game_id, &failure, at) {
        tracing::warn!(error = %error, "could not record the plan failure");
    }
    let _ = app.emit(
        "launch://plan-failed",
        crate::launch_plan::PlanFailedEvent {
            game_id,
            session_id: session_id.to_string(),
            plan_sentence: plan.sentence.clone(),
            rules: plan.rules.clone(),
            failure,
        },
    );
}

fn spawn_watcher(
    app: AppHandle,
    entry: Arc<ProcessEntry>,
    session_id: String,
    game_id: i64,
    source: String,
    title: String,
) {
    std::thread::spawn(move || {
        // #288: a running game keeps the machine awake for the whole session;
        // the guard releases when this thread ends (normal exit, stop, force
        // close or a panic in the tracking loop).
        let _awake = crate::inhibit::hold(crate::inhibit::Reason::game(&session_id, &title));
        // Process-tree tracking (#26): the session stays open while any
        // descendant of the launched game is alive, not just the root PID.
        let mut sampler = ProcessSampler::new();
        let mut tracker = TreeTracker::new(entry.pid);
        let mut exit_description: Option<String> = None;
        let mut exit_status: Option<std::process::ExitStatus> = None;

        loop {
            // Reap the root child as soon as it exits so it does not linger as
            // a zombie in the process snapshot (which would look "alive").
            if exit_description.is_none() {
                if let Ok(mut child) = entry.child.lock() {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            exit_status = Some(status);
                            exit_description = Some(status.to_string());
                        }
                        Ok(None) => {}
                        Err(error) => {
                            tracing::warn!(error = %error, "could not wait for the game process");
                            exit_description = Some(format!("error: {error}"));
                        }
                    }
                }
            }

            let snapshot = sampler.snapshot();
            let status = tracker.step(&snapshot);
            if status.finished {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }

        let elapsed_seconds = entry.started.elapsed().as_secs();
        let seconds = i64::try_from(elapsed_seconds).unwrap_or(i64::MAX);
        let minutes = ((elapsed_seconds + 30) / 60) as i64;
        let ended_at = now_rfc3339();

        // #187: an immediate exit is a failed launch, not a zero-minute
        // session. Keep the code (hex for NTSTATUS values) so the shell can
        // tell the user instead of doing nothing visible.
        let exit_code = exit_status.and_then(|status| status.code());
        let immediate_exit = immediate_exit_code(entry.started.elapsed(), exit_code);
        if let Some(code) = immediate_exit {
            // #280: name the log and what the code usually means; the wrapper's
            // own output is the only real evidence.
            tracing::warn!(
                game_id,
                status = ?exit_description,
                exit_code = %exit_code_hex(code),
                seconds,
                hint = immediate_exit_hint(code),
                log = %entry
                    .launch_log
                    .as_deref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
                "the game exited immediately"
            );
        } else {
            tracing::info!(
                game_id,
                started_at = %entry.started_at,
                status = ?exit_description,
                seconds,
                "game process tree exited"
            );
        }

        // #280: bound the logs once nothing can write to them any more.
        if let Some(log) = entry.launch_log.as_deref() {
            trim_launch_log(log);
            prune_launch_logs(log);
        }

        let state = app.state::<AppState>();
        // #298: a launch that never reached its first frame counts against the
        // profile; the second consecutive failure puts back the last plan that
        // worked, and the next Play uses it.
        if let Some(plan) = entry.plan.as_ref() {
            let stopped = entry.stopped.load(Ordering::SeqCst);
            let code = exit_code.map(|code| code as u32);
            let outcome = crate::launch_recovery::classify(entry.started.elapsed(), code, stopped);
            let path = crate::launch_recovery::state_path(&state.app_data);
            match outcome {
                crate::launch_recovery::LaunchOutcome::Success => {
                    if let Err(error) = crate::launch_recovery::record_success(&path, game_id, plan)
                    {
                        tracing::warn!(error = %error, game_id, "could not record the working launch plan");
                    }
                    // #297: the plan reached its frame; clear a stored failure
                    // so the panel stops offering the auto-revert warning.
                    if let Err(error) = crate::launch_plan::clear_failure(&state.app_data, game_id)
                    {
                        tracing::warn!(error = %error, game_id, "could not confirm the launch plan");
                    }
                }
                crate::launch_recovery::LaunchOutcome::NotCounted => {
                    if let Err(error) = crate::launch_recovery::record_neutral(&path, game_id, plan)
                    {
                        tracing::warn!(error = %error, game_id, "could not record the stopped launch");
                    }
                }
                crate::launch_recovery::LaunchOutcome::Failure => {
                    // #297 → #298: one failure decision feeds both the recovery
                    // counters above and the plan-facing event; the plan is
                    // never classified a second time.
                    emit_plan_failure(
                        &app,
                        &state,
                        game_id,
                        &session_id,
                        entry.auto_plan.as_deref(),
                        code,
                        elapsed_seconds,
                        &ended_at,
                    );
                    let failure = crate::launch_recovery::FailureRecord {
                        at: ended_at.clone(),
                        exit_code: code,
                        seconds: elapsed_seconds,
                    };
                    match crate::launch_recovery::record_failure(&path, game_id, plan, failure) {
                        Ok(decision) => {
                            if let Some(options) = decision.restore.as_ref() {
                                let applied = match state.db.lock() {
                                    Ok(connection) => db::save_launch_options(
                                        &connection,
                                        game_id,
                                        options,
                                        &ended_at,
                                    )
                                    .map_err(|error| {
                                        tracing::warn!(error = %error, game_id, "could not write the last working launch plan back")
                                    })
                                    .is_ok(),
                                    Err(_) => false,
                                };
                                if applied {
                                    let message = decision.notice(&title);
                                    tracing::warn!(
                                        game_id,
                                        from = decision.from_hash.as_deref().unwrap_or_default(),
                                        to = decision.to_hash.as_deref().unwrap_or_default(),
                                        "reverted the failing launch profile to the last plan that worked"
                                    );
                                    let _ = app.emit(
                                        "game://recovery",
                                        GameRecovery {
                                            game_id,
                                            title: title.clone(),
                                            reverted: true,
                                            from_hash: decision
                                                .from_hash
                                                .clone()
                                                .unwrap_or_default(),
                                            to_hash: decision.to_hash.clone().unwrap_or_default(),
                                            message,
                                        },
                                    );
                                }
                            } else if decision.failures > 0 {
                                tracing::warn!(
                                    game_id,
                                    failures = decision.failures,
                                    "the launch failed; no working plan is known yet"
                                );
                            }
                        }
                        Err(error) => {
                            tracing::warn!(error = %error, game_id, "could not record the failed launch");
                        }
                    }
                }
            }
        }
        if let Ok(connection) = state.db.lock() {
            if let Err(error) =
                db::finish_session(&connection, &session_id, &ended_at, minutes, seconds)
            {
                tracing::warn!(error = %error, "could not record the play session");
            }
        }
        if let Ok(mut processes) = state.processes.lock() {
            processes.remove(&session_id);
        }

        let _ = app.emit(
            "game://exited",
            GameExited {
                session_id: session_id.clone(),
                game_id,
                minutes_played: minutes,
                seconds_played: seconds,
                source,
                exit_code: exit_code.map(|code| code as u32),
                immediate_exit: immediate_exit.is_some(),
            },
        );

        // Cloud saves (#108): debounce the automatic upload so the game's
        // final writes settle; failures are queued and retried off-thread.
        crate::saves::sync::schedule_after_exit(app.clone(), game_id);

        // Discord presence goes back to idle (#12).
        crate::presence::set_payload(Some(crate::presence::PresencePayload::idle()));

        let sync_app = app.clone();
        let sync_session_id = session_id;
        tauri::async_runtime::spawn(async move {
            if let Err(error) = sync_session(&sync_app, &sync_session_id, game_id, minutes, 0).await
            {
                tracing::debug!(error = %error, "playtime will be synced later");
            }
            sync_pending(sync_app).await;
        });
    });
}

pub fn stop(app: &AppHandle, session_id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let entry = {
        let processes = state
            .processes
            .lock()
            .map_err(|_| "The process list is unavailable; restart Ludex.".to_string())?;
        processes.get(session_id).cloned()
    };
    let entry = entry.ok_or_else(|| format!("No running game for session {session_id}."))?;

    // #298: an intentional stop is never a failed launch, even when it happens
    // inside the 30 s startup budget.
    entry.stopped.store(true, Ordering::SeqCst);

    // Kill the whole tree: launchers spawn the real game as a child, and
    // killing only the root would leave it running in the background.
    let mut sampler = ProcessSampler::new();
    let killed = sampler.kill_tree(entry.pid);

    let mut child = entry
        .child
        .lock()
        .map_err(|_| "The game process is unavailable; restart Ludex.".to_string())?;
    match child.try_wait() {
        Ok(Some(_)) => Ok(()),
        Ok(None) => child
            .kill()
            .map(|_| ())
            .map_err(|e| format!("Could not stop the game: {e}")),
        Err(error) => Err(format!("Could not inspect the game process: {error}")),
    }?;

    // #315: destroy the virtual pad as soon as the game is stopped; the watcher
    // also drops it when the session ends, and `stop` is idempotent.
    if let Some(pad) = entry.virtual_pad.as_ref() {
        pad.stop();
    }

    tracing::info!(session_id, killed, "stopped the game process tree");
    Ok(())
}

/// How long the tree stop gets before the force close escalates (#191).
pub(crate) const FORCE_CLOSE_GRACE: Duration = Duration::from_secs(2);
/// How long a hard-killed tree gets to disappear before the close is failed.
pub(crate) const FORCE_CLOSE_VERIFY: Duration = Duration::from_secs(3);

/// Outcome of a force close, shown to the user and logged (#191).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForceCloseResult {
    /// True when the tree stop did not end the game and the hard kill ran.
    pub escalated: bool,
}

/// Force close (#191): the normal tree stop first, then a hard kill that is
/// verified. Unlike `stop`, a game that ignores the first pass does not leave
/// the page showing a ghost "running" session.
pub fn force_close(app: &AppHandle, session_id: &str) -> Result<ForceCloseResult, String> {
    let state = app.state::<AppState>();
    let pid = {
        let processes = state
            .processes
            .lock()
            .map_err(|_| "The process list is unavailable; restart Ludex.".to_string())?;
        processes.get(session_id).map(|entry| entry.pid)
    };
    // Idempotent: the game may have exited between the page's poll and the
    // click. There is nothing left to kill, and the page already offers Play.
    let Some(pid) = pid else {
        tracing::debug!(session_id, "force close: the session already ended");
        return Ok(ForceCloseResult { escalated: false });
    };

    // First pass: the same tree stop the tray and the overlay use. A game that
    // ignores it (anti-cheat helper, hung renderer) is still alive afterwards.
    if let Err(error) = stop(app, session_id) {
        tracing::debug!(session_id, error = %error, "the tree stop failed; escalating");
    }

    let mut sampler = ProcessSampler::new();
    let mut escalated = false;
    if !wait_for_session_end(&state, session_id, &mut sampler, pid, FORCE_CLOSE_GRACE) {
        escalated = true;
        let signalled = hard_kill_tree(pid);
        tracing::warn!(
            session_id,
            pid,
            signalled,
            "the game ignored the tree stop; hard-killed it"
        );
        if !wait_for_session_end(&state, session_id, &mut sampler, pid, FORCE_CLOSE_VERIFY) {
            tracing::error!(
                session_id,
                pid,
                "the game is still running after the hard kill"
            );
            return Err(format!(
                "The game is still running (PID {pid}). Close it from the system task manager."
            ));
        }
    }

    tracing::info!(session_id, pid, escalated, "force closed the game");
    Ok(ForceCloseResult { escalated })
}

/// True once the root PID is gone **and** the watcher removed the session, so
/// the game page cannot keep a stale "running" row.
fn session_ended(
    state: &AppState,
    session_id: &str,
    sampler: &mut ProcessSampler,
    pid: u32,
) -> bool {
    let root_alive = sampler.snapshot().iter().any(|process| process.pid == pid);
    let registered = state
        .processes
        .lock()
        .map(|processes| processes.contains_key(session_id))
        .unwrap_or(false);
    !root_alive && !registered
}

fn wait_for_session_end(
    state: &AppState,
    session_id: &str,
    sampler: &mut ProcessSampler,
    pid: u32,
    timeout: Duration,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if session_ended(state, session_id, sampler, pid) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// Second pass (#191): a platform-native tree kill for a game that survived
/// [`stop`]. Returns the number of processes signalled, for the log.
#[cfg(windows)]
fn hard_kill_tree(root: u32) -> usize {
    // `taskkill /F /T` also reaches helpers that sysinfo cannot enumerate.
    let status = crate::process::command("taskkill")
        .args(["/F", "/T", "/PID", &root.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let signalled = ProcessSampler::new().kill_tree(root);
    match status {
        Ok(status) if status.success() => signalled.max(1),
        _ => signalled,
    }
}

/// Unix: sysinfo already sends `SIGKILL`; re-snapshot so descendants spawned
/// between the two passes are reached too.
#[cfg(not(windows))]
fn hard_kill_tree(root: u32) -> usize {
    ProcessSampler::new().kill_tree(root)
}

pub async fn sync_session(
    app: &AppHandle,
    session_id: &str,
    game_id: i64,
    minutes: i64,
    attempts: i64,
) -> Result<(), String> {
    if minutes <= 0 {
        return Ok(());
    }

    let state = app.state::<AppState>();
    // Steam imports (#13) and other external entries have no server game.
    if game_id <= 0 {
        if let Ok(connection) = state.db.lock() {
            let _ = db::mark_session_synced(&connection, session_id);
        }
        return Ok(());
    }
    let settings = state.settings_snapshot();
    let base = settings.server_url.trim_end_matches('/');
    if base.is_empty() {
        // Not a delivery failure: the user has no server yet. Leave the row
        // untouched so it is retried once a server is configured.
        return Err("No server URL is configured.".to_string());
    }

    let mut request = state
        .http
        .post(format!("{base}/api/games/{game_id}/playtime"))
        .timeout(Duration::from_secs(15))
        // The session id is the outbox message id: replaying this exact report
        // after a crash between "server applied it" and "marked synced" is a
        // no-op there instead of adding the minutes again.
        .json(&serde_json::json!({ "minutes": minutes, "messageId": session_id }));
    if let Some(token) = non_empty(settings.token.as_deref()) {
        request = request.bearer_auth(token);
    }

    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            let message = format!("Playtime sync failed: {error}");
            record_sync_failure(app, session_id, attempts, &message, true);
            return Err(message);
        }
    };
    if !response.status().is_success() {
        let status = response.status();
        let message = format!("Playtime sync failed with HTTP {status}");
        // 4xx means the server will never accept this report (the game was
        // deleted, the token lost write scope, ...). Retrying it forever only
        // hides the failure; record it and stop.
        let retryable = !status.is_client_error()
            || status == reqwest::StatusCode::REQUEST_TIMEOUT
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
        record_sync_failure(app, session_id, attempts, &message, retryable);
        return Err(message);
    }

    if let Ok(connection) = state.db.lock() {
        let _ = db::mark_session_synced(&connection, session_id);
    }
    Ok(())
}

/// Exponential backoff for a failed playtime delivery (same shape as cloud
/// saves): 30 s, 1 min, 2 min, 4 min, then exhausted.
pub fn playtime_retry_delay(attempts: i64) -> Option<i64> {
    if attempts <= 0 || attempts > db::PLAYTIME_SYNC_MAX_ATTEMPTS {
        return None;
    }
    let exponent = u32::try_from(attempts - 1).unwrap_or(0).min(8);
    Some((30_i64 * 2_i64.pow(exponent)).min(3600))
}

/// Records one failed delivery so the worker backs off instead of retrying on
/// every launch. A non-retryable failure spends the budget immediately.
fn record_sync_failure(
    app: &AppHandle,
    session_id: &str,
    attempts: i64,
    message: &str,
    retryable: bool,
) {
    let state = app.state::<AppState>();
    let Ok(connection) = state.db.lock() else {
        return;
    };
    // A non-retryable rejection spends the budget immediately: `attempts`
    // above the cap plus a NULL `next_attempt_at` is what the outbox query
    // treats as "never retry automatically".
    let attempts = if retryable {
        attempts + 1
    } else {
        db::PLAYTIME_SYNC_MAX_ATTEMPTS + 1
    };
    let next = playtime_retry_delay(attempts).map(|seconds| {
        (chrono::Utc::now() + chrono::Duration::seconds(seconds))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    });
    let message = if next.is_none() {
        format!("{message} (automatic retries exhausted)")
    } else {
        message.to_string()
    };
    let _ = db::record_session_sync_failure(
        &connection,
        session_id,
        attempts,
        next.as_deref(),
        &message,
    );
}

pub async fn sync_pending(app: AppHandle) {
    if SYNCING.swap(true, Ordering::SeqCst) {
        return;
    }
    let _flag = SyncFlag;

    let state = app.state::<AppState>();
    let pending = {
        let Ok(connection) = state.db.lock() else {
            return;
        };
        match db::pending_sessions(&connection) {
            Ok(sessions) => sessions,
            Err(error) => {
                tracing::warn!(error = %error, "could not read pending playtime");
                return;
            }
        }
    };

    for session in pending {
        if let Err(error) = sync_session(
            &app,
            &session.id,
            session.game_id,
            session.minutes,
            session.attempts,
        )
        .await
        {
            tracing::debug!(error = %error, session = %session.id, "playtime still pending");
            // Stop at the first failure: the server is probably unreachable.
            break;
        }
    }
}

/// Flushes the outbox periodically so a delivery that failed while the app
/// stayed open is retried without waiting for the next launch.
pub fn start_retry_worker(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(120)).await;
            sync_pending(app.clone()).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ludex-launch-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn canonical(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    #[test]
    fn resolve_prefers_requested_then_stored() {
        let root = temp_root("prefer");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin/game.exe"), b"x").unwrap();
        std::fs::write(root.join("other.exe"), b"x").unwrap();

        let resolved = resolve_executable(&root, None, Some("bin/game.exe"), None, "Game").unwrap();
        assert!(resolved.ends_with("bin/game.exe"));

        let resolved = resolve_executable(&root, None, None, Some("other.exe"), "Game").unwrap();
        assert!(resolved.ends_with("other.exe"));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// #160: the GameVault layout stores the linked folder as the parent of an
    /// edition folder, and the executable as a path relative to it.
    #[test]
    fn a_relative_stored_executable_resolves_inside_the_adopted_folder() {
        let root = temp_root("nested");
        let installation = root.join("Installation");
        let edition = installation.join("Uncharted Legacy of Thieves Collection");
        std::fs::create_dir_all(edition.join("Uncharted4_data")).unwrap();
        std::fs::write(edition.join("Uncharted4.exe"), b"MZ").unwrap();
        std::fs::write(edition.join("Uncharted4_data/pak0.psarc"), b"data").unwrap();

        let stored = Path::new("Uncharted Legacy of Thieves Collection")
            .join("Uncharted4.exe")
            .to_string_lossy()
            .to_string();
        let resolved = resolve_executable(
            &installation,
            None,
            None,
            Some(&stored),
            "Uncharted: Legacy of Thieves Collection",
        )
        .unwrap();
        assert_eq!(resolved, canonical(&edition.join("Uncharted4.exe")));

        let cwd = launch_working_directory(&installation, &resolved, db::SOURCE_ADOPTED);
        assert_eq!(canonical(&cwd), canonical(&edition));
        assert!(cwd.starts_with(&installation));
    }

    /// #160: a stale recorded path falls back to a scan deep enough for
    /// GameVault's `Installation/<edition>/<version>/binaries/x64` nesting.
    #[test]
    fn a_stale_stored_executable_falls_back_to_a_deep_discovery() {
        let root = temp_root("stale");
        let installation = root.join("Installation");
        let deep = installation.join("a/b/c/d/e");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("Uncharted4.exe"), b"MZ").unwrap();

        let resolved = resolve_executable(
            &installation,
            None,
            None,
            Some("binaries/x64/Uncharted4.exe"),
            "Uncharted 4",
        )
        .unwrap();
        assert_eq!(resolved, canonical(&deep.join("Uncharted4.exe")));

        let cwd = launch_working_directory(&installation, &resolved, db::SOURCE_ADOPTED);
        assert_eq!(canonical(&cwd), canonical(&deep));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The Play button forwards the recorded path as `requested`
    /// (`apps/web/src/lib/hooks.ts`); a stale value there must fall back to the
    /// record and then to a scan, exactly like a stale stored path.
    #[test]
    fn a_stale_requested_executable_falls_back_to_the_record_and_a_scan() {
        let root = temp_root("stale-requested");
        let installation = root.join("Installation");
        std::fs::create_dir_all(installation.join("bin")).unwrap();
        std::fs::write(installation.join("bin/Uncharted4.exe"), b"MZ").unwrap();

        // The recorded path is still good.
        let resolved = resolve_executable(
            &installation,
            None,
            Some("gone/Uncharted4.exe"),
            Some("bin/Uncharted4.exe"),
            "Uncharted 4",
        )
        .unwrap();
        assert_eq!(
            resolved,
            canonical(&installation.join("bin/Uncharted4.exe"))
        );

        // No record at all: discovery takes over.
        let resolved = resolve_executable(
            &installation,
            None,
            Some("gone/Uncharted4.exe"),
            None,
            "Uncharted 4",
        )
        .unwrap();
        assert_eq!(
            resolved,
            canonical(&installation.join("bin/Uncharted4.exe"))
        );

        // A requested path that exists but escapes the folder still fails hard.
        std::fs::write(root.join("outside.exe"), b"MZ").unwrap();
        let absolute = root.join("outside.exe").to_string_lossy().to_string();
        let error = resolve_executable(&installation, None, Some(&absolute), None, "Uncharted 4")
            .unwrap_err();
        assert!(error.contains("outside the install folder"), "{error}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_executable_outside_the_install_folder_is_never_launched() {
        let root = temp_root("outside");
        let installation = root.join("Installation");
        std::fs::create_dir_all(&installation).unwrap();
        std::fs::write(root.join("outside.exe"), b"MZ").unwrap();

        // A recorded path escaping the folder is refused instead of scanned
        // around: an existing file outside the folder must never run.
        let error = resolve_executable(
            &installation,
            None,
            None,
            Some("../outside.exe"),
            "Uncharted 4",
        )
        .unwrap_err();
        assert!(error.contains("outside the install folder"), "{error}");

        let absolute = root.join("outside.exe").to_string_lossy().to_string();
        let error = resolve_executable(&installation, None, Some(&absolute), None, "Uncharted 4")
            .unwrap_err();
        assert!(error.contains("outside the install folder"), "{error}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_recorded_as_the_executable_is_rejected() {
        let root = temp_root("folder-exe");
        let installation = root.join("Installation");
        std::fs::create_dir_all(installation.join("binaries")).unwrap();
        std::fs::write(installation.join("Uncharted4.exe"), b"MZ").unwrap();

        let error = resolve_executable(&installation, None, None, Some("binaries"), "Uncharted 4")
            .unwrap_err();
        assert!(error.contains("is a folder, not a file"), "{error}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_executable_error_lists_what_the_scan_found() {
        let root = temp_root("candidates");
        let installation = root.join("Installation");
        std::fs::create_dir_all(installation.join("alpha")).unwrap();
        std::fs::create_dir_all(installation.join("beta")).unwrap();
        std::fs::write(installation.join("alpha/alpha.exe"), b"MZ").unwrap();
        std::fs::write(installation.join("beta/beta.exe"), b"MZ").unwrap();

        let error = resolve_executable(
            &installation,
            None,
            None,
            Some("binaries/x64/Uncharted4.exe"),
            "Uncharted 4",
        )
        .unwrap_err();
        // The saved path is named, both candidates are listed, and the user is
        // pointed at the settings instead of a bare failure.
        assert!(error.contains("Uncharted4.exe"), "{error}");
        assert!(error.contains("alpha.exe"), "{error}");
        assert!(error.contains("beta.exe"), "{error}");
        assert!(error.contains("game settings"), "{error}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_executable_error_explains_an_empty_scan() {
        let root = temp_root("empty");
        let installation = root.join("Installation");
        std::fs::create_dir_all(&installation).unwrap();

        let error = resolve_executable(&installation, None, None, None, "Uncharted 4").unwrap_err();
        assert!(error.contains("No executable is recorded"), "{error}");
        assert!(error.contains("folder levels"), "{error}");
        assert!(error.contains("game settings"), "{error}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_root_level_executable_keeps_the_install_root_as_working_directory() {
        let root = temp_root("root-level");
        std::fs::write(root.join("Uncharted4.exe"), b"MZ").unwrap();

        let resolved =
            resolve_executable(&root, None, None, Some("Uncharted4.exe"), "Uncharted 4").unwrap();
        let cwd = launch_working_directory(&root, &resolved, db::SOURCE_ADOPTED);
        assert_eq!(canonical(&cwd), canonical(&root));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The cwd rule is scoped to adopted folders: a `ludex` install keeps the
    /// historical install-root behaviour (#129) even with a nested executable.
    #[test]
    fn a_ludex_install_keeps_the_install_root_as_working_directory() {
        let root = temp_root("ludex-level");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin/game.exe"), b"MZ").unwrap();

        let resolved = resolve_executable(&root, None, None, Some("bin/game.exe"), "Game").unwrap();
        let cwd = launch_working_directory(&root, &resolved, db::SOURCE_LUDEX);
        assert_eq!(canonical(&cwd), canonical(&root));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// #187: a persisted working-directory override wins over the adopted
    /// rule; a relative value is resolved against the install folder and a
    /// missing folder is ignored instead of blocking the launch.
    #[test]
    fn a_saved_working_directory_override_wins_over_the_adopted_rule() {
        let root = temp_root("cwd-override");
        let installation = root.join("Installation");
        let edition = installation.join("Uncharted Legacy of Thieves Collection");
        let override_dir = root.join("cwd");
        std::fs::create_dir_all(&edition).unwrap();
        std::fs::create_dir_all(&override_dir).unwrap();
        std::fs::write(edition.join("Uncharted4.exe"), b"MZ").unwrap();

        let resolved = resolve_executable(
            &installation,
            None,
            None,
            Some("Uncharted Legacy of Thieves Collection/Uncharted4.exe"),
            "Uncharted 4",
        )
        .unwrap();
        let rule = launch_working_directory(&installation, &resolved, db::SOURCE_ADOPTED);
        assert_eq!(canonical(&rule), canonical(&edition), "rule: exe folder");

        let chosen =
            working_directory_override(&override_dir.to_string_lossy(), &installation).unwrap();
        assert_eq!(canonical(&chosen), canonical(&override_dir));

        std::fs::create_dir_all(installation.join("bin")).unwrap();
        let relative = working_directory_override("bin", &installation).unwrap();
        assert_eq!(canonical(&relative), canonical(&installation.join("bin")));

        assert!(working_directory_override("   ", &installation).is_none());
        assert!(working_directory_override("gone", &installation).is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// #187: the program and cwd handed to `Command` are exactly what the
    /// normalizer returns. The Windows table is injected here so the wiring is
    /// checked on every host; the platform normalizer is a no-op elsewhere.
    #[test]
    fn the_command_takes_its_program_and_cwd_from_the_normalizer() {
        use std::ffi::OsStr;

        let executable = Path::new(r"\\?\C:\Games\Installation\Uncharted\u4.bin");
        let working_dir = Path::new(r"\\?\C:\Games\Installation\Uncharted");
        let command = build_command_with(executable, working_dir, Some("--windowed"), |path| {
            PathBuf::from(paths::normalize_verbatim_string(&path.to_string_lossy()))
        })
        .unwrap();

        assert_eq!(
            command.get_program(),
            OsStr::new(r"C:\Games\Installation\Uncharted\u4.bin")
        );
        assert_eq!(
            command.get_current_dir(),
            Some(Path::new(r"C:\Games\Installation\Uncharted"))
        );
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        assert_eq!(args, vec!["--windowed"]);
    }

    /// #289: the dedicated Audio output field wins, and the older tier-2
    /// `PULSE_SINK` switch still names a sink when the field is empty. Both
    /// surfaces then route the same stream instead of overriding each other.
    #[test]
    fn the_launch_sink_merges_the_audio_field_and_pulse_sink() {
        let mut options = crate::linux_gaming::LaunchOptions::default();
        assert_eq!(launch_audio_sink(&options), "");
        options
            .env
            .insert("PULSE_SINK".into(), " alsa_output.hdmi ".into());
        assert_eq!(launch_audio_sink(&options), "alsa_output.hdmi");
        options.audio_sink = "alsa_output.headset".into();
        assert_eq!(launch_audio_sink(&options), "alsa_output.headset");
    }

    /// #187: the immediate-exit mapping feeds the `game://exited` payload. An
    /// NTSTATUS code keeps its bits (`0xC0000409`), the 5 s boundary is
    /// exclusive, and a signal kill (no code) never raises the toast.
    #[test]
    fn immediate_exit_codes_are_mapped_and_hex_formatted() {
        assert_eq!(
            immediate_exit_code(Duration::from_millis(1), Some(0)),
            Some(0)
        );
        assert_eq!(
            immediate_exit_code(Duration::from_secs(2), Some(0xC0000409u32 as i32)),
            Some(0xC0000409)
        );
        assert_eq!(
            immediate_exit_code(Duration::from_millis(4_999), Some(3)),
            Some(3)
        );
        assert_eq!(immediate_exit_code(Duration::from_secs(5), Some(3)), None);
        assert_eq!(immediate_exit_code(Duration::from_secs(1), None), None);
        assert_eq!(exit_code_hex(0xC0000409), "0xC0000409");
    }

    /// #219: the payload the shell receives after waiting for the game window.
    /// A failure is reported as data — `focused: false` plus a reason — never as
    /// a launch error, and the launch result is built before the wait starts.
    #[test]
    fn a_focus_failure_is_reported_not_raised() {
        let success = GameFocus::new(
            7,
            "session-1",
            &FocusOutcome::Focused {
                window: "4242".to_string(),
            },
        );
        assert!(success.focused);
        assert_eq!(success.window.as_deref(), Some("4242"));
        assert_eq!(success.reason, None);

        let no_window = GameFocus::new(7, "session-1", &FocusOutcome::NoWindow);
        assert!(!no_window.focused);
        assert_eq!(no_window.window, None);
        assert_eq!(no_window.reason.as_deref(), Some("noWindow"));

        let refused = GameFocus::new(
            7,
            "session-1",
            &FocusOutcome::Refused {
                window: "9".to_string(),
            },
        );
        assert!(!refused.focused);
        assert_eq!(refused.window.as_deref(), Some("9"));
        assert_eq!(refused.reason.as_deref(), Some("focusRefused"));
    }

    /// #219: Ludex only steps aside when the game really took the foreground.
    /// A failed focus leaves Ludex on screen (Resume/Force close live there),
    /// and a console session keeps its shell because the shell is the display.
    #[test]
    fn ludex_is_hidden_only_after_a_successful_focus() {
        assert!(should_hide_ludex(true, false));
        assert!(!should_hide_ludex(false, false));
        assert!(!should_hide_ludex(true, true));
        assert!(!should_hide_ludex(false, true));
    }

    /// The playtime outbox backs off like cloud saves and then stops: a dead
    /// server must not be polled on every launch forever.
    #[test]
    fn playtime_retry_delay_is_bounded() {
        assert_eq!(playtime_retry_delay(0), None);
        assert_eq!(playtime_retry_delay(1), Some(30));
        assert_eq!(playtime_retry_delay(2), Some(60));
        assert_eq!(playtime_retry_delay(3), Some(120));
        assert_eq!(
            playtime_retry_delay(db::PLAYTIME_SYNC_MAX_ATTEMPTS),
            Some(480)
        );
        assert_eq!(
            playtime_retry_delay(db::PLAYTIME_SYNC_MAX_ATTEMPTS + 1),
            None
        );
    }

    /// #160 end-to-end: the resolved nested executable really runs, from its
    /// own folder, with the launch arguments, as a direct child (the process
    /// the playtime watcher tracks is the game, not a wrapper).
    #[cfg(unix)]
    #[test]
    fn a_nested_adopted_executable_launches_with_its_folder_as_cwd_and_args() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_root("spawn");
        let installation = root.join("Installation");
        let edition = installation.join("Uncharted Legacy of Thieves Collection");
        std::fs::create_dir_all(&edition).unwrap();
        let script = edition.join("launch.sh");
        std::fs::write(
            &script,
            r#"#!/bin/sh
out="$1"
shift
{
  printf 'cwd=%s\n' "$(pwd -P)"
  for arg in "$@"; do
    printf 'arg=%s\n' "$arg"
  done
} > "$out"
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).unwrap();

        let stored = Path::new("Uncharted Legacy of Thieves Collection")
            .join("launch.sh")
            .to_string_lossy()
            .to_string();
        let executable = resolve_executable(
            &installation,
            None,
            None,
            Some(&stored),
            "Uncharted: Legacy of Thieves Collection",
        )
        .unwrap();
        let cwd = launch_working_directory(&installation, &executable, db::SOURCE_ADOPTED);

        let output = root.join("launch.txt");
        let args = format!("\"{}\" --windowed \"save slot\"", output.display());
        let status = build_command(&executable, &cwd, Some(&args))
            .unwrap()
            .status()
            .unwrap();
        assert!(status.success(), "the fixture game exited with {status}");

        let recorded = std::fs::read_to_string(&output).unwrap();
        let expected_cwd = canonical(&edition).display().to_string();
        assert!(
            recorded.contains(&format!("cwd={expected_cwd}")),
            "wrong working directory: {recorded}"
        );
        assert!(recorded.contains("arg=--windowed"), "{recorded}");
        assert!(recorded.contains("arg=save slot"), "{recorded}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// #280: the immediate-exit log names the likely layer for the known codes
    /// and never claims a cause for the unknown ones.
    #[test]
    fn immediate_exit_hints_name_the_known_codes() {
        assert!(immediate_exit_hint(1).contains("environment"));
        assert!(immediate_exit_hint(53).contains("Steam emulator"));
        assert!(immediate_exit_hint(0xC0000409).contains("crashed"));
        assert!(immediate_exit_hint(127).contains("launch log"));
        // 0x35 is 53: one hint covers both spellings.
        assert_eq!(immediate_exit_hint(0x35), immediate_exit_hint(53));
    }

    /// #280: the log name carries the game and session so a report can match
    /// the file with the session in `ludex.log`.
    #[test]
    fn launch_log_path_carries_the_game_and_session() {
        let path = launch_log_path(Path::new("/data/ludex"), 14, "0f8fad5b-uuid");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("launch-14-0f8fad5b-uuid.log")
        );
        assert_eq!(
            path.parent().and_then(|dir| dir.file_name()),
            Some(std::ffi::OsStr::new("logs"))
        );
    }

    /// #287: a wrapped launch still writes the same per-session log, and its
    /// header names the wrapper so a gamescope exit before the game is
    /// diagnosable from the file alone.
    #[test]
    fn a_gamescope_launch_names_the_wrapper_in_the_log_header() {
        use crate::linux_gaming::{DisplayLaunch, GamescopeCommand, ResolvedLaunch};

        let resolved = ResolvedLaunch {
            program: PathBuf::from("/usr/bin/gamescope"),
            args: vec![
                "-w".to_string(),
                "1920".to_string(),
                "-h".to_string(),
                "1080".to_string(),
                "-r".to_string(),
                "60".to_string(),
                "--filter".to_string(),
                "fsr".to_string(),
                "--".to_string(),
                "/usr/bin/umu-run".to_string(),
                "/games/Hades/Hades.exe".to_string(),
            ],
            display: DisplayLaunch::Gamescope(GamescopeCommand {
                hdr: false,
                adaptive_sync: true,
                filter: Some("fsr".to_string()),
                warnings: Vec::new(),
            }),
        };
        let header = launch_log_header(&LaunchLogContext {
            game_id: 14,
            executable: Path::new("/games/Hades/Hades.exe"),
            working_dir: Path::new("/games/Hades"),
            tool: "GE-Proton9-20",
            program: Path::new("/usr/bin/umu-run"),
            prefix: Some(Path::new("/games/Hades/.prefix")),
            game_id_env: Some("0"),
            appimage: None,
            display: &resolved,
            controllers: &crate::controllers::ControllerLaunch::default(),
        });
        assert!(
            header.contains("display: gamescope (hdr: false, adaptive sync: true, filter: fsr)")
        );
        assert!(header.contains(
            "gamescope command: /usr/bin/gamescope -w 1920 -h 1080 -r 60 --filter fsr -- /usr/bin/umu-run /games/Hades/Hades.exe"
        ));
        assert!(header.contains("program: /usr/bin/umu-run"));
        assert!(header.ends_with("=== tool output ===\n"));
    }

    /// #287: the default profile is a no-op and says so in the log.
    #[test]
    fn a_desktop_launch_says_no_gamescope_profile_was_set() {
        use crate::linux_gaming::{DisplayLaunch, ResolvedLaunch};

        let resolved = ResolvedLaunch {
            program: PathBuf::from("/usr/bin/umu-run"),
            args: vec!["/games/Hades/Hades.exe".to_string()],
            display: DisplayLaunch::Desktop,
        };
        let header = launch_log_header(&LaunchLogContext {
            game_id: 14,
            executable: Path::new("/games/Hades/Hades.exe"),
            working_dir: Path::new("/games/Hades"),
            tool: "GE-Proton9-20",
            program: Path::new("/usr/bin/umu-run"),
            prefix: Some(Path::new("/games/Hades/.prefix")),
            game_id_env: None,
            appimage: None,
            display: &resolved,
            controllers: &crate::controllers::ControllerLaunch::default(),
        });
        assert!(header.contains("display: desktop (no gamescope profile)"));
        assert!(!header.contains("gamescope command:"));
    }

    /// #314: the controller decision and its variables are in the header, so a
    /// game that binds the wheel can be diagnosed from the log alone.
    #[test]
    fn a_preferred_controller_is_named_in_the_log_header() {
        use crate::linux_gaming::{DisplayLaunch, ResolvedLaunch};

        let resolved = ResolvedLaunch {
            program: PathBuf::from("/usr/bin/umu-run"),
            args: vec!["/games/Hades/Hades.exe".to_string()],
            display: DisplayLaunch::Desktop,
        };
        let controllers = crate::controllers::ControllerLaunch {
            env: std::collections::BTreeMap::from([
                (
                    "SDL_GAMECONTROLLER_IGNORE_DEVICES".to_string(),
                    "0x046d/0xc260".to_string(),
                ),
                (
                    "SDL_JOYSTICK_DEVICE".to_string(),
                    "/dev/input/js2".to_string(),
                ),
            ]),
            summary: Some(
                "preferred Microsoft X-Box 360 pad (045e:028e, js2); ignoring Logitech G29"
                    .to_string(),
            ),
            warning: None,
        };
        let header = launch_log_header(&LaunchLogContext {
            game_id: 14,
            executable: Path::new("/games/Hades/Hades.exe"),
            working_dir: Path::new("/games/Hades"),
            tool: "GE-Proton9-20",
            program: Path::new("/usr/bin/umu-run"),
            prefix: Some(Path::new("/games/Hades/.prefix")),
            game_id_env: None,
            appimage: None,
            display: &resolved,
            controllers: &controllers,
        });
        assert!(header.contains("controller: preferred Microsoft X-Box 360 pad"));
        assert!(
            header.contains("controller env: SDL_GAMECONTROLLER_IGNORE_DEVICES=0x046d/0xc260\n")
        );
        assert!(header.contains("controller env: SDL_JOYSTICK_DEVICE=/dev/input/js2\n"));
        assert!(!header.contains("controller warning:"));

        let missing = crate::controllers::ControllerLaunch {
            env: std::collections::BTreeMap::new(),
            summary: None,
            warning: Some("The preferred controller X is not connected".to_string()),
        };
        let header = launch_log_header(&LaunchLogContext {
            game_id: 14,
            executable: Path::new("/games/Hades/Hades.exe"),
            working_dir: Path::new("/games/Hades"),
            tool: "GE-Proton9-20",
            program: Path::new("/usr/bin/umu-run"),
            prefix: Some(Path::new("/games/Hades/.prefix")),
            game_id_env: None,
            appimage: None,
            display: &resolved,
            controllers: &missing,
        });
        assert!(
            header.contains("controller warning: The preferred controller X is not connected\n")
        );
        assert!(!header.contains("controller env:"));
    }

    /// #280: a huge Proton log keeps its tail (the error is at the end) on a
    /// line boundary and gains a truncation marker.
    #[test]
    fn a_launch_log_is_truncated_to_its_last_kibibytes() {
        let root = temp_root("log-trim");
        let log = root.join("launch-14-session.log");
        let line = format!("{}-0123456789abcdef\n", "x".repeat(80));
        let mut body = String::from("header\n");
        while body.len() < (LAUNCH_LOG_MAX_BYTES as usize) + 64 * 1024 {
            body.push_str(&line);
        }
        body.push_str("final error line\n");
        std::fs::write(&log, &body).unwrap();

        trim_launch_log(&log);

        let after = std::fs::read_to_string(&log).unwrap();
        assert!(after.len() as u64 <= LAUNCH_LOG_MAX_BYTES + 128);
        assert!(after.starts_with("[ludex: log truncated"));
        assert!(after.contains("final error line"));
        assert!(!after.contains("header"));
        // Every kept line is complete: the tail starts after a newline.
        for line in after.lines().skip(1) {
            if line == "final error line" {
                continue;
            }
            assert_eq!(line.len(), 97);
        }

        // A small log is left byte-for-byte alone.
        std::fs::write(&log, b"short log\n").unwrap();
        trim_launch_log(&log);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "short log\n");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// #280: only the five newest `launch-*.log` survive; unrelated files in
    /// the logs folder are never touched.
    #[test]
    fn launch_logs_are_pruned_to_the_five_newest() {
        let root = temp_root("log-prune");
        // A dedicated folder: the real logs folder may hold other files.
        let dir = root.join("logs");
        std::fs::create_dir_all(&dir).unwrap();
        let newest = dir.join("launch-14-newest.log");
        for index in 0..7 {
            let path = dir.join(format!("launch-1-{index}.log"));
            std::fs::write(&path, b"x").unwrap();
            // Deterministic order: newer index, newer mtime.
            std::thread::sleep(Duration::from_millis(10));
        }
        std::fs::write(&newest, b"x").unwrap();
        std::fs::write(dir.join("ludex.log"), b"keep").unwrap();
        std::fs::write(dir.join("notes.txt"), b"keep").unwrap();

        prune_launch_logs(&newest);

        let mut remaining: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(LAUNCH_LOG_PREFIX))
            .collect();
        remaining.sort();
        assert_eq!(remaining.len(), LAUNCH_LOG_KEEP);
        assert!(remaining.contains(&"launch-14-newest.log".to_string()));
        assert!(dir.join("ludex.log").is_file());
        assert!(dir.join("notes.txt").is_file());

        let _ = std::fs::remove_dir_all(&root);
    }
}
