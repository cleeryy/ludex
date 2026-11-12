//! Device registry routes (#184).
//!
//! Desktops register and heartbeat here; the phone lists the account's PCs and
//! revokes one it no longer trusts. See [`crate::devices`] for the security
//! model.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::api_keys::Scope;
use crate::audit::actions;
use crate::auth::AuthUser;
use crate::devices;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, JsonBody};
use crate::models::{Device, DeviceHeartbeatRequest};
use crate::state::AppState;

/// Device routes, relative to `/api`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/devices", get(list))
        .route("/devices/heartbeat", post(heartbeat))
        .route("/devices/{id}", delete(revoke))
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<Device>>, ApiError> {
    user.require_scope(Scope::Read)?;
    Ok(Json(devices::list(&state.pool, user.id).await?))
}

async fn heartbeat(
    State(state): State<AppState>,
    user: AuthUser,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    JsonBody(request): JsonBody<DeviceHeartbeatRequest>,
) -> Result<Json<Device>, ApiError> {
    user.require_scope(Scope::Write)?;
    state
        .device_heartbeat_limits
        .check(&user.id.to_string())
        .map_err(|retry| {
            ApiError::too_many_requests(
                "rate_limited",
                format!("Too many heartbeats; try again in {retry} seconds"),
            )
        })?;
    let outcome = devices::heartbeat(&state.pool, user.id, &request).await?;
    // Heartbeats repeat every 20–30 s; only the first one is audited so the
    // trail stays readable.
    if outcome.registered {
        state.events.emit(
            Event::new(actions::DEVICE_REGISTERED, user.actor())
                .target("device", outcome.device.id.clone())
                .metadata(json!({
                    "platform": outcome.device.platform,
                    "appVersion": outcome.device.app_version,
                }))
                .ip(Some(peer.ip())),
        );
    }
    Ok(Json(outcome.device))
}

async fn revoke(
    State(state): State<AppState>,
    user: AuthUser,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    ApiPath(id): ApiPath<String>,
) -> Result<StatusCode, ApiError> {
    user.require_scope(Scope::Write)?;
    let revoked = devices::revoke(&state.pool, user.id, &id).await?;
    if revoked {
        state.events.emit(
            Event::new(actions::DEVICE_REVOKED, user.actor())
                .target("device", id)
                .ip(Some(peer.ip())),
        );
    }
    Ok(StatusCode::NO_CONTENT)
}
