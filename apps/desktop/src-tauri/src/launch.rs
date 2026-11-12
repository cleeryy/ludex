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

    let install_path = PathBuf::from(&installed.install_path);
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
    // [`launch_working_directory`]).
    let working_dir = executable_path
        .as_deref()
        .map(|path| launch_working_directory(&install_path, path, &installed.source));
    if let (Some(path), Some(cwd)) = (executable_path.as_deref(), working_dir.as_deref()) {
        tracing::info!(
            game_id,
            source = %installed.source,
            executable = %path.display(),
            working_dir = %cwd.display(),
            args = args.as_deref().unwrap_or(""),
            "launching the game"
        );
    }

    let (mut command, tool) = prepare_launch(
        app,
        &installed,
        executable_path.as_deref(),
        working_dir.as_deref(),
        &install_path,
        &prefix,
        args.as_deref(),
        &options,
    )?;

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
        Ok(Err(error)) => tracing::debug!(error = %error, game_id, "save candidate snapshot failed"),
        Err(error) => tracing::debug!(error = %error, game_id, "save candidate snapshot task failed"),
    }

    let child = command.spawn().map_err(|e| match &executable_path {
        Some(path) => format!(
            "Could not launch {} (working directory {}): {e}",
            path.display(),
            working_dir.as_deref().unwrap_or(&install_path).display()
        ),
        None => format!(
            "Could not open {}: {e}",
            installed.launch_uri.as_deref().unwrap_or("the game")
        ),
    })?;
    let pid = child.id();

    let session_id = uuid::Uuid::new_v4().to_string();
    let started_at = chrono::Utc::now();
    let started_stamp = started_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let entry = Arc::new(ProcessEntry {
        child: std::sync::Mutex::new(child),
        pid,
        game_id,
        started_at,
        started: std::time::Instant::now(),
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

    spawn_watcher(
        app.clone(),
        entry,
        session_id.clone(),
        game_id,
        installed.source.clone(),
    );
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

/// Build the process for a launch: URI handler, native executable, or a Linux
/// compatibility tool around a Windows executable (#89).
#[cfg(all(unix, not(target_os = "macos")))]
#[allow(clippy::too_many_arguments)]
fn prepare_launch(
    app: &AppHandle,
    installed: &db::InstalledGame,
    executable: Option<&Path>,
    working_dir: Option<&Path>,
    install_path: &Path,
    prefix: &Path,
    args: Option<&str>,
    options: &crate::linux_gaming::LaunchOptions,
) -> Result<(Command, Option<String>), String> {
    if let Some(uri) = installed
        .launch_uri
        .as_deref()
        .map(str::trim)
        .filter(|uri| !uri.is_empty())
    {
        return Ok((build_uri_command(uri)?, None));
    }

    let executable =
        executable.ok_or_else(|| "No executable was found for this game.".to_string())?;
    let working_dir = working_dir.unwrap_or(install_path);
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    linux_prepare_launch(executable, working_dir, prefix, args, options, &settings)
}

/// Windows and macOS: no compatibility layer.
#[cfg(not(all(unix, not(target_os = "macos"))))]
#[allow(clippy::too_many_arguments)]
fn prepare_launch(
    _app: &AppHandle,
    installed: &db::InstalledGame,
    executable: Option<&Path>,
    working_dir: Option<&Path>,
    install_path: &Path,
    _prefix: &Path,
    args: Option<&str>,
    _options: &crate::linux_gaming::LaunchOptions,
) -> Result<(Command, Option<String>), String> {
    if let Some(uri) = installed
        .launch_uri
        .as_deref()
        .map(str::trim)
        .filter(|uri| !uri.is_empty())
    {
        return Ok((build_uri_command(uri)?, None));
    }

    let executable =
        executable.ok_or_else(|| "No executable was found for this game.".to_string())?;
    let working_dir = working_dir.unwrap_or(install_path);
    Ok((build_command(executable, working_dir, args)?, None))
}

/// Linux: run a Windows executable through the selected compatibility tool.
///
/// The runner receives the resolved executable path in its arguments and the
/// same working directory as a native launch (#160), so a nested adopted
/// executable is found from the Wine/UMU side too.
#[cfg(all(unix, not(target_os = "macos")))]
fn linux_prepare_launch(
    executable: &Path,
    working_dir: &Path,
    prefix: &Path,
    args: Option<&str>,
    options: &crate::linux_gaming::LaunchOptions,
    settings: &crate::settings::LocalSettings,
) -> Result<(Command, Option<String>), String> {
    use crate::compat;
    use crate::linux_gaming::{self, LaunchMode};

    let is_windows_exe = executable
        .extension()
        .map(|extension| extension.eq_ignore_ascii_case("exe"))
        .unwrap_or(false);

    let mode = match options.mode {
        LaunchMode::Auto if is_windows_exe => LaunchMode::Auto,
        LaunchMode::Auto => LaunchMode::Native,
        other => other,
    };

    // Native keeps the historical behaviour: `.sh` through `sh`, `.exe`
    // through Wine when the user explicitly asked for it.
    if mode == LaunchMode::Native {
        return Ok((build_command(executable, working_dir, args)?, None));
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

    let mut command = Command::new(&plan.program);
    command
        .args(&plan.args)
        .current_dir(working_dir)
        .envs(plan.env.iter())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    tracing::info!(
        tool = %plan.tool_name,
        prefix = %plan.prefix.display(),
        working_dir = %working_dir.display(),
        "launching a Windows game through a compatibility tool"
    );
    Ok((command, Some(plan.tool_name)))
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
/// `ludex_core::exe::discover_executables` clamps to 4..=8; the maximum covers
/// GameVault layouts such as
/// `Installation/<edition>/<version>/binaries/x64/game.exe`.
const STALE_EXECUTABLE_SCAN_DEPTH: usize = ludex_core::exe::MAX_SCAN_DEPTH;

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
        return Ok(path);
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
    Ok(canonical)
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
pub(crate) fn launch_working_directory(
    install_path: &Path,
    executable: &Path,
    source: &str,
) -> PathBuf {
    if source == db::SOURCE_ADOPTED {
        if let Some(parent) = executable.parent() {
            if !parent.as_os_str().is_empty() && !same_path(parent, install_path) {
                return parent.to_path_buf();
            }
        }
    }
    install_path.to_path_buf()
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

    let mut command = platform_command(executable)?;
    command.current_dir(working_dir);
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
        let mut command = Command::new("open");
        command.arg(executable);
        Ok(command)
    } else {
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
            let mut command = Command::new(wine);
            command.arg(executable);
            Ok(command)
        }
        "sh" => {
            let mut command = Command::new("sh");
            command.arg(executable);
            Ok(command)
        }
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

fn spawn_watcher(
    app: AppHandle,
    entry: Arc<ProcessEntry>,
    session_id: String,
    game_id: i64,
    source: String,
) {
    std::thread::spawn(move || {
        // Process-tree tracking (#26): the session stays open while any
        // descendant of the launched game is alive, not just the root PID.
        let mut sampler = ProcessSampler::new();
        let mut tracker = TreeTracker::new(entry.pid);
        let mut exit_description: Option<String> = None;

        loop {
            // Reap the root child as soon as it exits so it does not linger as
            // a zombie in the process snapshot (which would look "alive").
            if exit_description.is_none() {
                if let Ok(mut child) = entry.child.lock() {
                    match child.try_wait() {
                        Ok(Some(status)) => exit_description = Some(status.to_string()),
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

        tracing::info!(
            game_id,
            started_at = %entry.started_at,
            status = ?exit_description,
            seconds,
            "game process tree exited"
        );

        let state = app.state::<AppState>();
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
            },
        );

        // Cloud saves (#108): debounce the automatic upload so the game's
        // final writes settle; failures are queued and retried off-thread.
        crate::saves_sync::schedule_after_exit(app.clone(), game_id);

        // Discord presence goes back to idle (#12).
        crate::presence::set_payload(Some(crate::presence::PresencePayload::idle()));

        let sync_app = app.clone();
        let sync_session_id = session_id;
        tauri::async_runtime::spawn(async move {
            if let Err(error) = sync_session(&sync_app, &sync_session_id, game_id, minutes).await {
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

    tracing::info!(session_id, killed, "stopped the game process tree");
    Ok(())
}

pub async fn sync_session(
    app: &AppHandle,
    session_id: &str,
    game_id: i64,
    minutes: i64,
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
        return Err("No server URL is configured.".to_string());
    }

    let mut request = state
        .http
        .post(format!("{base}/api/games/{game_id}/playtime"))
        .timeout(Duration::from_secs(15))
        .json(&serde_json::json!({ "minutes": minutes }));
    if let Some(token) = non_empty(settings.token.as_deref()) {
        request = request.bearer_auth(token);
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("Playtime sync failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Playtime sync failed with HTTP {}",
            response.status()
        ));
    }

    if let Ok(connection) = state.db.lock() {
        let _ = db::mark_session_synced(&connection, session_id);
    }
    Ok(())
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
        if let Err(error) = sync_session(&app, &session.id, session.game_id, session.minutes).await
        {
            tracing::debug!(error = %error, session = %session.id, "playtime still pending");
            // Stop at the first failure: the server is probably unreachable.
            break;
        }
    }
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
}
