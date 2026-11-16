//! Download tickets, ranged archive downloads and tar streams.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::Json;

use crate::auth::{authenticate_token, bearer_token, issue_download_ticket, AuthUser};
use crate::db;
use crate::download_limits::{parse_job_id, LimitRejection, JOB_ID_HEADER};
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, ApiQuery, JsonBody};
use crate::models::{
    DownloadQuery, LocalPathResponse, StreamQuery, TicketRequest, TicketResponse, VersionRow,
};
use crate::roots;
use crate::state::AppState;
use crate::transfer::local::is_s3_version;
use crate::transfer::{self, log_stream, StreamContext};

use super::ensure_game;
use super::versions::{fetch_version, preferred_version_id};

/// Reads a game's age rating, if any.
pub(crate) async fn game_age_rating(
    state: &AppState,
    game_id: i64,
) -> Result<Option<u8>, ApiError> {
    let rating = db::query_scalar::<Option<i64>>("SELECT age_rating FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?
        .flatten();
    Ok(rating.and_then(|value| u8::try_from(value).ok()))
}

pub(super) async fn download_ticket(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    JsonBody(request): JsonBody<TicketRequest>,
) -> Result<Json<TicketResponse>, ApiError> {
    ensure_game(&state, game_id).await?;
    if age_blocked(user.age_limit, game_age_rating(&state, game_id).await?) {
        deny_age_restricted(&state, &user, game_id);
        return Err(age_not_found());
    }
    let version_id = match request.version_id {
        Some(version_id) => {
            let exists = db::query_scalar::<i64>(
                "SELECT id FROM game_versions WHERE id = ? AND game_id = ? AND missing = 0",
            )
            .bind(version_id)
            .bind(game_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(ApiError::from)?;
            exists
                .ok_or_else(|| ApiError::not_found("version_not_found", "Game version not found"))?
        }
        None => preferred_version_id(&state, game_id).await?,
    };

    let (token, expires_at) =
        issue_download_ticket(&state.jwt_secret, user.id, game_id, version_id)?;
    Ok(Json(TicketResponse {
        url: format!("/api/games/{game_id}/download?versionId={version_id}&token={token}"),
        expires_at,
    }))
}

/// Whether a game's age rating exceeds the account's limit (#39).
pub(crate) fn age_blocked(limit: Option<u8>, rating: Option<u8>) -> bool {
    matches!((limit, rating), (Some(limit), Some(rating)) if rating > limit)
}

/// A restricted game is indistinguishable from a missing one.
pub(crate) fn age_not_found() -> ApiError {
    ApiError::not_found("game_not_found", "Game not found")
}

/// Records an age-restricted access attempt without leaking the game to the
/// caller.
pub(crate) fn deny_age_restricted(state: &AppState, user: &AuthUser, game_id: i64) {
    state.events.emit(
        Event::new(crate::audit::actions::GAME_ACCESS_DENIED, user.actor())
            .target("game", game_id.to_string())
            .metadata(serde_json::json!({
                "reason": "age_restricted",
                "ageLimit": user.age_limit,
            })),
    );
}

async fn authorize_version(
    state: &AppState,
    headers: &HeaderMap,
    game_id: i64,
    requested_version: Option<i64>,
    ticket: Option<&str>,
) -> Result<i64, ApiError> {
    if let Some(ticket) = ticket {
        let decoded = crate::auth::decode_download_ticket(&state.jwt_secret, ticket)
            .ok_or_else(ApiError::unauthorized)?;
        if decoded.game_id != game_id {
            return Err(ApiError::forbidden(
                "ticket_mismatch",
                "Ticket does not match this game",
            ));
        }
        if let Some(requested) = requested_version {
            if requested != decoded.version_id {
                return Err(ApiError::forbidden(
                    "ticket_mismatch",
                    "Ticket does not match this version",
                ));
            }
        }
        return Ok(decoded.version_id);
    }

    let token = bearer_token(headers).ok_or_else(ApiError::unauthorized)?;
    let user = authenticate_token(state, &token).await?;
    if age_blocked(user.age_limit, game_age_rating(state, game_id).await?) {
        deny_age_restricted(state, &user, game_id);
        return Err(age_not_found());
    }
    match requested_version {
        Some(version_id) => Ok(version_id),
        None => preferred_version_id(state, game_id).await,
    }
}

pub(super) async fn download(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    ApiPath(game_id): ApiPath<i64>,
    ApiQuery(query): ApiQuery<DownloadQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _download = state.metrics.download_started();
    let version_id = authorize_version(
        &state,
        &headers,
        game_id,
        query.version_id,
        query.token.as_deref(),
    )
    .await?;

    let version = fetch_version(&state, game_id, version_id).await?;
    if version.missing != 0 {
        return Err(ApiError::not_found(
            "file_not_found",
            "This version is no longer available",
        ));
    }
    // A stalled single-stream client reconnects with `Range` from its last
    // byte, which is a normal request here, so #195 bounded how many times per
    // minute and version one address may do that. A parallel job instead names
    // itself and gets its own budget, address ceiling and part cap (#217).
    let job_id = download_job_id(&headers, &query)?;
    let part_index = download_part_index(&query)?;
    let admission = state
        .download_limits
        .admit(peer.ip(), version.id, job_id.as_deref())
        .map_err(download_limited)?;
    let context = StreamContext {
        job_id,
        part_index,
        admission,
    };
    let _lease = state.transfers.begin(version.id);
    let response = transfer::serve_archive(&state, game_id, &version, &headers).await?;
    Ok(log_stream(response, game_id, context, &version))
}

/// Authorizes a stream request, loads the version row and leases it (#168).
///
/// Returns the lease so the caller keeps it for the lifetime of the response.
pub(super) async fn authorized_version(
    state: &AppState,
    headers: &HeaderMap,
    game_id: i64,
    requested: Option<i64>,
    ticket: Option<&str>,
) -> Result<(VersionRow, crate::state::TransferLease), ApiError> {
    let version_id = authorize_version(state, headers, game_id, requested, ticket).await?;
    let version = fetch_version(state, game_id, version_id).await?;
    if version.missing != 0 {
        return Err(ApiError::not_found(
            "file_not_found",
            "This version is no longer available",
        ));
    }
    let lease = state.transfers.begin(version.id);
    Ok((version, lease))
}

/// `GET /api/games/{id}/versions/{versionId}/stream`
///
/// Sequential tar stream of the archive (#204).
pub(super) async fn stream_version(
    State(state): State<AppState>,
    ApiPath((game_id, version_id)): ApiPath<(i64, i64)>,
    ApiQuery(query): ApiQuery<StreamQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _download = state.metrics.download_started();
    let (version, _lease) = authorized_version(
        &state,
        &headers,
        game_id,
        Some(version_id),
        query.token.as_deref(),
    )
    .await?;
    transfer::tar::serve(&state, game_id, &version).await
}

/// Refused admission as the public `429 too_many_download_attempts` error.
fn download_limited(rejection: LimitRejection) -> ApiError {
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "too_many_download_attempts",
        rejection.message(),
    )
    .with_retry_after(rejection.retry_after)
}

/// Job id from `X-Ludex-Job-Id` (preferred) or `?jobId=` (#217).
fn download_job_id(headers: &HeaderMap, query: &DownloadQuery) -> Result<Option<String>, ApiError> {
    let raw = match headers.get(JOB_ID_HEADER) {
        Some(value) => Some(value.to_str().map_err(|_| invalid_job_id())?),
        None => query.job_id.as_deref(),
    };
    parse_job_id(raw).map_err(|_| invalid_job_id())
}

fn invalid_job_id() -> ApiError {
    ApiError::bad_request(
        "invalid_job_id",
        "Job id must be 1-64 characters of A-Z, a-z, 0-9, '-' or '_'",
    )
}

/// Largest accepted `partIndex`; purely a log label, so it is not otherwise
/// constrained.
const MAX_PART_INDEX: u32 = 255;

fn download_part_index(query: &DownloadQuery) -> Result<Option<u32>, ApiError> {
    match query.part_index {
        Some(index) if index > MAX_PART_INDEX => Err(ApiError::bad_request(
            "invalid_part_index",
            "partIndex must be at most 255",
        )),
        other => Ok(other),
    }
}

pub(super) async fn local_path(
    State(state): State<AppState>,
    _user: AuthUser,
    ApiPath((game_id, version_id)): ApiPath<(i64, i64)>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<LocalPathResponse>, ApiError> {
    if !state.config.local_mode || !peer.ip().is_loopback() {
        return Err(ApiError::not_found(
            "local_path_unavailable",
            "Local paths are only available in local mode from loopback",
        ));
    }
    let version = fetch_version(&state, game_id, version_id).await?;
    if version.missing != 0 {
        return Err(ApiError::not_found(
            "file_not_found",
            "This version is no longer available",
        ));
    }
    if is_s3_version(&version) {
        return Err(ApiError::not_found(
            "local_path_unavailable",
            "This version is stored in object storage and has no local path",
        ));
    }
    if version.root_unavailable != 0 {
        return Err(ApiError::not_found(
            "root_unavailable",
            "The library root holding this file is not available",
        ));
    }
    let path = roots::resolve_version_path(
        &state.pool,
        &state.config,
        version.root_id,
        &version.file_path,
    )
    .await
    .ok_or_else(ApiError::internal)?;
    let canonical = tokio::fs::canonicalize(&path).await.map_err(|_| {
        ApiError::not_found("file_not_found", "The library file is missing from disk")
    })?;
    Ok(Json(LocalPathResponse {
        path: canonical.to_string_lossy().into_owned(),
    }))
}
