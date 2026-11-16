//! Embedded `ludex-server` lifecycle for local mode.

use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::settings::{LocalSettings, Mode};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub running: bool,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub url: String,
    pub token: String,
}

pub fn emit_status(app: &AppHandle, status: &ServerStatus) {
    let _ = app.emit("local-server://status", status);
}

/// Builds the embedded server configuration from the desktop settings (#241).
///
/// The extracted cache follows the "Server file cache" setting, which is on by
/// default in local mode: the server runs on the same machine, the budget
/// bounds the disk cost, and the file transport is reachable from the install
/// pipeline. Turning it off makes the manifest route answer `409
/// extracted_cache_unavailable`, and the `automatic` policy falls back to the
/// archive without a prompt. A remote server never uses this path; it keeps
/// `LUDEX_EXTRACTED_CACHE` as its own opt-in.
fn embedded_config(
    settings: &LocalSettings,
    games_dir: PathBuf,
    data_dir: PathBuf,
    media_dir: PathBuf,
) -> ludex_server::Config {
    let mut config = ludex_server::Config::for_local(games_dir, data_dir, media_dir)
        .with_extracted_cache(
            settings.server_file_cache,
            settings.server_file_cache_budget_bytes(),
        );
    // `serverPort` 0 asks the OS for a free port; a non-zero value is the
    // preferred port for the embedded server.
    if settings.server_port != 0 {
        config.bind = std::net::SocketAddr::from(([127, 0, 0, 1], settings.server_port));
    }
    config
}

/// Start the embedded server, or return the running instance's URL and token.
pub async fn start_embedded(app: &AppHandle) -> Result<ServerInfo, String> {
    let state = app.state::<AppState>();
    let mut guard = state.server.lock().await;

    if let Some(handle) = guard.as_ref() {
        let info = ServerInfo {
            url: handle.url.clone(),
            token: handle.token.clone(),
        };
        drop(guard);
        let _ = state.update_settings(|settings| {
            settings.mode = Mode::Local;
            settings.server_url = info.url.clone();
            settings.token = Some(info.token.clone());
        });
        // #212 follow-up: the local server may have moved to another port since
        // the interrupted job was recorded; point it at the live URL before the
        // startup resume rebuilds the workers.
        crate::resume_url::refresh_pending_install_urls(&state, &info.url);
        emit_status(
            app,
            &ServerStatus {
                running: true,
                url: Some(info.url.clone()),
            },
        );
        return Ok(info);
    }

    let settings = state.settings_snapshot();
    let games_dir = settings
        .games_dir
        .clone()
        .filter(|dir| !dir.trim().is_empty())
        .ok_or_else(|| "Choose a games folder before starting the local server.".to_string())?;

    let games_path = PathBuf::from(&games_dir);
    std::fs::create_dir_all(&games_path).map_err(|e| {
        format!(
            "Could not create the games folder {}: {e}",
            games_path.display()
        )
    })?;

    let data_dir = state.app_data.join("server");
    let media_dir = state.app_data.join("media");
    std::fs::create_dir_all(&data_dir).map_err(|e| {
        format!(
            "Could not create the server data folder {}: {e}",
            data_dir.display()
        )
    })?;
    std::fs::create_dir_all(&media_dir).map_err(|e| {
        format!(
            "Could not create the media folder {}: {e}",
            media_dir.display()
        )
    })?;

    // `serverPort` 0 asks the OS for a free port; a non-zero value is the
    // preferred port for the embedded server.
    let config = embedded_config(&settings, games_path, data_dir, media_dir);
    let handle = ludex_server::start(config)
        .await
        .map_err(|e| format!("Could not start the local server: {e}"))?;

    let info = ServerInfo {
        url: handle.url.clone(),
        token: handle.token.clone(),
    };
    *guard = Some(handle);
    drop(guard);

    if let Err(error) = state.update_settings(|settings| {
        settings.mode = Mode::Local;
        settings.server_url = info.url.clone();
        settings.token = Some(info.token.clone());
    }) {
        tracing::warn!(error = %error, "local server started but settings could not be persisted");
    }

    // #212 follow-up: local mode binds an ephemeral port unless `serverPort` is
    // pinned, so an interrupted job recorded in a previous run points at a port
    // this process does not own. Re-point it at the live server before the
    // startup resume runs.
    crate::resume_url::refresh_pending_install_urls(&state, &info.url);

    emit_status(
        app,
        &ServerStatus {
            running: true,
            url: Some(info.url.clone()),
        },
    );
    tracing::info!(url = %info.url, "embedded server started");
    Ok(info)
}

pub async fn stop_embedded(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let handle = {
        let mut guard = state.server.lock().await;
        guard.take()
    };

    if let Some(handle) = handle {
        let _ = handle.shutdown.send(());
        tracing::info!("embedded server stopped");
    }

    if let Err(error) = state.update_settings(|settings| {
        settings.server_url.clear();
        settings.token = None;
    }) {
        tracing::warn!(error = %error, "server stopped but settings could not be persisted");
    }

    emit_status(
        app,
        &ServerStatus {
            running: false,
            url: None,
        },
    );
    Ok(())
}

pub async fn status(state: &AppState) -> ServerStatus {
    let guard = state.server.lock().await;
    ServerStatus {
        running: guard.is_some(),
        url: guard.as_ref().map(|handle| handle.url.clone()),
    }
}

/// Called once at startup: start the embedded server when local mode is
/// configured, otherwise just publish the status for the UI.
pub async fn autostart(app: AppHandle) {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let wants_local = settings.mode == Mode::Local
        && settings
            .games_dir
            .as_deref()
            .map(|dir| !dir.trim().is_empty())
            .unwrap_or(false);

    if wants_local {
        match start_embedded(&app).await {
            Ok(info) => tracing::info!(url = %info.url, "local server ready"),
            Err(error) => {
                tracing::warn!(error = %error, "local server did not start");
                emit_status(
                    &app,
                    &ServerStatus {
                        running: false,
                        url: None,
                    },
                );
            }
        }
    } else {
        emit_status(
            &app,
            &ServerStatus {
                running: false,
                url: None,
            },
        );
    }
}

/// Best-effort shutdown on app exit. Uses `try_lock` because this runs on the
/// main thread while the app is tearing down.
pub fn stop_on_exit(app: &AppHandle) {
    let state = app.state::<AppState>();
    if let Ok(mut guard) = state.server.try_lock() {
        if let Some(handle) = guard.take() {
            let _ = handle.shutdown.send(());
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{default_server_file_cache_budget_gb, LocalSettings};

    fn paths() -> (PathBuf, PathBuf, PathBuf) {
        (
            PathBuf::from("/tmp/ludex/games"),
            PathBuf::from("/tmp/ludex/data"),
            PathBuf::from("/tmp/ludex/media"),
        )
    }

    #[test]
    fn embedded_launch_enables_the_file_cache_by_default() {
        let settings = LocalSettings::for_app_data(std::path::Path::new("/tmp/ludex"));
        let (games, data, media) = paths();
        let config = embedded_config(&settings, games, data, media);
        assert!(config.local_mode);
        assert!(config.extracted_cache_enabled);
        assert_eq!(
            config.extracted_cache_budget_bytes,
            default_server_file_cache_budget_gb() * 1024 * 1024 * 1024
        );
        assert!(config.extracted_cache_ttl > std::time::Duration::ZERO);
    }

    #[test]
    fn embedded_launch_passes_the_cache_setting_and_budget() {
        let settings = LocalSettings {
            server_file_cache: false,
            server_file_cache_budget_gb: 12,
            ..LocalSettings::default()
        };
        let (games, data, media) = paths();
        let config = embedded_config(&settings, games, data.clone(), media);
        assert!(!config.extracted_cache_enabled);
        assert_eq!(
            config.extracted_cache_budget_bytes,
            12 * 1024 * 1024 * 1024,
            "the desktop budget reaches the embedded server"
        );
        assert_eq!(config.extracted_cache_dir, data.join("extracted"));
    }

    #[test]
    fn embedded_launch_falls_back_to_a_bounded_budget() {
        // A hand-edited 0 must not mean "unbounded"; the server-side floor
        // applies.
        let settings = LocalSettings {
            server_file_cache: true,
            server_file_cache_budget_gb: 0,
            ..LocalSettings::default()
        };
        let (games, data, media) = paths();
        let config = embedded_config(&settings, games, data, media);
        assert!(config.extracted_cache_enabled);
        assert_eq!(
            config.extracted_cache_budget_bytes,
            default_server_file_cache_budget_gb() * 1024 * 1024 * 1024
        );
    }

    #[test]
    fn embedded_launch_keeps_the_preferred_port() {
        let settings = LocalSettings {
            server_port: 45_678,
            ..LocalSettings::default()
        };
        let (games, data, media) = paths();
        let config = embedded_config(&settings, games, data, media);
        assert_eq!(config.bind.port(), 45_678);
        assert!(config.bind.ip().is_loopback());
    }
}
