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

async fn report(
    state: &AppState,
    base: &str,
    token: Option<&str>,
    id: &str,
    status: &str,
    progress: Option<u8>,
    message: Option<&str>,
) -> Result<(), String> {
    let mut body = json!({ "status": status });
    if let Some(progress) = progress {
        body["progress"] = json!(progress);
    }
    if let Some(message) = message {
        body["message"] = json!(message);
    }
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
pub async fn pending(app: AppHandle) -> Result<Vec<RemoteInstallRequest>, String> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let base = api_base(&settings)
        .ok_or_else(|| "Configure a Ludex server before using remote installs.".to_string())?;
    let mut request = state
        .http
        .get(format!("{base}/api/remote-installs?status=queued"))
        .timeout(Duration::from_secs(15));
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
    let queued = pending(app.clone()).await?;
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

    for request in queued {
        let Some(version_id) = request.version_id else {
            // The library version disappeared between queue and pickup; let
            // the phone see a failure instead of waiting forever.
            let _ = report(
                &state,
                &base,
                token.as_deref(),
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
        spawn_reporter(app.clone(), request.id, job.job_id);
    }
    Ok(result)
}

/// Polls the local install job and mirrors its state onto the server.
fn spawn_reporter(app: AppHandle, remote_id: String, job_id: String) {
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
