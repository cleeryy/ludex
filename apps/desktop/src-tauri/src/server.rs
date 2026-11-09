//! Embedded `ludex-server` lifecycle for local mode.

use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::settings::Mode;
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
    let mut config = ludex_server::Config::for_local(games_path, data_dir, media_dir);
    if settings.server_port != 0 {
        config.bind = std::net::SocketAddr::from(([127, 0, 0, 1], settings.server_port));
    }
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
