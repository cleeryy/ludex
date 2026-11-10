//! Tauri commands exposed to the web UI (`docs/CONTRACT.md`, Desktop section).

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::db::{self, GamePlaytime, InstalledGame};
use crate::install::{self, InstallJob, InstallParams};
use crate::installer::{self, AttemptReport};
use crate::launch::{self, LaunchResult};
use crate::maintenance::{self, RepairReport};
use crate::manifest::VerifyReport;
use crate::paths;
use crate::presence::{self, PresenceStatus};
use crate::server::{self, ServerInfo, ServerStatus};
use crate::settings::{self, LocalSettings};
use crate::state::AppState;
use crate::steam::{self, SteamGame, SteamScan};
use crate::updater::{self, UpdateStatus};
use crate::updates::{self, GameUpdate};
use crate::util::{merge_json, now_rfc3339};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskUsage {
    pub total_bytes: u64,
    pub free_bytes: u64,
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<LocalSettings, String> {
    Ok(state.settings_snapshot())
}

#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: Value) -> Result<LocalSettings, String> {
    if !settings.is_object() {
        return Err("Settings must be a JSON object.".to_string());
    }

    let current = state.settings_snapshot();
    let mut merged = serde_json::to_value(&current)
        .map_err(|e| format!("Could not read the current settings: {e}"))?;
    merge_json(&mut merged, settings);

    let mut next: LocalSettings =
        serde_json::from_value(merged).map_err(|e| format!("Invalid settings: {e}"))?;
    next.normalize(&state.app_data);
    settings::validate(&next)?;
    settings::ensure_dirs(&next)?;
    settings::save(&state.settings_path, &next)?;
    state.set_settings(next.clone())?;
    presence::configure(next.discord_presence, &next.discord_client_id);

    Ok(next)
}

#[tauri::command]
pub async fn pick_folder(app: AppHandle, title: Option<String>) -> Result<Option<String>, String> {
    let initial = app.state::<AppState>().settings_snapshot().games_dir;
    let title = title.unwrap_or_else(|| "Choose a folder".to_string());

    let (sender, receiver) = tokio::sync::oneshot::channel();
    let mut dialog = app.dialog().file().set_title(title);
    if let Some(directory) = initial.filter(|value| PathBuf::from(value).is_dir()) {
        dialog = dialog.set_directory(directory);
    }
    dialog.pick_folder(move |path| {
        let _ = sender.send(path);
    });

    let picked = receiver
        .await
        .map_err(|_| "The folder dialog closed unexpectedly.".to_string())?;

    match picked {
        Some(file_path) => {
            let path = file_path
                .into_path()
                .map_err(|e| format!("Invalid folder path: {e}"))?;
            Ok(Some(path.to_string_lossy().to_string()))
        }
        None => Ok(None),
    }
}

#[tauri::command]
pub async fn local_server_start(app: AppHandle) -> Result<ServerInfo, String> {
    server::start_embedded(&app).await
}

#[tauri::command]
pub async fn local_server_stop(app: AppHandle) -> Result<(), String> {
    server::stop_embedded(&app).await
}

#[tauri::command]
pub async fn local_server_status(state: State<'_, AppState>) -> Result<ServerStatus, String> {
    Ok(server::status(&state).await)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn install_game(
    app: AppHandle,
    game_id: i64,
    version_id: i64,
    server_url: String,
    title: String,
    version: Option<String>,
    executable: Option<String>,
    install_dir: Option<String>,
    download_dir: Option<String>,
    stream: Option<bool>,
    token: Option<String>,
) -> Result<InstallJob, String> {
    install::begin(
        &app,
        InstallParams {
            game_id,
            version_id,
            server_url,
            token,
            title,
            version,
            executable,
            install_dir,
            download_dir,
            stream,
            replace_path: None,
        },
    )
}

#[tauri::command]
pub fn cancel_install(app: AppHandle, job_id: String) -> Result<(), String> {
    install::cancel(&app, &job_id)
}

#[tauri::command]
pub fn pause_install(app: AppHandle, job_id: String) -> Result<InstallJob, String> {
    install::pause(&app, &job_id)
}

#[tauri::command]
pub fn resume_install(app: AppHandle, job_id: String) -> Result<InstallJob, String> {
    install::resume(&app, &job_id)
}

#[tauri::command]
pub fn get_install_job(app: AppHandle, job_id: String) -> Result<InstallJob, String> {
    match install::job(&app, &job_id) {
        Ok(job) => Ok(job),
        Err(not_in_memory) => {
            // Fall back to the persisted row: jobs from a previous app run
            // (including interrupted downloads) stay visible.
            let state = app.state::<AppState>();
            let connection = state
                .db
                .lock()
                .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
            match db::get_install_job(&connection, &job_id)? {
                Some(record) => Ok(InstallJob::from_record(record)),
                None => Err(not_in_memory),
            }
        }
    }
}

/// All install jobs (active + persisted, newest first) so the UI can show the
/// queue and offer to resume interrupted jobs after a restart.
#[tauri::command]
pub fn list_install_jobs(app: AppHandle) -> Result<Vec<InstallJob>, String> {
    install::list(&app)
}

#[tauri::command]
pub fn list_installed(state: State<'_, AppState>) -> Result<Vec<InstalledGame>, String> {
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    db::list_installed(&connection)
}

#[tauri::command]
pub async fn launch_game(
    app: AppHandle,
    game_id: i64,
    executable: Option<String>,
    args: Option<String>,
) -> Result<LaunchResult, String> {
    launch::launch(&app, game_id, executable, args).await
}

#[tauri::command]
pub fn stop_game(app: AppHandle, session_id: String) -> Result<(), String> {
    launch::stop(&app, &session_id)
}

#[tauri::command]
pub async fn uninstall_game(app: AppHandle, game_id: i64) -> Result<(), String> {
    let state = app.state::<AppState>();

    let installed = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::get_installed(&connection, game_id)?
    };
    let installed = installed.ok_or_else(|| format!("Game {game_id} is not installed."))?;

    {
        let processes = state
            .processes
            .lock()
            .map_err(|_| "The process list is unavailable; restart Ludex.".to_string())?;
        if processes.values().any(|entry| entry.game_id == game_id) {
            return Err("Stop the game before uninstalling it.".to_string());
        }
    }

    let install_root = installed
        .install_root
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| state.settings_snapshot().install_path());
    let install_path = PathBuf::from(&installed.install_path);

    // Steam imports (#13) live in Steam's folder: removing the index entry must
    // never delete game files Ludex does not own.
    let external = installed.source == db::SOURCE_STEAM
        || installed
            .launch_uri
            .as_deref()
            .map(|uri| !uri.trim().is_empty())
            .unwrap_or(false);

    if !external {
        tokio::task::spawn_blocking(move || {
            paths::remove_installed_dir(&install_root, &install_path)
        })
        .await
        .map_err(|e| format!("The uninstall task failed: {e}"))??;
    }

    {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::remove_installed(&connection, game_id)?;
        db::remove_manifest(&connection, game_id)?;
    }

    Ok(())
}

/// Move an installed game to another install root. Refuses while the game is
/// running or an install for it is active.
#[tauri::command]
pub async fn move_install(
    app: AppHandle,
    game_id: i64,
    target_dir: String,
) -> Result<InstalledGame, String> {
    maintenance::move_install(&app, game_id, target_dir).await
}

/// Compare the installed files with their manifest.
#[tauri::command]
pub async fn verify_install(app: AppHandle, game_id: i64) -> Result<VerifyReport, String> {
    maintenance::verify_install(&app, game_id).await
}

/// Restore missing/corrupt files from the version archive.
#[tauri::command]
pub async fn repair_install(app: AppHandle, game_id: i64) -> Result<RepairReport, String> {
    maintenance::repair_install(&app, game_id).await
}

#[tauri::command]
pub fn open_path(app: AppHandle, path: String) -> Result<(), String> {
    let target = PathBuf::from(&path);
    if !target.exists() {
        return Err(format!("Path does not exist: {path}"));
    }
    app.opener()
        .open_path(target.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| format!("Could not open {path}: {e}"))
}

/// Recently launched installed games, newest first (Home + tray).
#[tauri::command]
pub fn list_recent_games(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<InstalledGame>, String> {
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    db::recent_installed(&connection, limit.unwrap_or(8).clamp(1, 50))
}

/// Aggregated playtime for one game: totals, per-day buckets and sessions.
#[tauri::command]
pub fn get_game_playtime(
    state: State<'_, AppState>,
    game_id: i64,
    days: Option<i64>,
    sessions: Option<i64>,
) -> Result<GamePlaytime, String> {
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    db::game_playtime(
        &connection,
        game_id,
        days.unwrap_or(30),
        sessions.unwrap_or(10),
    )
}

/// Installed games with a newer version on the server (#30).
#[tauri::command]
pub async fn check_game_updates(app: AppHandle) -> Result<Vec<GameUpdate>, String> {
    updates::check(&app).await
}

/// Install the newest version of a game, replacing the current install.
#[tauri::command]
pub async fn update_game(app: AppHandle, game_id: i64) -> Result<InstallJob, String> {
    updates::update_game(&app, game_id).await
}

/// Check for an application update (#16).
#[tauri::command]
pub async fn check_for_updates(
    app: AppHandle,
    manual: Option<bool>,
) -> Result<UpdateStatus, String> {
    updater::check_now(app, manual.unwrap_or(true)).await
}

/// Download and install the available application update.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<UpdateStatus, String> {
    updater::install(app).await
}

/// Restart Ludex (after an update installed on Linux/macOS).
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    updater::restart(&app);
}

#[tauri::command]
pub fn get_disk_usage(path: String) -> Result<DiskUsage, String> {
    let target = PathBuf::from(&path);
    let existing =
        paths::nearest_existing(&target).ok_or_else(|| format!("Path does not exist: {path}"))?;

    let total_bytes = fs2::total_space(&existing)
        .map_err(|e| format!("Could not read disk usage for {}: {e}", existing.display()))?;
    let free_bytes = fs2::available_space(&existing)
        .map_err(|e| format!("Could not read free space for {}: {e}", existing.display()))?;

    Ok(DiskUsage {
        total_bytes,
        free_bytes,
    })
}

/* ------------------------- installers (#7) ------------------------------ */

/// What `run_installer` would do, shown before anything is executed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallerPlan {
    pub executable: Option<String>,
    /// Installer-looking executables found in the install folder.
    pub detected: Vec<String>,
    pub attempts: Vec<Vec<String>>,
    pub timeout_secs: u64,
    pub interactive: bool,
    pub platform: String,
}

/// Outcome of a silent or interactive installer run.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallerReport {
    pub game_id: i64,
    pub executable: String,
    pub success: bool,
    pub cancelled: bool,
    pub timed_out: bool,
    pub interactive: bool,
    pub exit_code: Option<i32>,
    pub attempts: Vec<AttemptReport>,
    /// Executable recorded in the index after a successful install.
    pub recorded_executable: Option<String>,
    /// Programs that look launchable after the installer ran, for the picker.
    pub candidates: Vec<String>,
    pub message: String,
}

fn installed_or_error(state: &AppState, game_id: i64) -> Result<InstalledGame, String> {
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    db::get_installed(&connection, game_id)?
        .ok_or_else(|| format!("Game {game_id} is not installed on this device."))
}

fn installer_candidates(installed: &InstalledGame) -> Vec<String> {
    installer::detect_installers(&PathBuf::from(&installed.install_path))
}

/// Preview the silent install plan (#7) without running anything.
#[tauri::command]
pub fn installer_plan(
    state: State<'_, AppState>,
    game_id: Option<i64>,
    executable: Option<String>,
) -> Result<InstallerPlan, String> {
    let settings = state.settings_snapshot();
    let detected = match game_id {
        Some(game_id) => installer_candidates(&installed_or_error(&state, game_id)?),
        None => Vec::new(),
    };
    let requested = executable
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let executable = requested.or_else(|| {
        if detected.len() == 1 {
            Some(detected[0].clone())
        } else {
            None
        }
    });

    Ok(InstallerPlan {
        executable,
        detected,
        attempts: installer::plan_attempts(&settings.installer_silent_args),
        timeout_secs: settings.installer_timeout_secs,
        interactive: false,
        platform: std::env::consts::OS.to_string(),
    })
}

/// Run a game's installer with silent switches, then record its executable.
///
/// Only ever called from an explicit user action. When every silent attempt
/// fails, the report asks the UI to offer an interactive run (`interactive`).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn run_installer(
    app: AppHandle,
    game_id: i64,
    executable: Option<String>,
    interactive: Option<bool>,
    args: Option<Vec<String>>,
) -> Result<InstallerReport, String> {
    let interactive = interactive.unwrap_or(false);
    let state = app.state::<AppState>();
    let installed = installed_or_error(&state, game_id)?;
    let install_root = PathBuf::from(&installed.install_path);
    if !install_root.is_dir() {
        return Err(format!(
            "The install folder is missing: {}. Reinstall the game.",
            install_root.display()
        ));
    }

    let current = state.settings_snapshot();
    let candidates = installer_candidates(&installed);
    let requested = executable
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            if candidates.len() == 1 {
                Some(candidates[0].clone())
            } else {
                None
            }
        });

    let requested = requested.ok_or_else(|| {
        if candidates.is_empty() {
            "No installer was found in the install folder. Choose the setup program first."
                .to_string()
        } else {
            format!(
                "Several installers were found ({}). Choose the one to run.",
                candidates.join(", ")
            )
        }
    })?;
    let installer_path = installer::resolve_installer(&install_root, &requested)?;

    let attempts: Vec<Vec<String>> = if interactive {
        vec![Vec::new()]
    } else if let Some(args) = args.filter(|args| !args.is_empty()) {
        vec![args]
    } else {
        installer::plan_attempts(&current.installer_silent_args)
    };
    let timeout = if interactive {
        std::time::Duration::ZERO
    } else {
        std::time::Duration::from_secs(current.installer_timeout_secs)
    };

    // Silent installers drive Windows setup programs; elsewhere the plan stays
    // available but nothing is executed.
    if !interactive && !cfg!(windows) {
        return Err(
            "Silent installer runs are Windows-only. Run the installer manually on this platform."
                .to_string(),
        );
    }

    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let mut running = state
            .installers
            .lock()
            .map_err(|_| "The installer registry is unavailable; restart Ludex.".to_string())?;
        if running.contains_key(&game_id) {
            return Err("An installer is already running for this game.".to_string());
        }
        running.insert(game_id, std::sync::Arc::clone(&cancel));
    }

    let base = installer::Invocation {
        executable: installer_path.clone(),
        args: Vec::new(),
        working_dir: install_root.clone(),
    };
    let report = tokio::task::spawn_blocking(move || {
        installer::run_plan(&installer::SystemRunner, &base, &attempts, timeout, &cancel)
    })
    .await;

    if let Ok(mut running) = state.installers.lock() {
        running.remove(&game_id);
    }

    let report = report.map_err(|error| format!("The installer task failed: {error}"))?;
    let exit_code = report.last_exit_code();
    let timed_out = report.timed_out();

    // After a successful install, record the game binary the installer left
    // behind (never the installer itself).
    let mut recorded_executable = None;
    if report.success {
        let picked =
            crate::bridge::pick_executable(&install_root, &installed.title).filter(|path| {
                path != &installer_path
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| !installer::is_installer_name(name))
                        .unwrap_or(false)
            });
        if let Some(path) = picked {
            let value = path.to_string_lossy().to_string();
            let connection = state
                .db
                .lock()
                .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
            db::set_executable(&connection, game_id, Some(&value))?;
            recorded_executable = Some(value);
        }
    }

    let candidates = installer_candidates(&installed);
    let message = if report.cancelled {
        "Installer cancelled.".to_string()
    } else if report.success {
        match &recorded_executable {
            Some(path) => format!(
                "Installer finished (exit code {}). Recorded {} as the game executable.",
                exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "n/a".into()),
                path
            ),
            None => "Installer finished. Choose the game executable to launch it.".to_string(),
        }
    } else if timed_out {
        format!(
            "The installer did not finish within {} seconds. Run it interactively to see what it is waiting for.",
            current.installer_timeout_secs
        )
    } else if interactive {
        format!(
            "The installer exited with code {}. Choose the game executable to launch it.",
            exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".into())
        )
    } else {
        "Every silent switch was rejected. Run the installer interactively, or set the arguments in the desktop settings.".to_string()
    };

    let _ = app.emit(
        "installer://done",
        serde_json::json!({
            "gameId": game_id,
            "success": report.success,
            "exitCode": exit_code,
        }),
    );

    Ok(InstallerReport {
        game_id,
        executable: installer_path.to_string_lossy().to_string(),
        success: report.success,
        cancelled: report.cancelled,
        timed_out,
        interactive,
        exit_code,
        attempts: report.attempts,
        recorded_executable,
        candidates,
        message,
    })
}

/// Ask a running installer to stop.
#[tauri::command]
pub fn cancel_installer(state: State<'_, AppState>, game_id: i64) -> Result<(), String> {
    let running = state
        .installers
        .lock()
        .map_err(|_| "The installer registry is unavailable; restart Ludex.".to_string())?;
    let flag = running
        .get(&game_id)
        .ok_or_else(|| "No installer is running for this game.".to_string())?;
    flag.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// Record the executable the user picked for an installed game.
#[tauri::command]
pub fn set_executable(
    state: State<'_, AppState>,
    game_id: i64,
    executable: String,
) -> Result<InstalledGame, String> {
    let installed = installed_or_error(&state, game_id)?;
    let root = PathBuf::from(&installed.install_path);
    let candidate = PathBuf::from(executable.trim());
    let full = if candidate.is_absolute() {
        candidate
    } else {
        root.join(candidate)
    };
    if !full.is_file() {
        return Err(format!("That file does not exist: {}", full.display()));
    }
    if !paths::is_inside(&root, &full) {
        return Err(format!(
            "Refusing to use {} because it is outside the install folder.",
            full.display()
        ));
    }

    let value = full.to_string_lossy().to_string();
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    db::set_executable(&connection, game_id, Some(&value))?;
    drop(connection);

    let mut updated = installed;
    updated.executable = Some(value);
    Ok(updated)
}

/// Pick an executable with the system file dialog and record it.
#[tauri::command]
pub async fn choose_executable(
    app: AppHandle,
    game_id: i64,
) -> Result<Option<InstalledGame>, String> {
    let (install_root, title) = {
        let state = app.state::<AppState>();
        let installed = installed_or_error(&state, game_id)?;
        (
            PathBuf::from(&installed.install_path),
            installed.title.clone(),
        )
    };
    if !install_root.is_dir() {
        return Err(format!(
            "The install folder is missing: {}",
            install_root.display()
        ));
    }

    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title(format!("Choose the {title} executable"))
        .set_directory(&install_root)
        .add_filter("Programs", &["exe", "sh", "AppImage", "bat", "cmd"])
        .pick_file(move |path| {
            let _ = sender.send(path);
        });

    let picked = receiver
        .await
        .map_err(|_| "The file dialog closed unexpectedly.".to_string())?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let path = picked
        .into_path()
        .map_err(|error| format!("Invalid file path: {error}"))?;
    if !paths::is_inside(&install_root, &path) {
        return Err("Choose a file inside the game's install folder.".to_string());
    }

    let state = app.state::<AppState>();
    let installed = installed_or_error(&state, game_id)?;
    let value = path.to_string_lossy().to_string();
    {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::set_executable(&connection, game_id, Some(&value))?;
    }
    let mut updated = installed;
    updated.executable = Some(value);
    Ok(Some(updated))
}

/* --------------------- Discord Rich Presence (#12) ---------------------- */

/// Current Discord presence state (never blocks on the IPC socket).
#[tauri::command]
pub fn presence_status() -> PresenceStatus {
    presence::status()
}

/* ----------------------- Steam import (#13) ----------------------------- */

/// One Steam game, enriched with the server match and whether it is imported.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SteamGamePreview {
    #[serde(flatten)]
    pub game: SteamGame,
    pub matched_game_id: Option<i64>,
    pub matched_title: Option<String>,
    pub imported: bool,
    /// Local id used when no server game matches (negative app id).
    pub local_game_id: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SteamPreview {
    pub roots: Vec<String>,
    pub games: Vec<SteamGamePreview>,
    pub warnings: Vec<String>,
    /// False when the server could not be listed, so nothing is pre-matched.
    pub matched: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SteamSkip {
    pub app_id: u32,
    pub title: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SteamImportReport {
    pub imported: Vec<InstalledGame>,
    pub skipped: Vec<SteamSkip>,
}

/// Games already imported from Steam, by app id.
fn imported_steam_apps(state: &AppState) -> Result<Vec<u32>, String> {
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    let games = db::list_installed_by_source(&connection, db::SOURCE_STEAM)?;
    Ok(games
        .iter()
        .filter_map(|game| game.launch_uri.as_deref().and_then(steam::app_id_from_uri))
        .collect())
}

/// Every server game id and title, paged, for title matching.
async fn server_game_titles(state: &AppState) -> Result<Vec<(i64, String)>, String> {
    let settings = state.settings_snapshot();
    let base = settings.server_url.trim_end_matches('/');
    if base.is_empty() {
        return Err("No server URL is configured.".to_string());
    }

    let mut games = Vec::new();
    for page in 1..=10 {
        let mut request = state
            .http
            .get(format!("{base}/api/games?limit=200&page={page}"))
            .timeout(std::time::Duration::from_secs(20));
        if let Some(token) = crate::util::non_empty(settings.token.as_deref()) {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|error| format!("Could not reach the server: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "The server returned HTTP {} while listing games.",
                response.status()
            ));
        }
        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|error| format!("Invalid game list from the server: {error}"))?;
        let items = value["items"].as_array().cloned().unwrap_or_default();
        for item in &items {
            if let (Some(id), Some(title)) = (item["id"].as_i64(), item["title"].as_str()) {
                games.push((id, title.to_string()));
            }
        }
        let total = value["total"].as_i64().unwrap_or(0);
        if items.is_empty() || page * 200 >= total {
            break;
        }
    }
    Ok(games)
}

async fn steam_preview(app: &AppHandle) -> Result<SteamPreview, String> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let roots = steam::discover_roots(Some(settings.steam_root.as_str()));

    let scan: SteamScan = tokio::task::spawn_blocking(move || steam::scan(&roots))
        .await
        .map_err(|error| format!("The Steam scan failed: {error}"))?;

    let imported = imported_steam_apps(&state)?;
    let (server_games, matched) = match server_game_titles(&state).await {
        Ok(games) => (games, true),
        Err(error) => {
            tracing::debug!(error = %error, "steam matching without the server");
            (Vec::new(), false)
        }
    };

    let titles: Vec<(i64, String)> = server_games.clone();
    let matched_ids = steam::match_titles(&scan.games, &titles);
    let titles_by_id: std::collections::HashMap<i64, String> = server_games.into_iter().collect();

    let games = scan
        .games
        .into_iter()
        .map(|game| {
            let matched_game_id = matched_ids.get(&game.app_id).copied();
            SteamGamePreview {
                matched_title: matched_game_id.and_then(|id| titles_by_id.get(&id).cloned()),
                matched_game_id,
                imported: imported.contains(&game.app_id),
                local_game_id: matched_game_id.unwrap_or_else(|| steam::local_game_id(game.app_id)),
                game,
            }
        })
        .collect();

    Ok(SteamPreview {
        roots: scan.roots,
        games,
        warnings: scan.warnings,
        matched,
    })
}

/// List installed Steam games with their Ludex match (#13).
#[tauri::command]
pub async fn steam_scan(app: AppHandle) -> Result<SteamPreview, String> {
    steam_preview(&app).await
}

/// Import selected Steam games into the local index.
///
/// Nothing is downloaded: each entry launches through Steam.
#[tauri::command]
pub async fn steam_import(app: AppHandle, app_ids: Vec<u32>) -> Result<SteamImportReport, String> {
    if app_ids.is_empty() {
        return Err("Select at least one Steam game to import.".to_string());
    }
    let preview = steam_preview(&app).await?;
    let state = app.state::<AppState>();

    let mut imported = Vec::new();
    let mut skipped = Vec::new();
    let mut used_ids: Vec<i64> = Vec::new();

    for entry in preview
        .games
        .iter()
        .filter(|entry| app_ids.contains(&entry.game.app_id))
    {
        let app_id = entry.game.app_id;
        if entry.imported {
            skipped.push(SteamSkip {
                app_id,
                title: entry.game.name.clone(),
                reason: "Already imported from Steam.".to_string(),
            });
            continue;
        }

        let game_id = entry.local_game_id;
        if used_ids.contains(&game_id) {
            skipped.push(SteamSkip {
                app_id,
                title: entry.game.name.clone(),
                reason: "Another selected game matches the same Ludex entry.".to_string(),
            });
            continue;
        }

        // A Ludex-managed install of the same server game wins: importing it
        // would replace that row (same primary key).
        let existing = {
            let connection = state
                .db
                .lock()
                .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
            db::get_installed(&connection, game_id)?
        };
        if let Some(existing) = existing {
            skipped.push(SteamSkip {
                app_id,
                title: entry.game.name.clone(),
                reason: format!("{} is already installed through Ludex.", existing.title),
            });
            continue;
        }

        let record = InstalledGame {
            game_id,
            version_id: 0,
            title: entry
                .matched_title
                .clone()
                .unwrap_or_else(|| entry.game.name.clone()),
            version: None,
            install_path: entry.game.install_dir.clone(),
            executable: None,
            size_bytes: i64::try_from(entry.game.size_bytes).unwrap_or(i64::MAX),
            installed_at: now_rfc3339(),
            last_launched_at: None,
            archive_hash: None,
            install_root: Some(entry.game.library_path.clone()),
            source: db::SOURCE_STEAM.to_string(),
            launch_uri: Some(steam::launch_uri(app_id)),
        };

        {
            let connection = state
                .db
                .lock()
                .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
            db::upsert_installed(&connection, &record)?;
        }
        used_ids.push(game_id);
        imported.push(record);
    }

    if imported.is_empty() && skipped.is_empty() {
        return Err("None of the selected games are installed by Steam any more.".to_string());
    }

    crate::tray::refresh_menu(&app);
    Ok(SteamImportReport { imported, skipped })
}
