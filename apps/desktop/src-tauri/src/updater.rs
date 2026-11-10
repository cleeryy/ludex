//! Application self-update (#16) through `tauri-plugin-updater`.
//!
//! Two channels are supported:
//!
//! - `stable`: the latest GitHub release (`/releases/latest/download/latest.json`),
//!   which GitHub only serves for non-prerelease tags.
//! - `early`: the moving `edge` prerelease (`/releases/download/edge/latest.json`),
//!   published from `main` between releases.
//!
//! Releases are signed with a minisign key (`TAURI_SIGNING_PRIVATE_KEY` in CI).
//! The public key lives in `tauri.conf.json` under `plugins.updater.pubkey`.
//! When it is empty — a source build without release keys — the updater is
//! reported as *not configured* and installing is refused with an explicit
//! message instead of failing silently at download time.
//!
//! Checks run at most once per 24 h at startup and on demand from Settings or
//! the tray menu.

use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::settings::UpdateChannel;
use crate::state::AppState;
use crate::util::now_rfc3339;

/// Stable channel endpoint (latest non-prerelease release).
pub const STABLE_ENDPOINT: &str =
    "https://github.com/cleeryy/ludex/releases/latest/download/latest.json";
/// Early-access channel endpoint (moving `edge` prerelease).
pub const EARLY_ENDPOINT: &str =
    "https://github.com/cleeryy/ludex/releases/download/edge/latest.json";

/// Minimum delay between two automatic checks.
const AUTO_CHECK_INTERVAL_HOURS: i64 = 24;

/// Result of one update check, returned to the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// False when the build has no signing key configured.
    pub configured: bool,
    pub channel: UpdateChannel,
    pub current_version: String,
    pub last_checked_at: Option<String>,
    pub available: Option<UpdateInfo>,
    /// Human-readable reason when the check could not run.
    pub message: Option<String>,
}

/// An available application update.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    pub notes: Option<String>,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProgress {
    downloaded: u64,
    total: Option<u64>,
    percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateInstalled {
    version: String,
    /// True on platforms where the installer restarts Ludex by itself
    /// (Windows); the UI then only shows "restarting".
    restarted: bool,
}

/// Endpoint for a channel.
#[must_use]
pub fn endpoint_for(channel: UpdateChannel) -> &'static str {
    match channel {
        UpdateChannel::Stable => STABLE_ENDPOINT,
        UpdateChannel::Early => EARLY_ENDPOINT,
    }
}

/// Public signing key from `tauri.conf.json`, empty when unconfigured.
fn configured_pubkey(app: &AppHandle) -> String {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|config| config.get("pubkey"))
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn build_updater(app: &AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    let channel = app
        .try_state::<AppState>()
        .map(|state| state.settings_snapshot().update_channel)
        .unwrap_or_default();
    let endpoint = tauri::Url::parse(endpoint_for(channel))
        .map_err(|e| format!("Invalid update endpoint: {e}"))?;
    app.updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|e| format!("Could not configure the update endpoint: {e}"))?
        .build()
        .map_err(|e| format!("Could not start the updater: {e}"))
}

fn base_status(app: &AppHandle) -> UpdateStatus {
    let state = app.try_state::<AppState>();
    let settings = state.as_ref().map(|state| state.settings_snapshot());
    UpdateStatus {
        configured: !configured_pubkey(app).is_empty(),
        channel: settings
            .as_ref()
            .map(|settings| settings.update_channel)
            .unwrap_or_default(),
        current_version: app.package_info().version.to_string(),
        last_checked_at: settings.and_then(|settings| settings.last_update_check_at),
        available: None,
        message: None,
    }
}

/// Check for an application update.
///
/// `manual` checks ignore the 24 h throttle and always refresh the timestamp;
/// automatic checks respect the throttle (see [`check_on_startup`]).
pub async fn check_now(app: AppHandle, manual: bool) -> Result<UpdateStatus, String> {
    let mut status = base_status(&app);
    if !status.configured {
        status.message = Some(
            "App updates are disabled: this build has no release signing key. \
             See docs/RELEASING.md to configure updater keys."
                .to_string(),
        );
        return Ok(status);
    }

    let updater = build_updater(&app)?;
    let found = updater
        .check()
        .await
        .map_err(|e| format!("Could not check for updates: {e}"))?;

    status.available = found.as_ref().map(|update| UpdateInfo {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        notes: update.body.clone(),
        published_at: update.date.map(|date| date.to_string()),
    });
    if status.available.is_none() {
        status.message = Some("You are running the newest version.".to_string());
    }

    let checked_at = now_rfc3339();
    status.last_checked_at = Some(checked_at.clone());
    if let Some(state) = app.try_state::<AppState>() {
        let _ = state.update_settings(|settings| {
            settings.last_update_check_at = Some(checked_at.clone());
        });
    }

    tracing::info!(
        channel = status.channel.as_str(),
        available = status
            .available
            .as_ref()
            .map(|update| update.version.as_str()),
        manual,
        "update check finished"
    );
    Ok(status)
}

/// Startup check, throttled to once per 24 h.
pub async fn check_on_startup(app: AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let settings = state.settings_snapshot();
    if !settings.check_updates_on_startup {
        return;
    }
    if configured_pubkey(&app).is_empty() {
        tracing::debug!("updater is not configured (no signing key); skipping the startup check");
        return;
    }
    if let Some(last) = settings.last_update_check_at.as_deref() {
        if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(last) {
            let elapsed =
                chrono::Utc::now().signed_duration_since(parsed.with_timezone(&chrono::Utc));
            if elapsed < chrono::Duration::hours(AUTO_CHECK_INTERVAL_HOURS) {
                tracing::debug!("update check is throttled for 24 h");
                return;
            }
        }
    }

    match check_now(app.clone(), false).await {
        Ok(status) if status.available.is_some() => {
            let _ = app.emit("update://available", &status);
        }
        Ok(_) => {}
        Err(error) => tracing::debug!(error = %error, "startup update check failed"),
    }
}

/// Download and install the available update. Progress is emitted as
/// `update://progress`; on Linux the app must be restarted afterwards, on
/// Windows the installer handles it.
pub async fn install(app: AppHandle) -> Result<UpdateStatus, String> {
    let status = base_status(&app);
    if !status.configured {
        return Err(
            "App updates are disabled: this build has no release signing key. \
             See docs/RELEASING.md to configure updater keys."
                .to_string(),
        );
    }

    let updater = build_updater(&app)?;
    let Some(update) = updater
        .check()
        .await
        .map_err(|e| format!("Could not check for updates: {e}"))?
    else {
        return Err("You are running the newest version.".to_string());
    };

    let version = update.version.clone();
    tracing::info!(version = %version, "downloading the application update");

    let progress_app = app.clone();
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    update
        .download_and_install(
            move |chunk, total| {
                if last_emit.elapsed() < Duration::from_millis(250) {
                    return;
                }
                last_emit = Instant::now();
                let downloaded = chunk as u64;
                let percent = total
                    .filter(|total| *total > 0)
                    .map(|total| (downloaded as f64 / total as f64 * 100.0).clamp(0.0, 100.0));
                let _ = progress_app.emit(
                    "update://progress",
                    UpdateProgress {
                        downloaded,
                        total,
                        percent,
                    },
                );
            },
            || {},
        )
        .await
        .map_err(|e| format!("Could not install the update: {e}"))?;

    let restarted = cfg!(windows);
    let _ = app.emit(
        "update://installed",
        UpdateInstalled {
            version: version.clone(),
            restarted,
        },
    );

    Ok(UpdateStatus {
        available: Some(UpdateInfo {
            version,
            current_version: status.current_version.clone(),
            notes: None,
            published_at: None,
        }),
        message: Some(if restarted {
            "The update was installed. Ludex restarts automatically.".to_string()
        } else {
            "The update was installed. Restart Ludex to use it.".to_string()
        }),
        ..status
    })
}

/// Restart Ludex after an update (Linux/macOS).
pub fn restart(app: &AppHandle) -> ! {
    tracing::info!("restarting after an update");
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_point_at_distinct_github_endpoints() {
        assert!(STABLE_ENDPOINT.contains("/releases/latest/download/"));
        assert!(EARLY_ENDPOINT.contains("/releases/download/edge/"));
        assert_ne!(
            endpoint_for(UpdateChannel::Stable),
            endpoint_for(UpdateChannel::Early)
        );
        assert_eq!(endpoint_for(UpdateChannel::Stable), STABLE_ENDPOINT);
        assert_eq!(endpoint_for(UpdateChannel::Early), EARLY_ENDPOINT);
    }

    #[test]
    fn endpoints_are_valid_urls() {
        for channel in [UpdateChannel::Stable, UpdateChannel::Early] {
            let url = tauri::Url::parse(endpoint_for(channel)).unwrap();
            assert_eq!(url.scheme(), "https");
            assert_eq!(url.host_str(), Some("github.com"));
        }
    }
}
