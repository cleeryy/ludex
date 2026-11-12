//! Application self-update (#16) and the update center (#163).
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
//! Checks run at most once per 24 h at startup and on demand from the update
//! center, Settings or the tray menu. The status exposes the channel, the
//! endpoint that was queried, the release notes and date, and whether the user
//! skipped this version. Skipping only silences the startup prompt and the
//! badge: the update stays installable from the update center.
//!
//! Nothing here logs a token, a signing key or a password. The only values in
//! the logs are the channel, the version and the endpoint host.
//!
//! Installing is not the end of the story (#186): the Windows plugin exits the
//! process while NSIS replaces the executable, so `install` records the target
//! version first and [`crate::install_health`] verifies on the next start that
//! the running binary really is that version.

use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::install_health;
use crate::settings::UpdateChannel;
use crate::state::AppState;
use crate::util::now_rfc3339;

/// Stable channel endpoint (latest non-prerelease release).
pub const STABLE_ENDPOINT: &str =
    "https://github.com/cleeryy/ludex/releases/latest/download/latest.json";
/// Early-access channel endpoint (moving `edge` prerelease).
pub const EARLY_ENDPOINT: &str =
    "https://github.com/cleeryy/ludex/releases/download/edge/latest.json";
/// Human-facing release list, used by the update center and the web build.
pub const RELEASES_URL: &str = "https://github.com/cleeryy/ludex/releases";
/// Tag of the rolling early-access prerelease.
const EDGE_TAG: &str = "edge";

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
    /// Endpoint the check queried (or would query), shown verbatim in the UI.
    pub endpoint: String,
    /// Version the user asked not to be prompted about, if any.
    pub skipped_version: Option<String>,
    /// True when an update is available and should raise a prompt/badge. It is
    /// false for a skipped version, while `available` stays populated so the
    /// update center can still offer the install.
    pub notify: bool,
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
    /// Release page for this build. `stable` links to the version tag, `early`
    /// to the rolling `edge` release.
    pub release_url: String,
    /// True for `edge` builds (`0.3.2-edge.179`).
    pub is_prerelease: bool,
    /// CI run number of an `edge` build, parsed from the version.
    pub build: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProgress {
    downloaded: u64,
    total: Option<u64>,
    percent: Option<f64>,
    /// Instantaneous download speed since the previous event, in bytes/s.
    speed_bps: u64,
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

/// Parse a channel coming from the UI. Unknown values are rejected loudly
/// instead of silently falling back to stable.
#[must_use]
pub fn parse_channel(raw: &str) -> Option<UpdateChannel> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "stable" => Some(UpdateChannel::Stable),
        "early" => Some(UpdateChannel::Early),
        _ => None,
    }
}

/// True for prerelease versions such as `0.3.2-edge.179`.
#[must_use]
pub fn is_prerelease_version(version: &str) -> bool {
    version.contains('-')
}

/// CI run number of an `edge` build, when the version carries one.
#[must_use]
pub fn edge_build_number(version: &str) -> Option<u64> {
    let (_, rest) = version.split_once("-edge.")?;
    rest.trim().parse::<u64>().ok().filter(|build| *build > 0)
}

/// True when `available` is exactly the version the user skipped.
///
/// An empty or absent value never skips anything: a skipped version stops
/// prompting until the next one, so the comparison is exact.
#[must_use]
pub fn version_is_skipped(available: Option<&str>, skipped: Option<&str>) -> bool {
    let Some(available) = available.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    skipped
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|skipped| skipped == available)
}

/// Decide whether an available update should raise a prompt or a badge.
#[must_use]
pub fn should_notify(available: Option<&str>, skipped: Option<&str>) -> bool {
    available
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some()
        && !version_is_skipped(available, skipped)
}

/// Release page for a channel and version.
#[must_use]
pub fn release_url_for(channel: UpdateChannel, version: &str) -> String {
    match channel {
        UpdateChannel::Stable => {
            let version = version.trim().trim_start_matches('v');
            if version.is_empty() {
                format!("{RELEASES_URL}/latest")
            } else {
                format!("{RELEASES_URL}/tag/v{version}")
            }
        }
        UpdateChannel::Early => format!("{RELEASES_URL}/tag/{EDGE_TAG}"),
    }
}

/// Turn an updater error into something a player can act on.
///
/// The plugin reports signature problems, disk exhaustion and network failures
/// with very different wording across platforms; the UI needs one short,
/// specific sentence per case, with a retry path.
#[must_use]
pub fn install_error_message(raw: &str) -> String {
    let raw = raw.trim();
    let lower = raw.to_ascii_lowercase();
    let base = "Could not install the update.";
    if lower.contains("signature")
        || lower.contains("minisign")
        || lower.contains("verif")
        || lower.contains("untrusted")
    {
        format!(
            "{base} The download did not match the release signature, so Ludex refused to install it. Try again, then use the release page if it keeps failing."
        )
    } else if lower.contains("no space")
        || lower.contains("disk full")
        || lower.contains("insufficient")
        || lower.contains("not enough space")
    {
        format!("{base} The disk is full. Free some space and try again.")
    } else if lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("connection")
        || lower.contains("network")
        || lower.contains("dns")
        || lower.contains("unreachable")
        || lower.contains("offline")
        || lower.contains("request failed")
    {
        format!("{base} The download could not reach GitHub. Check your connection and try again.")
    } else if lower.contains("permission")
        || lower.contains("denied")
        || lower.contains("read-only")
    {
        format!(
            "{base} Ludex is not allowed to write the update. Check the install folder permissions, or reinstall from the release page."
        )
    } else if raw.is_empty() {
        base.to_string()
    } else {
        let detail: String = raw.chars().take(240).collect();
        format!("{base} {detail}")
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
    let channel = settings
        .as_ref()
        .map(|settings| settings.update_channel)
        .unwrap_or_default();
    UpdateStatus {
        configured: !configured_pubkey(app).is_empty(),
        channel,
        current_version: app.package_info().version.to_string(),
        last_checked_at: settings
            .as_ref()
            .and_then(|settings| settings.last_update_check_at.clone()),
        endpoint: endpoint_for(channel).to_string(),
        skipped_version: settings
            .as_ref()
            .and_then(|settings| settings.skipped_update_version.clone()),
        notify: false,
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
        notes: update.body.clone().filter(|notes| !notes.trim().is_empty()),
        published_at: update.date.map(|date| date.to_string()),
        release_url: release_url_for(status.channel, &update.version),
        is_prerelease: is_prerelease_version(&update.version),
        build: edge_build_number(&update.version),
    });
    status.notify = should_notify(
        status
            .available
            .as_ref()
            .map(|update| update.version.as_str()),
        status.skipped_version.as_deref(),
    );
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
        notify = status.notify,
        manual,
        "update check finished"
    );
    Ok(status)
}

/// Startup check, throttled to once per 24 h.
///
/// `update://available` is emitted whenever a version is available, including a
/// skipped one, so Settings and the update center stay accurate. The payload's
/// `notify` flag is false for a skipped version: the UI must gate the launch
/// prompt, badge and toast on it (the update stays installable).
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
    tracing::info!(
        version = %version,
        exe = std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        current_version = %status.current_version,
        commit = install_health::BUILD_COMMIT,
        "downloading the application update"
    );
    // #186: persist the version before the installer runs. On Windows the
    // plugin exits the process as soon as NSIS is launched, so the next start
    // is the only place where the result can be checked.
    install_health::record_pending_update(&app, &version);

    let progress_app = app.clone();
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    let mut last_time = Instant::now();
    let mut last_bytes = 0u64;
    let installed = update
        .download_and_install(
            move |chunk, total| {
                if last_emit.elapsed() < Duration::from_millis(250) {
                    return;
                }
                let now = Instant::now();
                let elapsed = now.duration_since(last_time).as_secs_f64();
                let downloaded = chunk as u64;
                let speed_bps = if elapsed > 0.0 && downloaded >= last_bytes {
                    ((downloaded - last_bytes) as f64 / elapsed).round() as u64
                } else {
                    0
                };
                last_emit = now;
                last_time = now;
                last_bytes = downloaded;
                let percent = total
                    .filter(|total| *total > 0)
                    .map(|total| (downloaded as f64 / total as f64 * 100.0).clamp(0.0, 100.0));
                let _ = progress_app.emit(
                    "update://progress",
                    UpdateProgress {
                        downloaded,
                        total,
                        percent,
                        speed_bps,
                    },
                );
            },
            || {},
        )
        .await;
    if let Err(error) = installed {
        // Nothing replaced the binary: drop the expectation so the next start
        // does not report a mismatch that never happened (#186).
        install_health::clear_pending_update(&app);
        return Err(install_error_message(&error.to_string()));
    }

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
            release_url: release_url_for(status.channel, &version),
            is_prerelease: is_prerelease_version(&version),
            build: edge_build_number(&version),
            version,
            current_version: status.current_version.clone(),
            notes: None,
            published_at: None,
        }),
        notify: false,
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

/* ------------------------------ commands -------------------------------- */

/// Current update status without touching the network (#163).
///
/// The update center and the Settings status line render this immediately,
/// then run a manual check on request. It also tells them whether the current
/// version was skipped.
#[tauri::command]
pub fn update_status(app: AppHandle) -> UpdateStatus {
    base_status(&app)
}

/// Persist the update channel and immediately re-check on the new channel
/// (#163). An invalid value is rejected; the stored channel is unchanged.
#[tauri::command]
pub async fn set_update_channel(app: AppHandle, channel: String) -> Result<UpdateStatus, String> {
    let parsed =
        parse_channel(&channel).ok_or_else(|| format!("Unknown update channel: {channel}"))?;
    let state = app.state::<AppState>();
    state.update_settings(|settings| settings.update_channel = parsed)?;
    tracing::info!(channel = parsed.as_str(), "update channel switched");
    check_now(app, true).await
}

/// Persist (or clear) the skipped version (#163).
///
/// `None` or a blank string clears the skip. The update stays available; only
/// the prompt, the badge and the toast are silenced.
#[tauri::command]
pub fn set_update_skip(app: AppHandle, version: Option<String>) -> Result<UpdateStatus, String> {
    let skipped = version
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(64).collect::<String>());
    let state = app.state::<AppState>();
    state.update_settings(|settings| settings.skipped_update_version = skipped.clone())?;
    tracing::info!(
        skipped = skipped.as_deref().unwrap_or("none"),
        "update skip preference saved"
    );
    Ok(base_status(&app))
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

    #[test]
    fn channel_parsing_accepts_known_values_only() {
        assert_eq!(parse_channel("stable"), Some(UpdateChannel::Stable));
        assert_eq!(parse_channel(" Stable "), Some(UpdateChannel::Stable));
        assert_eq!(parse_channel("early"), Some(UpdateChannel::Early));
        assert_eq!(parse_channel("EARLY"), Some(UpdateChannel::Early));
        assert_eq!(parse_channel("beta"), None);
        assert_eq!(parse_channel(""), None);
    }

    #[test]
    fn release_urls_follow_the_channel() {
        assert_eq!(
            release_url_for(UpdateChannel::Stable, "0.3.2"),
            "https://github.com/cleeryy/ludex/releases/tag/v0.3.2"
        );
        // A leading v is not doubled.
        assert_eq!(
            release_url_for(UpdateChannel::Stable, "v0.3.2"),
            "https://github.com/cleeryy/ludex/releases/tag/v0.3.2"
        );
        // The early channel always links to the rolling release.
        assert_eq!(
            release_url_for(UpdateChannel::Early, "0.3.2-edge.179"),
            "https://github.com/cleeryy/ludex/releases/tag/edge"
        );
        assert_eq!(
            release_url_for(UpdateChannel::Stable, "  "),
            "https://github.com/cleeryy/ludex/releases/latest"
        );
    }

    #[test]
    fn edge_versions_expose_their_build_number() {
        assert_eq!(edge_build_number("0.3.2-edge.179"), Some(179));
        assert_eq!(edge_build_number("0.3.2-edge.1"), Some(1));
        assert_eq!(edge_build_number("0.3.2"), None);
        assert_eq!(edge_build_number("0.3.2-edge.next"), None);
        assert_eq!(edge_build_number("0.3.2-edge.0"), None);
        assert!(is_prerelease_version("0.3.2-edge.179"));
        assert!(!is_prerelease_version("0.3.2"));
    }

    #[test]
    fn skipping_is_exact_and_only_silences_notifications() {
        // No update, nothing to skip.
        assert!(!version_is_skipped(None, Some("0.3.2")));
        assert!(!should_notify(None, None));
        // A skipped version is not announced...
        assert!(version_is_skipped(Some("0.3.2"), Some("0.3.2")));
        assert!(!should_notify(Some("0.3.2"), Some("0.3.2")));
        // ...but a newer one is.
        assert!(!version_is_skipped(Some("0.3.3"), Some("0.3.2")));
        assert!(should_notify(Some("0.3.3"), Some("0.3.2")));
        // Blank values never suppress prompts.
        assert!(!version_is_skipped(Some("0.3.2"), Some("   ")));
        assert!(!version_is_skipped(None, Some("   ")));
        assert!(should_notify(Some(" 0.3.2 "), None));
        // Trimming keeps a hand-edited settings file understandable.
        assert!(version_is_skipped(Some(" 0.3.2 "), Some("0.3.2 ")));
        // Edge builds are skipped one build at a time: the next build prompts.
        assert!(should_notify(
            Some("0.3.2-edge.180"),
            Some("0.3.2-edge.179")
        ));
    }

    #[test]
    fn install_errors_are_actionable() {
        let signature = install_error_message(
            "failed to verify signature: invalid minisign signature for the update",
        );
        assert!(signature.contains("signature"));
        assert!(signature.contains("refused"));

        let disk = install_error_message("write failed: No space left on device (os error 28)");
        assert!(disk.contains("disk is full"));

        let network = install_error_message("error sending request: connection timed out");
        assert!(network.contains("connection"));

        let permission = install_error_message("Permission denied (os error 13)");
        assert!(permission.contains("permissions"));

        // Unknown errors keep their detail, bounded, with the common prefix.
        let other = install_error_message("some unexpected updater failure");
        assert!(other.starts_with("Could not install the update."));
        assert!(other.contains("some unexpected updater failure"));
        assert_eq!(
            install_error_message("   "),
            "Could not install the update."
        );
        let long = install_error_message(&"x".repeat(1000));
        assert!(long.chars().count() < 300);
    }
}
