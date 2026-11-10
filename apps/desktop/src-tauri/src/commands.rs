//! Tauri commands exposed to the web UI (`docs/CONTRACT.md`, Desktop section).

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::db::{self, GamePlaytime, InstalledGame};
use crate::install::{self, InstallJob, InstallParams};
use crate::launch::{self, LaunchResult};
use crate::maintenance::{self, RepairReport};
use crate::manifest::VerifyReport;
use crate::paths;
use crate::server::{self, ServerInfo, ServerStatus};
use crate::settings::{self, LocalSettings};
use crate::state::AppState;
use crate::updater::{self, UpdateStatus};
use crate::updates::{self, GameUpdate};
use crate::util::merge_json;

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

    tokio::task::spawn_blocking(move || paths::remove_installed_dir(&install_root, &install_path))
        .await
        .map_err(|e| format!("The uninstall task failed: {e}"))??;

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
