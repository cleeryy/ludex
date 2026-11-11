//! Tauri commands exposed to the web UI (`docs/CONTRACT.md`, Desktop section).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::chrome;
use crate::compat::{self, CompatScan};
use crate::config_adapters::{self, AdapterReport};
use crate::db::{self, GamePlaytime, InstalledGame};
use crate::game_config::{self, ConfigDocument, ConfigEditInput};
use crate::install::{self, InstallJob, InstallParams};
use crate::installer::{self, AttemptReport};
use crate::launchers::{self, DiscoveredGame, ImportEntry, Launcher, LauncherScan};
use crate::launch::{self, LaunchResult};
use crate::linux_gaming::{self, LaunchMode, LaunchOptions};
use crate::locations::{self, LocationStatus};
use crate::maintenance::{self, RepairReport};
use crate::manifest::VerifyReport;
use crate::paths;
use crate::prefix::{self, PrefixInfo, WinetricksReport};
use crate::presence::{self, PresenceStatus};
use crate::protondb::{self, ProtonDbRating};
use crate::saves_detect::{self, SavePathInput, SavePaths};
use crate::saves_sync::{self, CloudSaveStatus, RestoreOutcome, SyncToggleResponse};
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
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Value,
) -> Result<LocalSettings, String> {
    if !settings.is_object() {
        return Err("Settings must be a JSON object.".to_string());
    }

    let current = state.settings_snapshot();
    let mut merged = serde_json::to_value(&current)
        .map_err(|e| format!("Could not read the current settings: {e}"))?;

    // An older web bundle still sends the two single folders (#99). Map them
    // onto the default location instead of letting `normalize` discard them.
    let mut patch = settings;
    let has_locations = patch.get("locations").is_some();
    let install_legacy = patch
        .as_object_mut()
        .and_then(|object| object.remove("installDir"))
        .and_then(|value| value.as_str().map(str::to_string));
    let download_legacy = patch
        .as_object_mut()
        .and_then(|object| object.remove("downloadDir"))
        .and_then(|value| value.as_str().map(str::to_string));
    if has_locations {
        // The new list is authoritative; ignore half-updated legacy fields.
    }
    merge_json(&mut merged, patch);
    if !has_locations && (install_legacy.is_some() || download_legacy.is_some()) {
        settings::apply_legacy_folder_patch(
            &mut merged,
            install_legacy.as_deref(),
            download_legacy.as_deref(),
        );
    }

    let mut next: LocalSettings =
        serde_json::from_value(merged).map_err(|e| format!("Invalid settings: {e}"))?;
    next.normalize(&state.app_data);
    settings::validate(&next)?;
    settings::ensure_dirs(&next)?;
    settings::save(&state.settings_path, &next)?;
    state.set_settings(next.clone())?;
    presence::configure(next.discord_presence, &next.discord_client_id);
    // The title bar preference is saved with the rest of the form; apply it so
    // the window never needs a restart (#111).
    chrome::apply(&app, next.native_title_bar);

    Ok(next)
}

/// Switch between the custom title bar and the native one (#111).
///
/// Applies immediately and persists right away: the Appearance toggle must not
/// depend on the "Save desktop settings" button, which lives in another section.
#[tauri::command]
pub fn set_native_title_bar(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<LocalSettings, String> {
    let next = state.update_settings(|settings| settings.native_title_bar = enabled)?;
    chrome::apply(&app, next.native_title_bar);
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
    location_id: Option<String>,
    download_location_id: Option<String>,
    expected_bytes: Option<u64>,
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
            install_location_id: location_id,
            download_location_id,
            expected_bytes,
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

/// Terminal phases whose Downloads history entry can be removed (#100).
/// Active and paused jobs keep their queue slot and their `.part` offset, so
/// their index row must survive until they finish or are cancelled.
fn is_removable_history_phase(phase: install::InstallPhase) -> bool {
    matches!(
        phase,
        install::InstallPhase::Done
            | install::InstallPhase::Cancelled
            | install::InstallPhase::Error
    )
}

/// Tell the UI that the install job list changed after a history deletion.
fn emit_install_jobs(app: &AppHandle) {
    match install::list(app) {
        Ok(jobs) => {
            let _ = app.emit("install://jobs", jobs);
        }
        Err(error) => tracing::warn!(error = %error, "could not emit the install job list"),
    }
    // The tray tooltip reports active downloads; keep it in sync.
    crate::tray::refresh_tooltip(app);
}

/// Remove one finished job from Downloads history (#100).
///
/// Only the SQLite index row goes: install folders, archives and `.part` files
/// are never touched. A job still tracked by the running app is refused while
/// it is queued, downloading, extracting, finalizing or paused.
#[tauri::command]
pub fn delete_install_job(app: AppHandle, job_id: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let in_memory = {
        let jobs = state
            .jobs
            .lock()
            .map_err(|_| "The install queue is unavailable; restart Ludex.".to_string())?;
        jobs.get(&job_id).cloned()
    };

    if let Some(entry) = &in_memory {
        let phase = entry.snapshot().phase;
        if !is_removable_history_phase(phase) {
            return Err(match phase {
                install::InstallPhase::Paused => {
                    "This install is paused. Cancel it before removing it from history.".to_string()
                }
                _ => "This install is still running. Cancel it before removing it from history."
                    .to_string(),
            });
        }
    }

    let deleted = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::delete_install_job(&connection, &job_id)?
    };
    // A job the current run tracks but never persisted (or whose row was
    // already removed) can still be dropped from memory; anything unknown is a
    // stale UI entry and gets a clear error.
    if !deleted && in_memory.is_none() {
        return Err(format!("Install job {job_id} is not in Downloads history."));
    }

    if in_memory.is_some() {
        let mut jobs = state
            .jobs
            .lock()
            .map_err(|_| "The install queue is unavailable; restart Ludex.".to_string())?;
        jobs.remove(&job_id);
    }

    emit_install_jobs(&app);
    Ok(())
}

/// Remove every finished job from Downloads history (#100).
///
/// Only terminal rows (`done`, `cancelled`, `error`, `interrupted`) go;
/// install folders, archives and `.part` files are kept. Returns the number of
/// rows removed.
#[tauri::command]
pub fn clear_finished_install_jobs(app: AppHandle) -> Result<usize, String> {
    let state = app.state::<AppState>();
    let cleared = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::clear_finished_install_jobs(&connection)?
    };

    // Mirror the deletion in the in-memory registry, otherwise
    // `list_install_jobs` would keep returning terminal jobs whose row is gone.
    {
        let mut jobs = state
            .jobs
            .lock()
            .map_err(|_| "The install queue is unavailable; restart Ludex.".to_string())?;
        jobs.retain(|_, entry| !is_removable_history_phase(entry.snapshot().phase));
    }

    emit_install_jobs(&app);
    Ok(cleared)
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
        db::delete_save_paths(&connection, game_id)?;
        db::delete_save_detection(&connection, game_id)?;
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
    /// Linux: the compatibility tool a run would go through (#93).
    pub compat_tool: Option<String>,
    /// Wine prefix the installer would write into.
    pub prefix: Option<String>,
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
    /// Linux compatibility tool used for the run (#93).
    pub compat_tool: Option<String>,
    /// Last lines of the installer log captured on Linux.
    pub log_tail: Option<String>,
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
    let installed = match game_id {
        Some(game_id) => Some(installed_or_error(&state, game_id)?),
        None => None,
    };
    let detected = installed
        .as_ref()
        .map(installer_candidates)
        .unwrap_or_default();
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

    // Linux: report the tool and prefix a run would use (#93).
    let mut compat_tool = None;
    let mut prefix = None;
    if let Some(installed) = &installed {
        if cfg!(all(unix, not(target_os = "macos"))) {
            let options = {
                let connection = state
                    .db
                    .lock()
                    .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
                db::get_launch_options(&connection, installed.game_id)?
            };
            if let Ok(path) = linux_gaming::prefix_for(
                &PathBuf::from(&installed.install_path),
                &options.prefix_path,
            ) {
                prefix = Some(path.to_string_lossy().to_string());
            }
            let scan = compat::scan(&settings);
            let mode = match options.mode {
                LaunchMode::Umu | LaunchMode::Proton => options.mode,
                _ => LaunchMode::Auto,
            };
            compat_tool = linux_gaming::select_tool(&scan.tools, mode, &options.tool_id)
                .ok()
                .map(|selection| selection.name);
        }
    }

    Ok(InstallerPlan {
        executable,
        detected,
        attempts: installer::plan_attempts(&settings.installer_silent_args),
        timeout_secs: settings.installer_timeout_secs,
        interactive: false,
        platform: std::env::consts::OS.to_string(),
        compat_tool,
        prefix,
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
    let options = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::get_launch_options(&connection, game_id)?
    };
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

    // Windows runs the setup program directly. Linux runs it through the same
    // UMU/Proton/Wine tool as the game (#93); macOS has no supported path.
    let mut compatibility_tool: Option<String> = None;
    let mut prefix_used: Option<PathBuf> = None;
    let mut log_path: Option<PathBuf> = None;

    let base = if cfg!(windows) {
        installer::Invocation::direct(installer_path.clone(), install_root.clone())
    } else if cfg!(all(unix, not(target_os = "macos"))) {
        let prefix = linux_gaming::prefix_for(&install_root, &options.prefix_path)?;
        std::fs::create_dir_all(&prefix).map_err(|error| {
            format!(
                "Could not create the Wine prefix {}: {error}",
                prefix.display()
            )
        })?;

        let scan = compat::scan(&current);
        let mode = match options.mode {
            LaunchMode::Umu | LaunchMode::Proton => options.mode,
            _ => LaunchMode::Auto,
        };
        let mut selection = linux_gaming::select_tool(&scan.tools, mode, &options.tool_id)?;
        selection.client_install_path = scan.steam_client_path.as_deref().map(PathBuf::from);
        let plan = linux_gaming::build_launch(&selection, &installer_path, &[], &prefix, &options)?;

        let log = state
            .app_data
            .join("installer-logs")
            .join(format!("{game_id}-{}", uuid::Uuid::new_v4()))
            .with_extension("log");
        compatibility_tool = Some(plan.tool_name.clone());
        prefix_used = Some(prefix);
        log_path = Some(log.clone());

        installer::compat_invocation(&plan, &install_root, Some(log))
    } else {
        return Err(
            "Installer automation is not available on macOS. Run the setup program manually."
                .to_string(),
        );
    };

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
    let log_tail = log_path.as_deref().and_then(prefix::read_tail);

    // After a successful install, record the game binary the installer left
    // behind (never the installer itself). On Linux that is usually inside the
    // Wine prefix, not next to `setup.exe`.
    let mut recorded_executable = None;
    if report.success {
        let picked = installer::detect_installed_executable(
            &install_root,
            prefix_used.as_deref(),
            &installed.title,
        )
        .filter(|path| {
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

    let mut candidates = installer_candidates(&installed);
    if let Some(path) = &recorded_executable {
        let relative = Path::new(path)
            .strip_prefix(&install_root)
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.clone());
        if !candidates.contains(&relative) {
            candidates.push(relative);
        }
    }

    let via = compatibility_tool
        .as_deref()
        .map(|tool| format!(" via {tool}"))
        .unwrap_or_default();
    let message = if report.cancelled {
        "Installer cancelled.".to_string()
    } else if report.success {
        match &recorded_executable {
            Some(path) => format!(
                "Installer finished{via} (exit code {}). Recorded {} as the game executable.",
                exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "n/a".into()),
                path
            ),
            None => format!("Installer finished{via}. Choose the game executable to launch it."),
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
        format!(
            "Every silent switch was rejected{via}. Run the installer interactively, or set the arguments in the desktop settings."
        )
    };

    let _ = app.emit(
        "installer://done",
        serde_json::json!({
            "gameId": game_id,
            "success": report.success,
            "exitCode": exit_code,
            "tool": compatibility_tool,
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
        compat_tool: compatibility_tool,
        log_tail,
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
    let options = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::get_launch_options(&connection, game_id)?
    };
    let prefix = linux_gaming::prefix_for(&root, &options.prefix_path).ok();
    let candidate = PathBuf::from(executable.trim());
    let full = if candidate.is_absolute() {
        candidate
    } else {
        root.join(candidate)
    };
    if !full.is_file() {
        return Err(format!("That file does not exist: {}", full.display()));
    }
    let allowed = paths::is_inside(&root, &full)
        || prefix
            .as_deref()
            .map(|prefix| paths::is_inside(prefix, &full))
            .unwrap_or(false);
    if !allowed {
        return Err(format!(
            "Refusing to use {} because it is outside the install folder and the Wine prefix.",
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

    let state = app.state::<AppState>();
    let installed = installed_or_error(&state, game_id)?;
    let prefix = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        let options = db::get_launch_options(&connection, game_id)?;
        linux_gaming::prefix_for(&install_root, &options.prefix_path).ok()
    };
    let allowed = paths::is_inside(&install_root, &path)
        || prefix
            .as_deref()
            .map(|prefix| paths::is_inside(prefix, &path))
            .unwrap_or(false);
    if !allowed {
        return Err(
            "Choose a file inside the game's install folder or its Wine prefix.".to_string(),
        );
    }

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

/* --------------- Launcher discovery and import (#116) -------------------- */

/// One discovered game enriched with its Ludex match.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherGamePreview {
    #[serde(flatten)]
    pub game: DiscoveredGame,
    pub matched_game_id: Option<i64>,
    pub matched_title: Option<String>,
    pub imported: bool,
    /// Id used when no server game matches.
    pub local_game_id: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherPreview {
    pub roots: Vec<String>,
    pub games: Vec<LauncherGamePreview>,
    pub sources: Vec<launchers::LauncherSource>,
    pub warnings: Vec<String>,
    /// False when the server could not be listed, so nothing is pre-matched.
    pub matched: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherSkip {
    pub source: String,
    pub external_id: String,
    pub title: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherImportReport {
    pub imported: Vec<InstalledGame>,
    pub skipped: Vec<LauncherSkip>,
}

/// Everything a launcher preview and an import need from one scan.
struct LauncherData {
    scan: LauncherScan,
    /// `externalId -> server gameId`, only for unambiguous titles.
    matches: std::collections::BTreeMap<String, i64>,
    /// Server titles by id, used for the preview.
    titles: std::collections::HashMap<i64, String>,
    /// Existing imports by launch URI: `uri -> (gameId, source)`.
    imported: std::collections::HashMap<String, (i64, String)>,
    matched: bool,
}

async fn launcher_data(app: &AppHandle) -> Result<LauncherData, String> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let steam_root = settings.steam_root.clone();

    let scan: LauncherScan = tokio::task::spawn_blocking(move || {
        let roots = launchers::ScanRoots::detect(Some(steam_root.as_str()));
        launchers::scan(&roots)
    })
    .await
    .map_err(|error| format!("The launcher scan failed: {error}"))?;

    let (server_games, matched) = match server_game_titles(&state).await {
        Ok(games) => (games, true),
        Err(error) => {
            tracing::debug!(error = %error, "launcher matching without the server");
            (Vec::new(), false)
        }
    };

    let imported = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::launch_uris(&connection)?
            .into_iter()
            .map(|(uri, game_id, source)| (uri, (game_id, source)))
            .collect()
    };

    let matches = if matched {
        launchers::match_titles(&scan.games, &server_games)
    } else {
        Default::default()
    };
    let server_games_empty = server_games.is_empty();
    let titles: std::collections::HashMap<i64, String> = server_games.into_iter().collect();

    Ok(LauncherData {
        scan,
        matches,
        titles,
        imported,
        matched: matched && !server_games_empty,
    })
}

fn launcher_preview_from(data: LauncherData) -> LauncherPreview {
    let games = data
        .scan
        .games
        .into_iter()
        .map(|game| {
            let matched_game_id = data.matches.get(&game.external_id).copied();
            let imported = game
                .launch_uri
                .as_deref()
                .is_some_and(|uri| data.imported.contains_key(uri));
            let local_game_id = matched_game_id.unwrap_or_else(|| game.local_id());
            LauncherGamePreview {
                matched_title: matched_game_id.and_then(|id| data.titles.get(&id).cloned()),
                matched_game_id,
                imported,
                local_game_id,
                game,
            }
        })
        .collect();
    LauncherPreview {
        roots: data.scan.roots,
        games,
        sources: data.scan.sources,
        warnings: data.scan.warnings,
        matched: data.matched,
    }
}

/// List games installed by Steam, Epic, GOG, EA, Ubisoft, Xbox, itch, Heroic
/// and Lutris, with their Ludex match (#116).
#[tauri::command]
pub async fn launcher_scan(app: AppHandle) -> Result<LauncherPreview, String> {
    let data = launcher_data(&app).await?;
    Ok(launcher_preview_from(data))
}

/// Import the selected launcher entries as external installed games.
///
/// Nothing is downloaded or copied: each row carries the launcher's `launchUri`
/// and the uninstall command only removes the index row.
#[tauri::command]
pub async fn launcher_import(
    app: AppHandle,
    entries: Vec<ImportEntry>,
) -> Result<LauncherImportReport, String> {
    if entries.is_empty() {
        return Err("Select at least one game to import.".to_string());
    }
    let data = launcher_data(&app).await?;
    let state = app.state::<AppState>();

    let mut imported = Vec::new();
    let mut skipped = Vec::new();
    let mut used_ids: Vec<i64> = Vec::new();

    for entry in &entries {
        let target = match launchers::resolve_import(entry, &data.scan.games, &data.matches) {
            Ok(target) => target,
            Err(reason) => {
                skipped.push(LauncherSkip {
                    source: entry.source.clone(),
                    external_id: entry.external_id.clone(),
                    title: entry.external_id.clone(),
                    reason,
                });
                continue;
            }
        };

        if let Some((game_id, source)) = data.imported.get(&target.launch_uri) {
            skipped.push(LauncherSkip {
                source: target.source.id().to_string(),
                external_id: target.external_id.clone(),
                title: target.name.clone(),
                reason: format!(
                    "Already imported from {} as game {game_id}.",
                    Launcher::parse(source).map(Launcher::label).unwrap_or(source.as_str())
                ),
            });
            continue;
        }

        let game_id = target
            .game_id
            .unwrap_or_else(|| launchers::hash_local_id(target.source, &target.external_id));
        if used_ids.contains(&game_id) {
            skipped.push(LauncherSkip {
                source: target.source.id().to_string(),
                external_id: target.external_id.clone(),
                title: target.name.clone(),
                reason: "Another selected game matches the same Ludex entry.".to_string(),
            });
            continue;
        }

        let existing = {
            let connection = state
                .db
                .lock()
                .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
            db::get_installed(&connection, game_id)?
        };
        if let Some(existing) = existing {
            skipped.push(LauncherSkip {
                source: target.source.id().to_string(),
                external_id: target.external_id.clone(),
                title: target.name.clone(),
                reason: if existing.source == db::SOURCE_LUDEX {
                    format!("{} is already installed through Ludex.", existing.title)
                } else {
                    format!(
                        "{} is already imported from {}.",
                        existing.title, existing.source
                    )
                },
            });
            continue;
        }

        let record = InstalledGame {
            game_id,
            version_id: 0,
            title: target.name.clone(),
            version: None,
            install_path: target.install_dir.clone().unwrap_or_default(),
            executable: None,
            size_bytes: i64::try_from(target.size_bytes).unwrap_or(i64::MAX),
            installed_at: now_rfc3339(),
            last_launched_at: None,
            archive_hash: None,
            install_root: None,
            source: target.source.id().to_string(),
            launch_uri: Some(target.launch_uri.clone()),
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
        return Err("None of the selected games are installed by their launcher any more.".to_string());
    }

    crate::tray::refresh_menu(&app);
    Ok(LauncherImportReport { imported, skipped })
}

/* ------------- Linux compatibility (#89, #90, #91, #92, #93) ------------- */

/// Detect Steam roots, Proton builds, UMU, Wine, winetricks and protonfixes.
#[tauri::command]
pub async fn compat_scan(app: AppHandle) -> Result<CompatScan, String> {
    let settings = app.state::<AppState>().settings_snapshot();
    tokio::task::spawn_blocking(move || compat::scan(&settings))
        .await
        .map_err(|error| format!("The compatibility scan failed: {error}"))
}

/// Per-game launch options; defaults when nothing was saved yet.
#[tauri::command]
pub fn get_launch_options(
    state: State<'_, AppState>,
    game_id: i64,
) -> Result<LaunchOptions, String> {
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    let mut options = db::get_launch_options(&connection, game_id)?;
    if !db::has_launch_options(&connection, game_id)? {
        let default = state.settings_snapshot().default_launch_mode;
        if default != LaunchMode::Auto {
            options.mode = default;
        }
    }
    Ok(options)
}

/// Save launch mode, tool, prefix, environment overrides and winetricks verbs.
#[tauri::command]
pub fn set_launch_options(
    state: State<'_, AppState>,
    game_id: i64,
    options: LaunchOptions,
) -> Result<LaunchOptions, String> {
    let installed = installed_or_error(&state, game_id)?;
    let install_root = PathBuf::from(&installed.install_path);

    let mut options = options;
    options.normalize();
    options.env = linux_gaming::validate_env(&options.env)?;
    options.winetricks_verbs = linux_gaming::validate_verbs(&options.winetricks_verbs)?;
    if options.steam_app_id == Some(0) {
        options.steam_app_id = None;
    }

    let prefix = linux_gaming::prefix_for(&install_root, &options.prefix_path)?;
    if !options.prefix_path.trim().is_empty() && prefix == install_root {
        return Err("The Wine prefix cannot be the install folder itself.".to_string());
    }

    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    db::save_launch_options(&connection, game_id, &options, &now_rfc3339())?;
    Ok(options)
}

/// Size and location of a game's Wine prefix (#92).
#[tauri::command]
pub async fn wine_prefix_info(app: AppHandle, game_id: i64) -> Result<PrefixInfo, String> {
    let state = app.state::<AppState>();
    let installed = installed_or_error(&state, game_id)?;
    let install_root = PathBuf::from(&installed.install_path);
    let options = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::get_launch_options(&connection, game_id)?
    };

    tokio::task::spawn_blocking(move || prefix::info(game_id, &install_root, &options))
        .await
        .map_err(|error| format!("Could not inspect the Wine prefix: {error}"))?
}

/// Delete a game's Wine prefix after an explicit confirmation in the UI (#92).
#[tauri::command]
pub async fn reset_wine_prefix(app: AppHandle, game_id: i64) -> Result<(), String> {
    let state = app.state::<AppState>();
    let installed = installed_or_error(&state, game_id)?;
    let install_root = PathBuf::from(&installed.install_path);
    let options = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::get_launch_options(&connection, game_id)?
    };

    {
        let processes = state
            .processes
            .lock()
            .map_err(|_| "The process list is unavailable; restart Ludex.".to_string())?;
        if processes.values().any(|entry| entry.game_id == game_id) {
            return Err("Stop the game before deleting its Wine prefix.".to_string());
        }
    }

    let prefix = linux_gaming::prefix_for(&install_root, &options.prefix_path)?;
    tokio::task::spawn_blocking(move || prefix::remove(&prefix, &install_root))
        .await
        .map_err(|error| format!("Could not delete the Wine prefix: {error}"))?
}

/// Run the stored (or supplied) winetricks verbs in the game prefix (#92).
#[tauri::command]
pub async fn run_winetricks(
    app: AppHandle,
    game_id: i64,
    verbs: Option<Vec<String>>,
) -> Result<WinetricksReport, String> {
    let state = app.state::<AppState>();
    let installed = installed_or_error(&state, game_id)?;
    let install_root = PathBuf::from(&installed.install_path);
    let options = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::get_launch_options(&connection, game_id)?
    };

    let verbs = match verbs.filter(|verbs| !verbs.is_empty()) {
        Some(verbs) => verbs,
        None => options.winetricks_verbs.clone(),
    };
    let verbs = linux_gaming::validate_verbs(&verbs)?;
    if verbs.is_empty() {
        return Err("Choose at least one winetricks verb first.".to_string());
    }

    let settings = state.settings_snapshot();
    let scan = compat::scan(&settings);
    let winetricks = scan.winetricks.clone().ok_or_else(|| {
        "winetricks is not installed. Install the winetricks package, then rescan in Settings → Compatibility."
            .to_string()
    })?;

    let prefix_path = linux_gaming::prefix_for(&install_root, &options.prefix_path)?;
    std::fs::create_dir_all(&prefix_path).map_err(|error| {
        format!(
            "Could not create the Wine prefix {}: {error}",
            prefix_path.display()
        )
    })?;

    let mode = match options.mode {
        LaunchMode::Umu | LaunchMode::Proton => options.mode,
        _ => LaunchMode::Auto,
    };
    let selection = linux_gaming::select_tool(&scan.tools, mode, &options.tool_id)
        .ok()
        .map(|mut selection| {
            selection.client_install_path = scan.steam_client_path.as_deref().map(PathBuf::from);
            selection
        });

    let command = linux_gaming::build_winetricks(
        Path::new(&winetricks),
        &verbs,
        &prefix_path,
        selection.as_ref(),
    )?;

    tokio::task::spawn_blocking(move || {
        prefix::run_winetricks(&command, prefix::WINETRICKS_TIMEOUT)
    })
    .await
    .map_err(|error| format!("The winetricks task failed: {error}"))?
}

fn rating_from_cache(cached: &db::CachedRating, stale: bool, from_cache: bool) -> ProtonDbRating {
    let summary = protondb::Summary {
        tier: cached.tier.clone(),
        best_tier: cached.best_tier.clone(),
        trending_tier: cached.trending_tier.clone(),
        confidence: cached.confidence.clone(),
        total: cached.total,
        score: cached.score,
    };
    ProtonDbRating::from_summary(
        cached.app_id,
        &summary,
        &cached.fetched_at,
        stale,
        from_cache,
    )
}

/// ProtonDB rating for a game (#91), cached for seven days.
///
/// Never fails for network reasons: a stale cache is returned with
/// `status: "stale"`, otherwise `status: "unavailable"`.
#[tauri::command]
pub async fn protondb_rating(
    app: AppHandle,
    game_id: i64,
    app_id: Option<u32>,
    refresh: Option<bool>,
) -> Result<ProtonDbRating, String> {
    let state = app.state::<AppState>();
    let installed = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::get_installed(&connection, game_id)?
    };
    let options = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::get_launch_options(&connection, game_id)?
    };

    let app_id = protondb::resolve_app_id(
        app_id,
        options.steam_app_id,
        installed
            .as_ref()
            .and_then(|game| game.launch_uri.as_deref()),
    );
    let Some(app_id) = app_id else {
        return Ok(ProtonDbRating::no_app_id());
    };

    let cached = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::cached_rating(&connection, app_id)?
    };

    let now = chrono::Utc::now();
    if !refresh.unwrap_or(false) {
        if let Some(cached) = &cached {
            if protondb::cache_is_fresh(&cached.fetched_at, now, protondb::CACHE_TTL_DAYS) {
                return Ok(rating_from_cache(cached, false, true));
            }
        }
    }

    let fetch = state
        .http
        .get(protondb::summary_url(app_id))
        .timeout(Duration::from_secs(8))
        .send()
        .await;

    let response = match fetch {
        Ok(response) => response,
        Err(error) => {
            return Ok(match cached {
                Some(cached) => rating_from_cache(&cached, true, false),
                None => {
                    ProtonDbRating::unavailable(app_id, format!("ProtonDB is unreachable: {error}"))
                }
            })
        }
    };

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(ProtonDbRating::not_found(app_id));
    }
    if !response.status().is_success() {
        return Ok(match cached {
            Some(cached) => rating_from_cache(&cached, true, false),
            None => ProtonDbRating::unavailable(
                app_id,
                format!("ProtonDB returned HTTP {}.", response.status()),
            ),
        });
    }

    let body = match response.text().await {
        Ok(body) => body,
        Err(error) => {
            return Ok(match cached {
                Some(cached) => rating_from_cache(&cached, true, false),
                None => {
                    ProtonDbRating::unavailable(app_id, format!("Could not read ProtonDB: {error}"))
                }
            })
        }
    };

    let summary = match protondb::parse_summary(&body) {
        Ok(summary) => summary,
        Err(error) => {
            return Ok(match cached {
                Some(cached) => rating_from_cache(&cached, true, false),
                None => ProtonDbRating::unavailable(app_id, error),
            })
        }
    };

    let fetched_at = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let record = db::CachedRating {
        app_id,
        tier: summary.tier.clone(),
        best_tier: summary.best_tier.clone(),
        trending_tier: summary.trending_tier.clone(),
        confidence: summary.confidence.clone(),
        total: summary.total,
        score: summary.score,
        fetched_at: fetched_at.clone(),
    };
    {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        if let Err(error) = db::store_rating(&connection, &record) {
            tracing::warn!(error = %error, "could not cache the ProtonDB rating");
        }
    }

    Ok(ProtonDbRating::from_summary(
        app_id,
        &summary,
        &fetched_at,
        false,
        false,
    ))
}

/// Run save detection and persist the resolved mapping (#107).
fn detect_and_persist(app: &AppHandle, game_id: i64) -> Result<SavePaths, String> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();

    let (game, manual, previous_primary, launch_options) = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        let game = db::get_installed(&connection, game_id)?
            .ok_or_else(|| format!("Game {game_id} is not installed on this machine."))?;
        let rows = db::list_save_paths(&connection, game_id)?;
        let previous_primary = rows
            .iter()
            .find(|row| row.primary)
            .map(|row| row.path.clone());
        // Manual and override rows are user-owned: re-detection keeps them.
        let manual = rows
            .into_iter()
            .filter(|row| row.source != saves_detect::SOURCE_MANIFEST)
            .collect::<Vec<_>>();
        let launch_options = db::get_launch_options(&connection, game_id)?;
        (game, manual, previous_primary, launch_options)
    };

    let prefix_override = crate::util::non_empty(Some(launch_options.prefix_path.as_str()));
    let detection = saves_detect::detect_for_game(
        &settings,
        &game,
        &manual,
        prefix_override,
        launch_options.steam_app_id,
    );
    let primary = saves_detect::choose_primary(&detection.saves, previous_primary.as_deref());
    let now = now_rfc3339();
    let rows = saves_detect::records_from_detection(game_id, &detection, primary.as_deref(), &now);
    let record = db::SaveDetectionRecord {
        game_id,
        version_id: game.version_id,
        manifest_version: detection.manifest_version.clone(),
        detected_at: now,
        result_json: serde_json::to_string(&detection)
            .map_err(|error| format!("Could not encode the save detection: {error}"))?,
    };

    {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::replace_save_paths(&connection, game_id, &rows)?;
        db::upsert_save_detection(&connection, &record)?;
    }

    Ok(saves_detect::persisted(game_id, rows, Some(record)))
}

/// Detect this game's save folders (Ludusavi CLI, manifest, then manual paths)
/// and persist the mapping. Paths stay on this machine (#107).
#[tauri::command]
pub async fn detect_save_paths(app: AppHandle, game_id: i64) -> Result<SavePaths, String> {
    tauri::async_runtime::spawn_blocking(move || detect_and_persist(&app, game_id))
        .await
        .map_err(|error| format!("The save detection task failed: {error}"))?
}

/// Re-run detection after a game launch created new save files (#107).
#[tauri::command]
pub async fn redetect_save_paths(app: AppHandle, game_id: i64) -> Result<SavePaths, String> {
    tauri::async_runtime::spawn_blocking(move || detect_and_persist(&app, game_id))
        .await
        .map_err(|error| format!("The save detection task failed: {error}"))?
}

/// Persisted save mapping and last preview. Never rescans the disk (#107).
#[tauri::command]
pub fn get_save_paths(state: State<'_, AppState>, game_id: i64) -> Result<SavePaths, String> {
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    let rows = db::list_save_paths(&connection, game_id)?;
    let detection = db::get_save_detection(&connection, game_id)?;
    Ok(saves_detect::persisted(game_id, rows, detection))
}

/// Replace the per-game manual override list (`manual`) or pin paths over the
/// manifest (`override`). Only one path may be primary (#107).
#[tauri::command]
pub fn set_save_paths(
    state: State<'_, AppState>,
    game_id: i64,
    paths: Vec<SavePathInput>,
) -> Result<SavePaths, String> {
    let settings = state.settings_snapshot();
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    let game = db::get_installed(&connection, game_id)?
        .ok_or_else(|| format!("Game {game_id} is not installed on this machine."))?;
    let previous = db::get_save_detection(&connection, game_id)?;
    let previous_snapshot: Option<saves_detect::Detection> = previous
        .as_ref()
        .and_then(|record| serde_json::from_str(&record.result_json).ok());
    let launch_options = db::get_launch_options(&connection, game_id)?;

    let env = saves_detect::DetectEnv::for_game(
        &settings,
        &game,
        crate::util::non_empty(Some(launch_options.prefix_path.as_str())),
        launch_options.steam_app_id,
    );
    let mut context = saves_detect::GameContext::from_installed(&game);
    if context.steam_app_id.is_none() {
        context.steam_app_id = launch_options.steam_app_id;
    }
    let now = now_rfc3339();
    let (mut rows, mut detection) =
        saves_detect::manual_override(&env, &context, game_id, &paths, &now)?;

    // A user path that matches a previous manifest result is pinned, not a
    // duplicate: the next re-detection must not silently replace it.
    if let Some(snapshot) = &previous_snapshot {
        for row in &mut rows {
            if snapshot
                .saves
                .iter()
                .any(|save| save.path == row.path && save.source == saves_detect::SOURCE_MANIFEST)
            {
                row.source = saves_detect::SOURCE_OVERRIDE.to_string();
            }
        }
        for save in &mut detection.saves {
            if let Some(row) = rows.iter().find(|row| row.path == save.path) {
                save.source = row.source.clone();
            }
        }
    }

    let overrides = rows
        .iter()
        .filter(|row| row.source == saves_detect::SOURCE_OVERRIDE)
        .count();
    let manual = rows.len().saturating_sub(overrides);
    detection.message = if rows.is_empty() {
        "No manual save paths. Run detection to find this game's save folders.".to_string()
    } else if overrides > 0 && manual > 0 {
        format!(
            "{} save path(s) set manually: {manual} custom and {overrides} pinned over the manifest.",
            rows.len()
        )
    } else if overrides > 0 {
        format!("{overrides} save path(s) pinned over the manifest.")
    } else {
        format!("{manual} manually configured save path(s).")
    };
    detection.source = if overrides > 0 {
        saves_detect::SOURCE_OVERRIDE
    } else {
        saves_detect::SOURCE_MANUAL
    }
    .to_string();
    detection.manifest_version = previous
        .as_ref()
        .and_then(|record| record.manifest_version.clone());
    detection.detected = !rows.is_empty();

    db::replace_save_paths(&connection, game_id, &rows)?;
    let record = db::SaveDetectionRecord {
        game_id,
        version_id: game.version_id,
        manifest_version: detection.manifest_version.clone(),
        detected_at: now,
        result_json: serde_json::to_string(&detection)
            .map_err(|error| format!("Could not encode the save paths: {error}"))?,
    };
    db::upsert_save_detection(&connection, &record)?;

    Ok(saves_detect::persisted(game_id, rows, Some(record)))
}

/* ------------------------------------------------------------ cloud saves */

/// Full cloud save state for a game (#108): detected paths, latest snapshot,
/// last upload/restore, effective toggle and conflict. Remote failures are
/// reported in `message` so the local state still renders offline.
#[tauri::command]
pub async fn cloud_save_status(app: AppHandle, game_id: i64) -> Result<CloudSaveStatus, String> {
    saves_sync::status(&app, game_id).await
}

/// Change the global automatic-sync default (`gameId` null) or one game's
/// override. Never touches the server.
#[tauri::command]
pub fn set_cloud_save_sync(
    app: AppHandle,
    game_id: Option<i64>,
    enabled: bool,
) -> Result<SyncToggleResponse, String> {
    saves_sync::set_sync(&app, game_id, enabled)
}

/// Archive the detected save paths and upload them now. Works even when
/// automatic sync is off; an unchanged archive is a no-op.
#[tauri::command]
pub async fn sync_cloud_save_now(app: AppHandle, game_id: i64) -> Result<CloudSaveStatus, String> {
    saves_sync::sync_now(&app, game_id).await
}

/// Restore the newest snapshot (`remote`, with a local backup when saves
/// exist) or keep the local files (`keep_local`).
#[tauri::command]
pub async fn restore_cloud_save(
    app: AppHandle,
    game_id: i64,
    strategy: String,
) -> Result<RestoreOutcome, String> {
    saves_sync::restore(&app, game_id, &strategy).await
}

/// Explicit conflict resolution; same strategies as `restore_cloud_save`.
#[tauri::command]
pub async fn resolve_cloud_save_conflict(
    app: AppHandle,
    game_id: i64,
    strategy: String,
) -> Result<RestoreOutcome, String> {
    saves_sync::resolve_conflict(&app, game_id, &strategy).await
}

/* ------------------------- locations (#99) ------------------------------- */

/// Every named location with live probe data. Never fails: a missing drive is
/// reported in the status, not as an error.
#[tauri::command]
pub fn list_locations(state: State<'_, AppState>) -> Result<Vec<LocationStatus>, String> {
    Ok(locations::picker_order(locations::list(
        &state.settings_snapshot(),
    )))
}

/// Validate one location (by id or path) and optionally check a download size
/// against its free space.
#[tauri::command]
pub fn check_location(
    state: State<'_, AppState>,
    path: Option<String>,
    id: Option<String>,
    required_bytes: Option<u64>,
) -> Result<LocationStatus, String> {
    let settings = state.settings_snapshot();
    let mut status = match (id.as_deref(), path.as_deref()) {
        (Some(id), _) => {
            let location = settings.location(id).ok_or_else(|| {
                format!("The location {id} no longer exists. Refresh Settings → Locations.")
            })?;
            locations::probe(location)
        }
        (None, Some(path)) => locations::probe_path(path)?,
        (None, None) => return Err("Choose a location to check.".to_string()),
    };
    if let Some(required) = required_bytes.filter(|value| *value > 0) {
        status.required_bytes = Some(required);
        match status.free_bytes {
            Some(free) => {
                if let Err(error) = locations::guard_free_space(required, free) {
                    status.free_space_ok = Some(false);
                    status.note = Some(match status.note.take() {
                        Some(note) => format!("{note} {error}"),
                        None => error,
                    });
                } else {
                    status.free_space_ok = Some(true);
                }
            }
            // A missing drive has no free-space reading: report it, but do not
            // turn "unknown" into "too small".
            None => status.free_space_ok = None,
        }
    }
    Ok(status)
}

/* ---------------------- per-game config editor (#105) -------------------- */

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameConfigListing {
    pub game_id: i64,
    pub install_path: String,
    pub files: Vec<game_config::ConfigFileInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigWriteReport {
    pub document: ConfigDocument,
    pub diff: String,
    pub backup: db::ConfigEditRecord,
}

/// Config files inside an install folder (ini/json/cfg/toml). Read-only: the
/// file list includes unparseable files with `editable: false` and a reason.
#[tauri::command]
pub fn list_game_configs(
    state: State<'_, AppState>,
    game_id: i64,
) -> Result<GameConfigListing, String> {
    let installed = installed_or_error(&state, game_id)?;
    let root = PathBuf::from(&installed.install_path);
    if !root.is_dir() {
        return Err(format!(
            "The install folder is missing: {}. Reinstall the game to edit its settings.",
            root.display()
        ));
    }
    Ok(GameConfigListing {
        game_id,
        install_path: installed.install_path.clone(),
        files: game_config::list(&root),
    })
}

/// Parse one config file and return its editable keys. Comments are preserved
/// on write; this command never changes anything.
#[tauri::command]
pub fn read_game_config(
    state: State<'_, AppState>,
    game_id: i64,
    path: String,
) -> Result<ConfigDocument, String> {
    let installed = installed_or_error(&state, game_id)?;
    game_config::read(Path::new(&installed.install_path), &path)
}

/// Apply key/value edits after validating types and backing up the original.
/// `origin` is `manual` (key editor) or `adapter` (typed control).
#[tauri::command]
pub fn write_game_config(
    state: State<'_, AppState>,
    game_id: i64,
    path: String,
    edits: Vec<ConfigEditInput>,
    origin: Option<String>,
) -> Result<ConfigWriteReport, String> {
    let installed = installed_or_error(&state, game_id)?;
    let origin = match origin.as_deref().unwrap_or("manual") {
        "adapter" => "adapter",
        _ => "manual",
    };
    let outcome = game_config::write(
        Path::new(&installed.install_path),
        &state.app_data,
        game_id,
        &path,
        &edits,
        origin,
    )?;
    {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        if let Err(error) = db::insert_config_edit(&connection, &outcome.record) {
            tracing::warn!(error = %error, "could not record the config edit");
        }
    }
    Ok(ConfigWriteReport {
        document: outcome.document,
        diff: outcome.diff,
        backup: outcome.record,
    })
}

/// Restore a previous backup. A safety copy of the current file is taken first.
#[tauri::command]
pub fn restore_game_config(
    state: State<'_, AppState>,
    game_id: i64,
    backup_id: String,
) -> Result<ConfigWriteReport, String> {
    let installed = installed_or_error(&state, game_id)?;
    let record = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::get_config_edit(&connection, game_id, &backup_id)?
            .ok_or_else(|| "That backup is not in this game's history.".to_string())?
    };
    let outcome = game_config::restore_backup(
        Path::new(&installed.install_path),
        &state.app_data,
        game_id,
        &record.file_path,
        Path::new(&record.backup_path),
    )?;
    {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        if let Err(error) = db::insert_config_edit(&connection, &outcome.record) {
            tracing::warn!(error = %error, "could not record the config restore");
        }
    }
    Ok(ConfigWriteReport {
        document: outcome.document,
        diff: outcome.diff,
        backup: outcome.record,
    })
}

/// Backup history for the game's Settings tab, newest first.
#[tauri::command]
pub fn list_game_config_backups(
    state: State<'_, AppState>,
    game_id: i64,
) -> Result<Vec<db::ConfigEditRecord>, String> {
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    db::list_config_edits(&connection, game_id, game_config::MAX_HISTORY)
}

/// Built-in engine adapters (#106): detection plus typed fields. Read-only.
#[tauri::command]
pub fn game_config_adapters(
    state: State<'_, AppState>,
    game_id: i64,
) -> Result<AdapterReport, String> {
    let installed = installed_or_error(&state, game_id)?;
    let root = PathBuf::from(&installed.install_path);
    if !root.is_dir() {
        return Err(format!(
            "The install folder is missing: {}. Reinstall the game to inspect its settings.",
            root.display()
        ));
    }
    Ok(config_adapters::report(game_id, &root))
}
