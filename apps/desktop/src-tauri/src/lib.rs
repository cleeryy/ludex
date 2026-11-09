//! Ludex desktop application (Tauri 2).

mod bridge;
mod commands;
mod db;
mod install;
mod launch;
mod paths;
mod server;
mod settings;
mod state;
mod util;

use tauri::{Manager, RunEvent};
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
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .setup(|app| {
            let state = state::AppState::new(app.handle()).map_err(std::io::Error::other)?;
            app.manage(state);

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
                launch::sync_pending(handle).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::pick_folder,
            commands::local_server_start,
            commands::local_server_stop,
            commands::local_server_status,
            commands::install_game,
            commands::cancel_install,
            commands::get_install_job,
            commands::list_install_jobs,
            commands::list_installed,
            commands::launch_game,
            commands::stop_game,
            commands::uninstall_game,
            commands::open_path,
            commands::get_disk_usage,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build the Ludex application");

    app.run(|app_handle, event| {
        if matches!(event, RunEvent::Exit) {
            server::stop_on_exit(app_handle);
        }
    });
}
