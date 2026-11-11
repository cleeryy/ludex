//! Game update detection (#30) and the update install pipeline.
//!
//! The server exposes the newest version of each game with a version label and
//! a cheap archive fingerprint. The desktop compares that with what it indexed
//! at install time and offers an update when the version id differs or when the
//! archive behind the same version was replaced.
//!
//! `update_game` reuses the normal install pipeline (download or stream) into a
//! staging folder, keeps the previous install untouched until the new files are
//! in place, and only then removes the old folder. Save files live outside the
//! install folder and are never touched.

use std::path::Path;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::db::{self, InstalledGame};
use crate::install::{self, InstallJob, InstallParams};
use crate::state::AppState;
use crate::util::non_empty;

/// Why an update is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateReason {
    /// The server has a newer version row for this game.
    NewVersion,
    /// Same version label, but the archive behind it was replaced.
    ReplacedArchive,
}

/// One game with an update available.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameUpdate {
    pub game_id: i64,
    pub title: String,
    pub installed_version_id: i64,
    pub installed_version: Option<String>,
    pub latest_version_id: i64,
    pub latest_version_label: Option<String>,
    pub archive_size: i64,
    pub archive_hash: Option<String>,
    pub updated_at: Option<String>,
    pub reason: UpdateReason,
}

fn base_url(state: &AppState) -> Result<String, String> {
    let settings = state.settings_snapshot();
    let base = settings.server_url.trim_end_matches('/').to_string();
    if base.is_empty() {
        return Err("No server URL is configured. Choose a server in Settings.".to_string());
    }
    Ok(base)
}

/// Fetch `GET /api/games/{id}/versions/latest` for one game.
pub async fn latest_version(
    state: &AppState,
    game_id: i64,
) -> Result<ludex_core::models::LatestVersion, String> {
    let base = base_url(state)?;
    let settings = state.settings_snapshot();
    let mut request = state
        .http
        .get(format!("{base}/api/games/{game_id}/versions/latest"))
        .timeout(std::time::Duration::from_secs(20));
    if let Some(token) = non_empty(settings.token.as_deref()) {
        request = request.bearer_auth(token);
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("Could not reach the server: {e}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err("The server has no downloadable version for this game.".to_string());
    }
    if !status.is_success() {
        return Err(format!("The server answered HTTP {status}."));
    }
    response
        .json::<ludex_core::models::LatestVersion>()
        .await
        .map_err(|e| format!("The server returned an invalid version payload: {e}"))
}

/// Compare the installed games with the server and return the ones that can be
/// updated. Per-game failures (offline, deleted game) are skipped and logged so
/// one bad game never hides the others.
pub async fn check(app: &AppHandle) -> Result<Vec<GameUpdate>, String> {
    let state = app.state::<AppState>();
    // Fail fast with an actionable message when no server is configured.
    base_url(&state)?;

    let installed: Vec<InstalledGame> = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::list_installed(&connection)?
    };

    let mut updates = Vec::new();
    let mut consecutive_errors = 0usize;
    for game in installed {
        // Steam imports (#13) are not managed by the Ludex server.
        if game.source == db::SOURCE_STEAM || game.game_id <= 0 {
            continue;
        }
        match latest_version(&state, game.game_id).await {
            Ok(latest) => {
                consecutive_errors = 0;
                if let Some(reason) = compare(&game, &latest) {
                    updates.push(GameUpdate {
                        game_id: game.game_id,
                        title: game.title.clone(),
                        installed_version_id: game.version_id,
                        installed_version: game.version.clone(),
                        latest_version_id: latest.version_id,
                        latest_version_label: latest.version_label.clone(),
                        archive_size: latest.archive_size,
                        archive_hash: latest.archive_hash.clone(),
                        updated_at: latest
                            .updated_at
                            .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
                        reason,
                    });
                }
            }
            Err(error) => {
                tracing::debug!(game_id = game.game_id, error = %error, "could not check for a game update");
                consecutive_errors += 1;
                // The server is probably unreachable: do not stall on every
                // installed game with a 20 s timeout each.
                if consecutive_errors >= 3 {
                    break;
                }
            }
        }
    }
    Ok(updates)
}

/// Decide whether `latest` is an update over `installed`.
fn compare(
    installed: &InstalledGame,
    latest: &ludex_core::models::LatestVersion,
) -> Option<UpdateReason> {
    if installed.version_id != latest.version_id {
        return Some(UpdateReason::NewVersion);
    }
    match (&installed.archive_hash, &latest.archive_hash) {
        (Some(installed_hash), Some(latest_hash)) if installed_hash != latest_hash => {
            Some(UpdateReason::ReplacedArchive)
        }
        _ => None,
    }
}

/// Install the newest version of a game, replacing the previous install.
pub async fn update_game(app: &AppHandle, game_id: i64) -> Result<InstallJob, String> {
    let state = app.state::<AppState>();

    let installed = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::get_installed(&connection, game_id)?
    }
    .ok_or_else(|| format!("Game {game_id} is not installed on this device."))?;

    {
        let processes = state
            .processes
            .lock()
            .map_err(|_| "The process list is unavailable; restart Ludex.".to_string())?;
        if processes.values().any(|entry| entry.game_id == game_id) {
            return Err("Stop the game before updating it.".to_string());
        }
    }

    let latest = latest_version(&state, game_id).await?;
    let settings = state.settings_snapshot();

    // Reuse the executable choice as a path relative to the install folder so
    // it resolves inside the new version folder.
    let executable = installed.executable.as_deref().and_then(|path| {
        Path::new(path)
            .strip_prefix(&installed.install_path)
            .ok()
            .map(|relative| relative.to_string_lossy().to_string())
    });

    install::begin(
        app,
        InstallParams {
            game_id,
            version_id: latest.version_id,
            server_url: settings.server_url.clone(),
            token: settings.token.clone(),
            title: installed.title.clone(),
            version: latest.version_label.clone(),
            executable,
            install_dir: installed.install_root.clone(),
            download_dir: None,
            install_location_id: None,
            download_location_id: None,
            expected_bytes: None,
            stream: None,
            replace_path: Some(installed.install_path.clone()),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(version_id: i64, hash: Option<&str>) -> InstalledGame {
        InstalledGame {
            game_id: 1,
            version_id,
            title: "Hades".into(),
            version: Some("1.0".into()),
            install_path: "/games/hades/1.0".into(),
            executable: None,
            size_bytes: 10,
            installed_at: "2026-01-01T00:00:00Z".into(),
            last_launched_at: None,
            archive_hash: hash.map(str::to_string),
            install_root: Some("/games".into()),
            source: crate::db::SOURCE_LUDEX.into(),
            launch_uri: None,
        }
    }

    fn latest(version_id: i64, hash: Option<&str>) -> ludex_core::models::LatestVersion {
        ludex_core::models::LatestVersion {
            game_id: 1,
            version_id,
            version_label: Some("1.1".into()),
            file_name: "Hades (2020) v1.1.zip".into(),
            archive_hash: hash.map(str::to_string),
            archive_size: 10,
            updated_at: None,
            early_access: false,
            download_url: "/api/games/1/download?versionId=2".into(),
        }
    }

    #[test]
    fn a_new_version_id_is_an_update() {
        assert_eq!(
            compare(&installed(1, None), &latest(2, None)),
            Some(UpdateReason::NewVersion)
        );
    }

    #[test]
    fn a_replaced_archive_is_an_update() {
        assert_eq!(
            compare(
                &installed(2, Some("sha256-sample:aa")),
                &latest(2, Some("sha256-sample:bb"))
            ),
            Some(UpdateReason::ReplacedArchive)
        );
    }

    #[test]
    fn identical_versions_are_not_an_update() {
        assert_eq!(
            compare(
                &installed(2, Some("sha256-sample:aa")),
                &latest(2, Some("sha256-sample:aa"))
            ),
            None
        );
        // Unknown fingerprints fall back to the version id.
        assert_eq!(
            compare(&installed(2, None), &latest(2, Some("sha256-sample:aa"))),
            None
        );
        assert_eq!(
            compare(&installed(2, Some("sha256-sample:aa")), &latest(2, None)),
            None
        );
    }
}
