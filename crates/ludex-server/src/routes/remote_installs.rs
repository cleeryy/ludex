//! Remote install routes (#167).
//!
//! The phone queues, the desktop claims and reports. See
//! [`crate::remote_installs`] for the security model and state machine.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::api_keys::Scope;
use crate::audit::actions;
use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, ApiQuery, JsonBody};
use crate::remote_installs::{
    self, CreateRemoteInstallRequest, RemoteInstall, RemoteInstallStatus,
    RemoteInstallStatusRequest,
};
use crate::state::AppState;

/// Remote install routes, relative to `/api`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/remote-installs", get(list).post(create))
        .route("/remote-installs/{id}", get(show).delete(cancel))
        .route("/remote-installs/{id}/status", post(update_status))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

fn parse_statuses(raw: Option<&str>) -> Result<Vec<RemoteInstallStatus>, ApiError> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(Vec::new());
    };
    raw.split(',')
        .map(|value| {
            RemoteInstallStatus::parse(value).ok_or_else(|| {
                ApiError::bad_request(
                    "invalid_status",
                    "status must be queued, claimed, downloading, ready, failed or cancelled",
                )
            })
        })
        .collect()
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<Vec<RemoteInstall>>, ApiError> {
    user.require_scope(Scope::Read)?;
    let statuses = parse_statuses(query.status.as_deref())?;
    let installs =
        remote_installs::list(&state.pool, user.id, &statuses, query.limit.unwrap_or(100)).await?;
    Ok(Json(installs))
}

async fn show(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<RemoteInstall>, ApiError> {
    user.require_scope(Scope::Read)?;
    Ok(Json(remote_installs::get(&state.pool, user.id, &id).await?))
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    JsonBody(request): JsonBody<CreateRemoteInstallRequest>,
) -> Result<(StatusCode, Json<RemoteInstall>), ApiError> {
    // Queueing needs the write capability: a read-only API key must not be
    // able to start downloads on the desktop.
    user.require_scope(Scope::Write)?;
    remote_installs::check_rate_limit(user.id)?;
    let queued = remote_installs::queue(&state.pool, user.id, user.age_limit, &request).await?;
    if queued.created {
        state.events.emit(
            Event::new(actions::REMOTE_INSTALL_REQUESTED, user.actor())
                .target("remote_install", queued.install.id.clone())
                .metadata(json!({
                    "gameId": queued.install.game_id,
                    "versionId": queued.install.version_id,
                    "device": queued.install.device,
                }))
                .ip(Some(peer.ip())),
        );
    }
    let status = if queued.inserted {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(queued.install)))
}

async fn update_status(
    State(state): State<AppState>,
    user: AuthUser,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    ApiPath(id): ApiPath<String>,
    JsonBody(request): JsonBody<RemoteInstallStatusRequest>,
) -> Result<Json<RemoteInstall>, ApiError> {
    user.require_scope(Scope::Write)?;
    let install = remote_installs::update_status(&state.pool, user.id, &id, &request).await?;
    // Progress patches can be frequent; only claim and terminal transitions
    // are worth an audit row.
    if matches!(
        install.status,
        RemoteInstallStatus::Claimed
            | RemoteInstallStatus::Ready
            | RemoteInstallStatus::Failed
            | RemoteInstallStatus::Cancelled
    ) {
        state.events.emit(
            Event::new(actions::REMOTE_INSTALL_UPDATED, user.actor())
                .target("remote_install", install.id.clone())
                .metadata(json!({
                    "gameId": install.game_id,
                    "status": install.status.as_str(),
                    "progress": install.progress,
                }))
                .ip(Some(peer.ip())),
        );
    }
    Ok(Json(install))
}

async fn cancel(
    State(state): State<AppState>,
    user: AuthUser,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    ApiPath(id): ApiPath<String>,
) -> Result<StatusCode, ApiError> {
    user.require_scope(Scope::Write)?;
    let cancelled = remote_installs::cancel(&state.pool, user.id, &id).await?;
    state.events.emit(
        Event::new(actions::REMOTE_INSTALL_CANCELLED, user.actor())
            .target("remote_install", id)
            .metadata(json!({ "cancelled": cancelled }))
            .ip(Some(peer.ip())),
    );
    Ok(StatusCode::NO_CONTENT)
}
