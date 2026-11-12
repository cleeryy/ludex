//! Ludex desktop application (Tauri 2).

mod adopt;
mod bridge;
mod chrome;
mod commands;
mod compat;
mod config_adapters;
mod config_fields;
mod db;
mod game_config;
mod game_executables;
mod install;
mod installer;
mod launch;
mod launchers;
mod linux_gaming;
mod locations;
mod maintenance;
mod manifest;
mod oidc_login;
mod paths;
mod prefix;
mod presence;
mod process_tree;
mod profiles;
mod protondb;
mod remote_install;
mod roms;
mod saves_detect;
mod saves_manifest;
mod saves_registry;
mod saves_sync;
mod secrets;
mod server;
mod settings;
mod state;
mod steam;
mod storage_suggest;
mod stream_install;
mod streaming;
#[cfg(test)]
mod test_support;
mod tray;
mod updater;
mod updates;
mod util;

use tauri::{Manager, RunEvent, WindowEvent};
use tracing_subscriber::EnvFilter;

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("ludex_desktop=info,ludex_server=info,warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let app = tauri::Builder::default()
        // The single-instance plugin must be registered first.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            tray::show_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        // Registered even without a signing key: the plugin refuses to install
        // and `updater::check_now` reports the missing key explicitly.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                // Close-to-tray (#15): hide instead of quitting. Quit from the
                // tray menu or the app menu.
                if tray::close_to_tray(app) {
                    api.prevent_close();
                    tray::hide_window(app);
                }
            }
        })
        .setup(|app| {
            let state = state::AppState::new(app.handle()).map_err(std::io::Error::other)?;
            let settings = state.settings_snapshot();
            app.manage(state);

            // Window chrome (#111): the base config is undecorated for Windows
            // and Linux, so this only does work when the user asked for the
            // native title bar back. macOS keeps its native chrome.
            chrome::apply(app.handle(), settings.native_title_bar);

            tray::init(app.handle()).map_err(std::io::Error::other)?;
            // Discord Rich Presence (#12): off unless enabled in settings.
            presence::start(settings.discord_presence, &settings.discord_client_id);

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let install_dir = handle
                    .state::<state::AppState>()
                    .settings_snapshot()
                    .install_path();
                let _ = tokio::task::spawn_blocking(move || {
                    paths::cleanup_staging_dirs(&install_dir);
                })
                .await;

                server::autostart(handle.clone()).await;
                launch::sync_pending(handle.clone()).await;
                // Cloud saves (#108): retry failed automatic uploads with
                // bounded backoff; pending state is persisted in SQLite.
                saves_sync::start_retry_worker(handle.clone());
                tray::refresh_menu(&handle);

                // Save-path manifest (#153): at most one check per 24 h,
                // skipped when the user turned automatic downloads off.
                saves_manifest::auto_update(handle.clone()).await;

                // Throttled to once per 24 h; a no-op without release keys.
                updater::check_on_startup(handle.clone()).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::set_native_title_bar,
            commands::pick_folder,
            commands::local_server_start,
            commands::local_server_stop,
            commands::local_server_status,
            commands::install_game,
            commands::cancel_install,
            commands::pause_install,
            commands::resume_install,
            commands::get_install_job,
            commands::list_install_jobs,
            commands::delete_install_job,
            commands::clear_finished_install_jobs,
            remote_install::remote_installs_sync,
            commands::list_installed,
            commands::list_recent_games,
            commands::get_game_playtime,
            commands::launch_game,
            commands::stop_game,
            roms::get_rom_settings,
            roms::save_rom_emulator,
            roms::delete_rom_emulator,
            roms::set_rom_default,
            roms::validate_rom_command,
            roms::launch_rom_game,
            roms::stop_rom_game,
            commands::uninstall_game,
            commands::move_install,
            commands::verify_install,
            commands::repair_install,
            commands::check_game_updates,
            commands::update_game,
            commands::check_for_updates,
            commands::install_update,
            commands::restart_app,
            updater::update_status,
            updater::set_update_channel,
            updater::set_update_skip,
            commands::open_path,
            commands::get_disk_usage,
            commands::installer_plan,
            commands::run_installer,
            commands::cancel_installer,
            commands::set_executable,
            commands::choose_executable,
            commands::list_game_executables,
            commands::check_game_executable,
            commands::presence_status,
            commands::steam_scan,
            commands::steam_import,
            commands::launcher_scan,
            commands::launcher_import,
            commands::adopt_installation,
            commands::confirm_adopt_installation,
            commands::build_install_manifest,
            commands::unlink_installation,
            commands::delete_adopted_installation,
            commands::scan_gamevault_folders,
            commands::adopt_gamevault_installations,
            commands::add_gamevault_root,
            commands::remove_gamevault_root,
            commands::reorder_gamevault_roots,
            commands::detect_gamevault_roots,
            commands::compat_scan,
            commands::get_launch_options,
            commands::set_launch_options,
            commands::wine_prefix_info,
            commands::reset_wine_prefix,
            commands::run_winetricks,
            commands::protondb_rating,
            commands::detect_save_paths,
            commands::get_save_paths,
            commands::set_save_paths,
            commands::redetect_save_paths,
            commands::ludusavi_manifest_status,
            commands::update_ludusavi_manifest,
            commands::remove_ludusavi_manifest,
            commands::list_save_candidates,
            commands::scan_save_candidates,
            commands::confirm_save_candidate,
            commands::ignore_save_candidate,
            commands::cloud_save_status,
            commands::set_cloud_save_sync,
            commands::sync_cloud_save_now,
            commands::restore_cloud_save,
            commands::resolve_cloud_save_conflict,
            commands::list_locations,
            commands::check_location,
            commands::suggest_storage_locations,
            commands::list_game_configs,
            commands::read_game_config,
            commands::write_game_config,
            commands::restore_game_config,
            commands::list_game_config_backups,
            commands::game_config_adapters,
            commands::game_config_common_fields,
            commands::preview_game_config_common_field,
            commands::apply_game_config_common_field,
            commands::save_game_config_field_mapping,
            commands::delete_game_config_field_mapping,
            commands::export_game_config_mappings,
            commands::game_config_sync_prefs,
            commands::set_game_config_sync_prefs,
            commands::set_config_sync_defaults,
            commands::sunshine_status,
            commands::sunshine_preview,
            commands::sunshine_sync,
            commands::sunshine_remove_managed,
            commands::moonlight_status,
            commands::stream_target,
            commands::stream_game,
            commands::moonlight_stream,
            commands::list_servers,
            commands::save_server,
            commands::remove_server,
            commands::switch_server,
            commands::test_server,
            commands::open_external_url,
            commands::oidc_login_start,
            commands::oidc_login_poll,
            commands::oidc_login_cancel,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build the Ludex application");

    app.run(|app_handle, event| {
        if matches!(event, RunEvent::Exit) {
            server::stop_on_exit(app_handle);
            presence::shutdown();
        }
    });
}
