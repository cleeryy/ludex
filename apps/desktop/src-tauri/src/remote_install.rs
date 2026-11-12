//! Remote install pickup (#167).
//!
//! The phone queues a request on the server; this module lets the desktop claim
//! queued requests, start the normal install pipeline and report progress back.
//! The desktop UI calls [`remote_installs_sync`] on a timer (or when the user
//! opens Downloads); the server remains the source of truth.
//!
//! Security: the command uses the saved server token, never a tunnel hostname
//! or a phone-provided credential. A request is only claimed after the server
//! accepted the claim; two desktops racing get `409` on the loser.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Manager};

use crate::install::{self, InstallParams, InstallPhase};
use crate::state::AppState;

/// Registry heartbeat interval (#184). The server marks a device offline after
/// 60 s without one, so 25 s leaves room for one missed tick.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(25);

/// One queued request as returned by the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteInstallRequest {
    pub id: String,
    pub game_id: i64,
    pub game_title: String,
    pub version_id: Option<i64>,
    pub version: Option<String>,
    pub size_bytes: Option<i64>,
    pub device: Option<String>,
}

/// What syncing did, for the desktop UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSyncResult {
    pub claimed: Vec<String>,
    pub skipped: usize,
    pub error: Option<String>,
}

fn in_flight() -> &'static Mutex<HashSet<String>> {
    static FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    FLIGHT.get_or_init(|| Mutex::new(HashSet::new()))
}

fn mark_in_flight(id: &str) -> bool {
    in_flight()
        .lock()
        .map(|mut set| set.insert(id.to_string()))
        .unwrap_or(false)
}

fn clear_in_flight(id: &str) {
    if let Ok(mut set) = in_flight().lock() {
        set.remove(id);
    }
}

fn api_base(settings: &crate::settings::LocalSettings) -> Option<String> {
    let base = settings.server_url.trim().trim_end_matches('/');
    (!base.is_empty()).then(|| base.to_string())
}

/// Last server-assigned device id, refreshed by every successful heartbeat.
fn last_device_id() -> &'static Mutex<Option<String>> {
    static LAST: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(None))
}

fn remember_device_id(id: Option<String>) {
    if let Ok(mut slot) = last_device_id().lock() {
        *slot = id;
    }
}

/// What this PC can do, sent with every heartbeat (#184).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HeartbeatCapabilities {
    installed_game_ids: Vec<i64>,
    installed_count: i64,
    free_disk_bytes: Option<i64>,
    streaming_available: bool,
}

/// Body of `POST /api/devices/heartbeat`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HeartbeatBody<'a> {
    client_id: &'a str,
    name: &'a str,
    platform: &'a str,
    app_version: &'a str,
    capabilities: HeartbeatCapabilities,
}

fn default_device_name() -> String {
    let host = std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .map(|value| value.trim().chars().take(80).collect::<String>())
        .filter(|value| !value.is_empty());
    host.unwrap_or_else(|| "Desktop".to_string())
}

/// Returns the stable local client id and display name, generating both on
/// first use. The id is a random UUID stored in `settings.json`, never derived
/// from hardware or the account.
fn ensure_device_identity(state: &AppState) -> Result<(String, String), String> {
    let settings = state.settings_snapshot();
    let needs_id = settings.device_id.trim().is_empty();
    let needs_name = settings.device_name.is_none();
    if !needs_id && !needs_name {
        return Ok((
            settings.device_id,
            settings.device_name.unwrap_or_else(default_device_name),
        ));
    }
    let updated = state.update_settings(|settings| {
        if settings.device_id.trim().is_empty() {
            settings.device_id = uuid::Uuid::new_v4().simple().to_string();
        }
        if settings.device_name.is_none() {
            settings.device_name = Some(default_device_name());
        }
    })?;
    Ok((
        updated.device_id,
        updated.device_name.unwrap_or_else(default_device_name),
    ))
}

fn heartbeat_body<'a>(
    client_id: &'a str,
    name: &'a str,
    installed_game_ids: Vec<i64>,
    free_disk_bytes: Option<u64>,
    streaming_available: bool,
) -> HeartbeatBody<'a> {
    let installed_count = i64::try_from(installed_game_ids.len()).unwrap_or(i64::MAX);
    HeartbeatBody {
        client_id,
        name,
        platform: std::env::consts::OS,
        app_version: env!("CARGO_PKG_VERSION"),
        capabilities: HeartbeatCapabilities {
            installed_game_ids,
            installed_count,
            free_disk_bytes: free_disk_bytes.and_then(|free| i64::try_from(free).ok()),
            streaming_available,
        },
    }
}

/// Sends one heartbeat, returning the server device id when the account is
/// signed in and the server answered. A missing server/token is not an error:
/// the loop keeps running and resumes after sign-in or a server switch.
pub async fn heartbeat_once(app: &AppHandle) -> Result<Option<String>, String> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let Some(base) = api_base(&settings) else {
        remember_device_id(None);
        return Ok(None);
    };
    let Some(token) = crate::util::non_empty(settings.token.as_deref()).map(str::to_string) else {
        remember_device_id(None);
        return Ok(None);
    };
    let (client_id, name) = ensure_device_identity(&state)?;

    let installed_game_ids = match state.db.lock() {
        Ok(connection) => crate::db::installed_game_ids(&connection).unwrap_or_default(),
        Err(poisoned) => crate::db::installed_game_ids(&poisoned.into_inner()).unwrap_or_default(),
    };
    let free_disk_bytes = fs2::available_space(settings.install_path()).ok();
    let streaming_available = crate::streaming::sunshine_status(&settings).detected;

    let body = heartbeat_body(
        &client_id,
        &name,
        installed_game_ids,
        free_disk_bytes,
        streaming_available,
    );
    let response = state
        .http
        .post(format!("{base}/api/devices/heartbeat"))
        .bearer_auth(token)
        .json(&body)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|error| format!("Could not reach the server: {error}"))?;
    match response.status() {
        status if status.is_success() => {
            let device: serde_json::Value = response
                .json()
                .await
                .map_err(|error| format!("Invalid device payload: {error}"))?;
            let id = device["id"].as_str().map(str::to_string);
            remember_device_id(id.clone());
            Ok(id)
        }
        reqwest::StatusCode::FORBIDDEN => {
            // Revoked: stop advertising this identity until the user signs out
            // or clears the device id from settings.
            remember_device_id(None);
            Err("This PC was revoked for the signed-in account.".to_string())
        }
        status => Err(format!(
            "The server answered HTTP {status} while registering this PC."
        )),
    }
}

/// Starts the background heartbeat (#184).
///
/// Runs for the lifetime of the app: it follows sign-in, sign-out and server
/// switches by reading the current settings each tick, and it keeps running
/// when the window is hidden to the tray. Each tick is a no-op without a
/// server and token.
pub fn start_heartbeat(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            if let Err(error) = heartbeat_once(&app).await {
                tracing::debug!(error = %error, "device heartbeat failed");
            }
            tokio::time::sleep(HEARTBEAT_INTERVAL).await;
        }
    });
}

fn status_body(
    status: &str,
    progress: Option<u8>,
    message: Option<&str>,
    client_id: Option<&str>,
) -> serde_json::Value {
    let mut body = json!({ "status": status });
    if let Some(progress) = progress {
        body["progress"] = json!(progress);
    }
    if let Some(message) = message {
        body["message"] = json!(message);
    }
    // The claim carries the stable client id so a targeted request can only be
    // taken by the PC the phone chose (#184).
    if let Some(client_id) = client_id {
        body["clientId"] = json!(client_id);
    }
    body
}

async fn report(
    state: &AppState,
    base: &str,
    token: Option<&str>,
    client_id: Option<&str>,
    id: &str,
    status: &str,
    progress: Option<u8>,
    message: Option<&str>,
) -> Result<(), String> {
    let body = status_body(status, progress, message, client_id);
    let mut request = state
        .http
        .post(format!("{base}/api/remote-installs/{id}/status"))
        .json(&body)
        .timeout(Duration::from_secs(15));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Could not reach the server: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "The server answered HTTP {} while reporting the remote install.",
            response.status()
        ));
    }
    Ok(())
}

/// Lists installs queued by phones for the signed-in account.
///
/// When `device_id` is known, the server hides requests targeted at another PC;
/// the account-wide backlog stays visible to every desktop.
pub async fn pending(
    app: AppHandle,
    device_id: Option<&str>,
) -> Result<Vec<RemoteInstallRequest>, String> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let base = api_base(&settings)
        .ok_or_else(|| "Configure a Ludex server before using remote installs.".to_string())?;
    let mut url = format!("{base}/api/remote-installs?status=queued");
    if let Some(device_id) = device_id {
        url.push_str(&format!("&deviceId={device_id}"));
    }
    let mut request = state.http.get(url).timeout(Duration::from_secs(15));
    if let Some(token) = crate::util::non_empty(settings.token.as_deref()) {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Could not reach the server: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "The server answered HTTP {} while listing remote installs.",
            response.status()
        ));
    }
    response
        .json()
        .await
        .map_err(|error| format!("Invalid remote install payload: {error}"))
}

/// Claims queued requests and starts one install job per request.
///
/// Safe to call repeatedly: ids already claimed by this process are skipped, a
/// `409` from another desktop counts as "not ours", and the install pipeline
/// already serialises actual downloads.
#[tauri::command]
pub async fn remote_installs_sync(app: AppHandle) -> Result<RemoteSyncResult, String> {
    // Refresh presence first: the heartbeat returns the server device id used
    // to filter targeted requests, and keeps this PC online while the window
    // is hidden to the tray.
    let device_id = heartbeat_once(&app).await.ok().flatten();
    let queued = pending(app.clone(), device_id.as_deref()).await?;
    let mut result = RemoteSyncResult {
        claimed: Vec::new(),
        skipped: 0,
        error: None,
    };
    if queued.is_empty() {
        return Ok(result);
    }

    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let Some(base) = api_base(&settings) else {
        return Ok(result);
    };
    let token = crate::util::non_empty(settings.token.as_deref()).map(str::to_string);
    let client_id = ensure_device_identity(&state).ok().map(|(id, _)| id);

    for request in queued {
        let Some(version_id) = request.version_id else {
            // The library version disappeared between queue and pickup; let
            // the phone see a failure instead of waiting forever.
            let _ = report(
                &state,
                &base,
                token.as_deref(),
                client_id.as_deref(),
                &request.id,
                "failed",
                None,
                Some("The game version is no longer available on the server."),
            )
            .await;
            continue;
        };
        if !mark_in_flight(&request.id) {
            result.skipped += 1;
            continue;
        }
        if let Err(error) = report(
            &state,
            &base,
            token.as_deref(),
            client_id.as_deref(),
            &request.id,
            "claimed",
            None,
            Some("Starting on the desktop"),
        )
        .await
        {
            clear_in_flight(&request.id);
            result.error = Some(error);
            continue;
        }

        let params = InstallParams {
            game_id: request.game_id,
            version_id,
            server_url: base.clone(),
            token: token.clone(),
            title: request.game_title.clone(),
            version: request.version.clone(),
            executable: None,
            install_dir: None,
            download_dir: None,
            install_location_id: None,
            download_location_id: None,
            expected_bytes: request
                .size_bytes
                .and_then(|value| u64::try_from(value).ok()),
            stream: None,
            replace_path: None,
        };
        let job = match install::begin(&app, params) {
            Ok(job) => job,
            Err(error) => {
                clear_in_flight(&request.id);
                let _ = report(
                    &state,
                    &base,
                    token.as_deref(),
                    client_id.as_deref(),
                    &request.id,
                    "failed",
                    None,
                    Some(&error),
                )
                .await;
                continue;
            }
        };

        result.claimed.push(request.id.clone());
        spawn_reporter(
            app.clone(),
            request.id,
            job.job_id,
            client_id.clone(),
        );
    }
    Ok(result)
}

/// Polls the local install job and mirrors its state onto the server.
fn spawn_reporter(
    app: AppHandle,
    remote_id: String,
    job_id: String,
    client_id: Option<String>,
) {
    tauri::async_runtime::spawn(async move {
        let mut last_percent: Option<u8> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let job = match install::job(&app, &job_id) {
                Ok(job) => job,
                Err(_) => {
                    clear_in_flight(&remote_id);
                    return;
                }
            };
            let percent = job
                .bytes_total
                .filter(|total| *total > 0)
                .map(|total| ((job.bytes_done.min(total) as f64 / total as f64) * 100.0) as u8)
                .or_else(|| match (job.entries_done, job.entries_total) {
                    (Some(done), Some(total)) if total > 0 => {
                        Some(((done.min(total) as f64 / total as f64) * 100.0) as u8)
                    }
                    _ => None,
                });
            let state = app.state::<AppState>();
            let settings = state.settings_snapshot();
            let Some(base) = api_base(&settings) else {
                clear_in_flight(&remote_id);
                return;
            };
            let token = crate::util::non_empty(settings.token.as_deref()).map(str::to_string);

            let outcome = match job.phase {
                InstallPhase::Done => Some(("ready", Some(100), None)),
                InstallPhase::Error => Some(("failed", None, job.message.as_deref())),
                InstallPhase::Cancelled => Some(("cancelled", None, None)),
                InstallPhase::Paused => {
                    Some(("downloading", percent, Some("Paused on the desktop")))
                }
                _ => {
                    if percent != last_percent {
                        last_percent = percent;
                        Some(("downloading", percent, job.message.as_deref()))
                    } else {
                        None
                    }
                }
            };
            if let Some((status, progress, message)) = outcome {
                let _ = report(
                    &state,
                    &base,
                    token.as_deref(),
                    client_id.as_deref(),
                    &remote_id,
                    status,
                    progress,
                    message,
                )
                .await;
                if matches!(status, "ready" | "failed" | "cancelled") {
                    clear_in_flight(&remote_id);
                    return;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_payload_reports_capabilities() {
        let body = heartbeat_body("pc-abc", "Gaming PC", vec![3, 1], Some(4096), true);
        let json = serde_json::to_value(&body).expect("json");
        assert_eq!(json["clientId"], "pc-abc");
        assert_eq!(json["name"], "Gaming PC");
        assert_eq!(json["platform"], std::env::consts::OS);
        assert_eq!(json["appVersion"], env!("CARGO_PKG_VERSION"));
        assert_eq!(json["capabilities"]["installedCount"], 2);
        assert_eq!(json["capabilities"]["installedGameIds"], json!([3, 1]));
        assert_eq!(json["capabilities"]["freeDiskBytes"], 4096);
        assert_eq!(json["capabilities"]["streamingAvailable"], true);
    }

    #[test]
    fn heartbeat_payload_tolerates_unknown_disk_and_empty_library() {
        let body = heartbeat_body("pc-abc", "Desktop", Vec::new(), None, false);
        let json = serde_json::to_value(&body).expect("json");
        assert_eq!(json["capabilities"]["installedCount"], 0);
        assert!(json["capabilities"]["freeDiskBytes"].is_null());
        assert_eq!(json["capabilities"]["streamingAvailable"], false);
    }

    #[test]
    fn claim_body_carries_the_device_id_only_when_known() {
        let body = status_body("claimed", None, Some("Starting"), Some("pc-abc"));
        assert_eq!(body["status"], "claimed");
        assert_eq!(body["clientId"], "pc-abc");
        assert_eq!(body["message"], "Starting");

        let legacy = status_body("claimed", None, None, None);
        assert!(legacy.get("clientId").is_none());
    }

    #[test]
    fn default_device_name_is_never_blank() {
        assert!(!default_device_name().trim().is_empty());
    }
}
