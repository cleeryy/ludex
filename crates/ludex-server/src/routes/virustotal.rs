//! VirusTotal integration routes (#224).
//!
//! The API key is write-only: `PUT` stores it encrypted and every response
//! carries only a `...last4` fingerprint. The environment variable
//! `LUDEX_VIRUSTOTAL_API_KEY` wins over the database row, and both `PUT` and
//! `DELETE` refuse with `409 env_managed` while it is set.
//!
//! Scanning is hash-only by default. Uploading a file is a separate, explicit
//! mode that requires `confirmUpload` and `consent: "public-sharing"` in the
//! same request; no other endpoint ever uploads anything.
//!
//! `GET /api/games/{id}/virustotal` is a player read: it returns cached
//! reports only and never consumes the administrator's VirusTotal quota.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::audit::actions;
use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, JsonBody};
use crate::state::AppState;
use crate::virustotal::{self, GameScanReport, IntegrationStatus, ScanReport, TestResponse};
use crate::virustotal_rescan::{self, RescanPolicyInput, RescanStatus};

/// Admin and player VirusTotal routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/admin/integrations/virustotal",
            get(status).put(save).delete(remove),
        )
        .route("/admin/integrations/virustotal/test", post(test))
        .route(
            "/admin/integrations/virustotal/rescan",
            get(rescan_status).put(save_rescan),
        )
        .route(
            "/admin/integrations/virustotal/rescan/run",
            post(run_rescan),
        )
        .route("/admin/virustotal/scan", post(scan))
        .route("/games/{id}/virustotal", get(game_reports))
}

/// `PUT /api/admin/integrations/virustotal` body.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SaveVirustotalKeyRequest {
    /// VirusTotal API key, write-only.
    pub api_key: String,
}

/// `POST /api/admin/integrations/virustotal/test` body. The body is optional:
/// `apiKey` tests an unsaved key, an empty body tests the stored/env key.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestVirustotalKeyRequest {
    /// Unsaved key to validate. It is never stored.
    #[serde(default)]
    pub api_key: Option<String>,
}

/// `POST /api/admin/virustotal/scan` response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScanResponse {
    /// One report per looked-up hash (archive first, then binaries/upload).
    pub scans: Vec<ScanReport>,
}

async fn status(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<IntegrationStatus>, ApiError> {
    user.require_admin_access()?;
    Ok(Json(virustotal::integration_status(&state).await))
}

async fn save(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<SaveVirustotalKeyRequest>,
) -> Result<Json<IntegrationStatus>, ApiError> {
    user.require_admin_access()?;
    Ok(Json(virustotal::save_key(&state, &request.api_key).await?))
}

async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<IntegrationStatus>, ApiError> {
    user.require_admin_access()?;
    Ok(Json(virustotal::delete_key(&state).await?))
}

async fn test(
    State(state): State<AppState>,
    user: AuthUser,
    body: axum::body::Bytes,
) -> Result<Json<TestResponse>, ApiError> {
    user.require_admin_access()?;
    // The body is optional so an empty POST tests the stored/env key, while
    // `{ "apiKey": "..." }` validates a key before it is saved.
    let api_key = if body.is_empty() {
        None
    } else {
        let request: TestVirustotalKeyRequest = serde_json::from_slice(&body)
            .map_err(|error| ApiError::bad_request("invalid_body", error.to_string()))?;
        request.api_key
    };
    Ok(Json(
        virustotal::test_key(&state, api_key.as_deref()).await?,
    ))
}

async fn rescan_status(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<RescanStatus>, ApiError> {
    user.require_admin_access()?;
    Ok(Json(virustotal_rescan::status(&state).await?))
}

async fn save_rescan(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<RescanPolicyInput>,
) -> Result<Json<RescanStatus>, ApiError> {
    user.require_admin_access()?;
    let enabled = request.enabled;
    let interval_hours = request.interval_hours;
    let include_malicious = request.include_malicious;
    let status = virustotal_rescan::save_policy(&state, request).await?;
    state.core.events.emit(
        Event::new(actions::VIRUSTOTAL_RESCAN_POLICY, user.actor())
            .target("virustotal", "rescan")
            .metadata(json!({
                "enabled": enabled,
                "intervalHours": interval_hours,
                "interval": virustotal_rescan::interval_label(interval_hours),
                "includeMalicious": include_malicious,
                "nextRunAt": status.next_run_at,
            })),
    );
    Ok(Json(status))
}

async fn run_rescan(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<RescanStatus>, ApiError> {
    user.require_admin_access()?;
    Ok(Json(
        virustotal_rescan::run_now(&state, user.actor()).await?,
    ))
}

async fn scan(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<virustotal::ScanRequest>,
) -> Result<Json<ScanResponse>, ApiError> {
    user.require_admin_access()?;
    let entries = virustotal::scan(&state, request).await?;
    for entry in &entries {
        state.core.events.emit(
            Event::new(actions::GAME_SCANNED, user.actor())
                .target("game", entry.game_id.to_string())
                .metadata(json!({
                    "gameId": entry.game_id,
                    "versionId": entry.report.version_id,
                    "sha256": entry.report.sha256,
                    "verdict": entry.report.verdict.as_str(),
                    "mode": entry.mode.as_str(),
                    "source": entry.report.source.as_str(),
                })),
        );
    }
    Ok(Json(ScanResponse {
        scans: entries.into_iter().map(|entry| entry.report).collect(),
    }))
}

async fn game_reports(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<GameScanReport>, ApiError> {
    // Any signed-in account may read the cached verdict; the handler never
    // calls VirusTotal.
    user.require_scope(crate::api_keys::Scope::Read)?;
    Ok(Json(virustotal::game_reports(&state, id).await?))
}
