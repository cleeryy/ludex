//! Ludex desktop application (Tauri 2).

mod adopt;
mod archive_verify;
mod bridge;
mod chrome;
mod commands;
mod compat;
mod compat_rules;
mod config_adapters;
mod config_fields;
mod console;
mod console_session;
mod crash_guard;
mod db;
mod diagnostics;
mod display_caps;
mod elevate;
mod file_manifest;
mod game_config;
mod game_executables;
mod host_preflight;
mod inhibit;
mod install;
mod install_health;
mod installer;
mod launch;
mod launchers;
mod linux_gaming;
mod locations;
mod logging;
mod maintenance;
mod manifest;
mod mounts;
mod net;
mod oidc_login;
mod overlay;
mod paths;
mod pipeline;
mod power;
mod prefix;
mod presence;
mod process;
mod process_tree;
mod profiles;
mod protondb;
mod remote_install;
mod remote_play;
mod resume;
mod resume_url;
mod roms;
mod runtime;
mod saves;
mod secrets;
mod server;
mod settings;
mod space;
mod state;
mod steam;
mod storage_suggest;
mod stream_host;
mod stream_install;
mod streaming;
#[cfg(test)]
mod test_support;
mod tray;
mod update;
mod util;
mod webview_profile;
mod window_focus;
mod window_policy;

use tauri::{Manager, RunEvent, WindowEvent};
use tracing_subscriber::EnvFilter;

pub fn run() {
    // #228: stderr stays, and `setup` mirrors it to {appDataDir}/logs/ludex.log
    // because a release Windows build has no visible stderr.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("ludex_desktop=info,ludex_server=info,warn")),
        )
        .with_writer(logging::LogWriter)
        .init();

    // #227: a `--console` launch swallowed by the single-instance plugin must
    // leave a trace. The receipt is written before Tauri builds anything, so a
    // missing receipt proves the process never started.
    let process_args: Vec<String> = std::env::args().collect();
    if process_args
        .iter()
        .any(|argument| argument == console::CONSOLE_FLAG)
    {
        match console::write_launch_receipt(&process_args) {
            Some(path) => tracing::info!(
                path = %path.display(),
                pid = std::process::id(),
                "console launch receipt written"
            ),
            None => tracing::warn!("could not write the console launch receipt"),
        }
    }

    // #177: a Windows update must show the new web UI. The WebView2 profile
    // survives the NSIS update, so drop the caches that can hold the previous
    // shell before any window opens. A no-op outside Windows, and never fatal.
    let context = tauri::generate_context!();
    webview_profile::maybe_reset(&context);

    let app = tauri::Builder::default()
        // The single-instance plugin must be registered first. #227: a second
        // `Ludex.exe --console` (the Sunshine app) must promote the running
        // instance instead of only showing the desktop shell.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            let current = app
                .try_state::<console::ConsoleSession>()
                .map(|session| session.mode())
                .unwrap_or_default();
            let action = console::second_instance_action(&argv, current);
            tracing::info!(
                ?action,
                ?argv,
                "second instance forwarded to the running app"
            );
            match action {
                console::InstanceAction::ShowWindow => tray::show_window(app),
                console::InstanceAction::Promote { fullscreen } => {
                    console::promote(app, fullscreen);
                }
                console::InstanceAction::FocusConsole => console::focus(app),
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        // #213: OS notifications for a new build (Windows toast, macOS
        // Notification Center, Linux libnotify). Plain notifications only; the
        // click handler below focuses Ludex on the update center.
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        // Registered even without a signing key: the plugin refuses to install
        // and `crate::update::app::check_now` reports the missing key explicitly.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                // Close-to-tray (#15) hides the main window instead of quitting;
                // #228: closing the overlay must never hide Ludex. A console
                // session (#172) has no tray and must close for real so Moonlight
                // sees it end. #269: the whole decision comes from one policy.
                match window_policy::close_request_action(
                    window.label(),
                    window_policy::current(app),
                ) {
                    window_policy::CloseAction::HideToTray => {
                        api.prevent_close();
                        tray::hide_window(app);
                    }
                    window_policy::CloseAction::CloseOverlay => {
                        api.prevent_close();
                        if let Err(error) = overlay::close(app, "window") {
                            tracing::debug!(error = %error, "could not close the overlay window");
                        }
                    }
                    window_policy::CloseAction::Close => {}
                }
            }
        })
        .setup(|app| {
            // Console mode (#172): Sunshine starts `ludex --console`. The same
            // binary then runs Big Picture as the console shell, with no tray
            // icon and a real close so the session returns to the desktop.
            // #227: the state is live, so the single-instance callback can
            // promote a running desktop instance into the console shell.
            let console_mode = console::ConsoleMode::from_args(std::env::args());
            app.manage(console::ConsoleSession::new(console_mode));

            // #171: capture panics before anything else can fail. The hook is
            // local-only; nothing is uploaded from it.
            diagnostics::install_panic_hook(app.handle().clone());
            let state = match state::AppState::new(app.handle()) {
                Ok(state) => state,
                Err(error) => {
                    diagnostics::record_startup_failure(app.handle(), &error);
                    return Err(std::io::Error::other(error).into());
                }
            };
            let settings = state.settings_snapshot();
            // #228: now that the app data folder is known, mirror tracing to
            // {appDataDir}/logs/ludex.log. Never fatal.
            match logging::install(&state.app_data) {
                Some(path) => tracing::info!(log_file = %path.display(), "file logging enabled"),
                None => tracing::warn!("file logging is unavailable; stderr only"),
            }
            // #179: the mapping file is the source of truth for cleanup, so a
            // crashed run cannot leave ports open. The startup sweep runs off
            // the main thread and never blocks the window.
            let remote_play_state = remote_play::RemotePlayState::new(&state.app_data);
            remote_play::spawn_startup_cleanup(&state.app_data);
            app.manage(state);
            app.manage(remote_play_state);
            // #288: one suspend inhibitor for the process lifetime, refcounted
            // by the install/game/stream lifecycles below.
            inhibit::init(app.handle(), settings.keep_awake_after_minutes);

            // #213: must be installed before the first notification, otherwise
            // the plugin does not track clicks.
            crate::update::notify::register_click_handler(app.handle())
                .map_err(std::io::Error::other)?;

            if console_mode.active {
                let _ = console::apply_window(app.handle());
            } else {
                // Window chrome (#111) and fullscreen are applied from the one
                // policy (#269): the base config is undecorated for Windows and
                // Linux, so this only does work when the user asked for the
                // native title bar back. macOS keeps its native chrome.
                let _ = window_policy::apply(app.handle());
                tray::init(app.handle()).map_err(std::io::Error::other)?;
            }
            // Discord Rich Presence (#12): off unless enabled in settings.
            presence::start(settings.discord_presence, &settings.discord_client_id);
            // In-game overlay (#144): the global controller listener starts even
            // when no pad is connected, so hot-plug works without a restart.
            overlay::start(app.handle());

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // #186: log the running exe, version and build commit, resolve
                // a pending update and scan for a second install before the
                // rest of the startup work. Blocking filesystem reads only.
                let health_handle = handle.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    install_health::startup_check(&health_handle);
                })
                .await;

                server::autostart(handle.clone()).await;
                // #212/#234: re-enter downloads interrupted by the previous
                // app run after the embedded server is up (their URL points at
                // it). Safe mode and the two-strike circuit breaker live there.
                install::auto_resume_on_startup(&handle);
                // #222: rebuild the per-volume reservation ledger from the
                // active rows, so a restart never double-counts a reservation.
                install::rebuild_space_ledger(&handle);
                // #225: sweep stale staging folders *after* the interrupted
                // jobs are rebuilt, so a folder a resuming job owns is
                // protected. Blocking filesystem work stays off the runtime.
                let sweep_handle = handle.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    install::sweep_stale_staging(&sweep_handle);
                })
                .await;
                // #290: finish a move the previous run interrupted. The source
                // is still indexed until the verified staging copy is swapped.
                maintenance::resume_moves(&handle).await;
                launch::sync_pending(handle.clone()).await;
                // Playtime outbox: finished sessions are already local rows;
                // the worker retries failed deliveries with bounded backoff
                // instead of waiting for the next launch.
                launch::start_retry_worker(handle.clone());
                // Cloud saves (#108): retry failed automatic uploads with
                // bounded backoff; pending state is persisted in SQLite.
                crate::saves::sync::start_retry_worker(handle.clone());
                // Device registry (#184): heartbeat every 25 s so the phone
                // lists this PC as online even when the window is in the tray.
                remote_install::start_heartbeat(handle.clone());
                tray::refresh_menu(&handle);

                // Save-path manifest (#153): at most one check per 24 h,
                // skipped when the user turned automatic downloads off.
                crate::saves::manifest::auto_update(handle.clone()).await;

                // #285: one host preflight at first start (persisted in
                // settings); later launches read the stored result instead of
                // re-probing. Off the runtime and never fatal.
                host_preflight::spawn_startup_probe(handle.clone());

                // Throttled to once per 24 h; a no-op without release keys.
                crate::update::app::check_on_startup(handle.clone()).await;
                // #213: a window left open for days still learns about a new
                // build; the same throttle and preference apply.
                crate::update::app::spawn_periodic_check(handle.clone());

                // #301: signed compatibility rules refresh in the background; a
                // launch never waits for the network and the shipped file always
                // works.
                compat_rules::spawn_startup_refresh(handle.clone());
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
            commands::retry_install_now,
            commands::set_download_network_state,
            commands::reorder_install_job,
            commands::get_install_job,
            commands::list_install_jobs,
            commands::delete_install_job,
            commands::clear_finished_install_jobs,
            commands::check_install_space,
            commands::install_space_ledger,
            remote_install::remote_installs_sync,
            commands::list_installed,
            commands::list_recent_games,
            commands::get_game_playtime,
            commands::launch_game,
            commands::stop_game,
            commands::force_close_game,
            resume::game_session,
            resume::resume_game,
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
            crate::update::notify::notify_update_available,
            commands::restart_app,
            crate::update::app::update_status,
            crate::update::app::set_update_channel,
            crate::update::app::set_update_skip,
            install_health::update_install_health,
            install_health::dismiss_update_mismatch,
            commands::open_path,
            commands::get_disk_usage,
            commands::installer_plan,
            commands::run_installer,
            commands::run_repack_installer,
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
            compat_rules::compat_rules_status,
            compat_rules::compat_rules_report,
            compat_rules::compat_rules_update,
            host_preflight::host_preflight_report,
            host_preflight::host_preflight_plan,
            host_preflight::host_preflight_install,
            commands::get_launch_options,
            commands::set_launch_options,
            commands::wine_prefix_info,
            commands::reset_wine_prefix,
            commands::run_winetricks,
            runtime::commands::runtime_catalog,
            runtime::commands::make_it_run_diagnose,
            runtime::commands::make_it_run_apply,
            runtime::commands::make_it_run_cancel,
            runtime::commands::make_it_run_import,
            runtime::commands::make_it_run_pick_import,
            runtime::commands::make_it_run_remove,
            runtime::commands::make_it_run_select,
            runtime::commands::set_runtime_prerequisites,
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
            commands::apply_account_preferences,
            commands::console_mode,
            commands::exit_console,
            window_policy::window_state,
            power::quit_app,
            power::system_sleep,
            power::sleep_capability,
            commands::inhibit_status,
            console_session::console_session_status,
            console_session::console_session_plan,
            console_session::console_session_apply,
            console_session::console_session_entry_plan,
            console_session::console_session_install_entry,
            console_session::console_session_uninstall_entry,
            commands::sunshine_status,
            commands::sunshine_preview,
            commands::sunshine_sync,
            commands::sunshine_remove_managed,
            commands::grant_sunshine_write_access,
            commands::moonlight_status,
            commands::stream_target,
            commands::stream_game,
            commands::moonlight_stream,
            remote_play::remote_play_status,
            remote_play::remote_play_plan,
            remote_play::remote_play_enable,
            remote_play::remote_play_verify,
            remote_play::remote_play_refresh,
            remote_play::remote_play_disable,
            stream_host::stream_host_status,
            stream_host::stream_host_self_test,
            stream_host::stream_host_config_plan,
            stream_host::stream_host_config_apply,
            stream_host::stream_host_config_revert,
            stream_host::stream_host_firewall_plan,
            stream_host::stream_host_firewall_apply,
            stream_host::stream_host_firewall_revert,
            stream_host::stream_host_pair,
            stream_host::stream_host_pairing_page,
            commands::list_servers,
            commands::save_server,
            commands::remove_server,
            commands::switch_server,
            commands::test_server,
            commands::open_external_url,
            commands::oidc_login_start,
            commands::oidc_login_poll,
            commands::oidc_login_cancel,
            overlay::overlay_status,
            overlay::set_overlay_settings,
            overlay::open_overlay,
            overlay::close_overlay,
            overlay::overlay_game_state,
            overlay::set_overlay_game_disabled,
            overlay::overlay_show_big_picture,
            commands::diagnostics_status,
            commands::set_diagnostics_enabled,
            commands::diagnostics_preview,
            commands::diagnostics_send,
            commands::diagnostics_delete_reports,
            commands::diagnostics_record_error,
        ])
        .build(context)
        .expect("failed to build the Ludex application");

    app.run(|app_handle, event| {
        if matches!(event, RunEvent::Exit) {
            // #234: a clean quit must not look like an unclean startup on the
            // next launch (startup markers are dropped here).
            install::clear_resume_markers_on_clean_exit(app_handle);
            server::stop_on_exit(app_handle);
            // #288: kill and reap the suspend inhibitor before the process
            // goes away; nothing may stay behind in `systemd-inhibit --list`.
            inhibit::shutdown();
            presence::shutdown();
        }
    });
}
