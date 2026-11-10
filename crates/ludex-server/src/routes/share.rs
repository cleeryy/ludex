//! Share links (#9): editor CRUD plus public, account-free access.
//!
//! Administrative routes require an editor bearer token. The two `/share/`
//! routes are public and must never expose account data: they return the
//! metadata of the linked game only and a short-lived download ticket.

use std::net::{IpAddr, SocketAddr};

use axum::body::Bytes;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::audit::actions;
use crate::auth::{self, AuthUser};
use crate::db;
use crate::error::ApiError;
use crate::events::{Actor, Event};
use crate::extract::{ApiPath, JsonBody};
use crate::share::{
    self, CreateShareRequest, ShareAccessRequest, ShareAccessResponse, ShareDenial, ShareLink,
    ShareLinkRow, ShareView,
};
use crate::state::AppState;

/// Longest accepted label.
const MAX_LABEL_CHARS: usize = 100;
/// Longest accepted password, to bound the Argon2 cost.
const MAX_PASSWORD_CHARS: usize = 1024;
/// Smallest accepted password.
const MIN_PASSWORD_CHARS: usize = 4;
/// Expiry bounds, in hours (one hour to one year).
const MIN_EXPIRY_HOURS: i64 = 1;
const MAX_EXPIRY_HOURS: i64 = 8_760;
/// Download cap bounds.
const MIN_DOWNLOADS: i64 = 1;
const MAX_DOWNLOADS: i64 = 100_000;

/// Share link routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/games/{id}/shares", post(create))
        .route("/shares", get(list))
        .route("/shares/{id}", delete(revoke))
        .route("/share/{token}", get(view))
        .route("/share/{token}/access", post(access))
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    JsonBody(request): JsonBody<CreateShareRequest>,
) -> Result<(StatusCode, Json<ShareLink>), ApiError> {
    user.require_editor()?;

    let exists = sqlx::query_scalar::<_, i64>("SELECT id FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?;
    if exists.is_none() {
        return Err(ApiError::not_found("game_not_found", "Game not found"));
    }

    let expires_at = match request.expires_in_hours {
        Some(hours) if (MIN_EXPIRY_HOURS..=MAX_EXPIRY_HOURS).contains(&hours) => {
            Some(db::now() + chrono::Duration::hours(hours))
        }
        Some(_) => {
            return Err(ApiError::bad_request(
                "invalid_expiry",
                format!("expiresInHours must be between {MIN_EXPIRY_HOURS} and {MAX_EXPIRY_HOURS}"),
            ))
        }
        None => None,
    };

    let max_downloads = match request.max_downloads {
        Some(value) if (MIN_DOWNLOADS..=MAX_DOWNLOADS).contains(&value) => Some(value),
        Some(_) => {
            return Err(ApiError::bad_request(
                "invalid_max_downloads",
                format!("maxDownloads must be between {MIN_DOWNLOADS} and {MAX_DOWNLOADS}"),
            ))
        }
        None => None,
    };

    let password = match request.password.as_deref() {
        Some(value) if value.trim().is_empty() => None,
        Some(value) if value.chars().count() < MIN_PASSWORD_CHARS => {
            return Err(ApiError::bad_request(
                "invalid_password",
                format!("Share passwords must be at least {MIN_PASSWORD_CHARS} characters"),
            ))
        }
        Some(value) if value.chars().count() > MAX_PASSWORD_CHARS => {
            return Err(ApiError::bad_request(
                "invalid_password",
                format!("Share passwords are limited to {MAX_PASSWORD_CHARS} characters"),
            ))
        }
        Some(value) => Some(value.to_string()),
        None => None,
    };

    let label = validate_label(request.label.as_deref())?;

    let version_id = match request.version_id {
        Some(version_id) => {
            let found = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM game_versions WHERE id = ? AND game_id = ? AND missing = 0",
            )
            .bind(version_id)
            .bind(game_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(ApiError::from)?;
            Some(found.ok_or_else(|| {
                ApiError::bad_request(
                    "invalid_version",
                    "This version does not belong to the game or is no longer available",
                )
            })?)
        }
        None => None,
    };

    let token = share::generate_token();
    let token_hash = share::hash_token(&token);
    let password_hash = password
        .as_deref()
        .map(auth::hash_password)
        .transpose()
        .map_err(ApiError::from)?;

    let id = share::insert(
        &state.pool,
        &share::NewShare {
            token_hash: &token_hash,
            game_id,
            version_id,
            label: label.as_deref(),
            created_by: user.id,
            expires_at,
            password_hash: password_hash.as_deref(),
            max_downloads,
        },
    )
    .await?;

    state.events.emit(
        Event::new(actions::SHARE_CREATED, user.actor())
            .target("share", id.to_string())
            .metadata(json!({
                "gameId": game_id,
                "versionId": version_id,
                "label": label,
                "hasPassword": password.is_some(),
                "maxDownloads": max_downloads,
                "expiresAt": expires_at.map(db::db_time),
            })),
    );

    let row = share::get(&state.pool, id)
        .await?
        .ok_or_else(ApiError::internal)?;
    Ok((StatusCode::CREATED, Json(row.to_public(Some(token)))))
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<ShareLink>>, ApiError> {
    user.require_editor()?;
    let rows = share::list(&state.pool).await?;
    Ok(Json(rows.iter().map(|row| row.to_public(None)).collect()))
}

async fn revoke(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_editor()?;
    let row = share::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("share_not_found", "Share link not found"))?;
    if row.revoked_at.is_none() {
        share::revoke(&state.pool, id).await?;
        state.events.emit(
            Event::new(actions::SHARE_REVOKED, user.actor())
                .target("share", id.to_string())
                .metadata(json!({
                    "gameId": row.game_id,
                    "versionId": row.version_id,
                })),
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Public metadata page. No auth extractor: an expired, revoked or unknown
/// link answers the same 404 so the route cannot be used to probe the library.
async fn view(
    State(state): State<AppState>,
    ApiPath(token): ApiPath<String>,
) -> Result<Json<ShareView>, ApiError> {
    let token_hash = share::hash_token(&token);
    let share = share::find_valid_by_token(&state.pool, &token_hash)
        .await?
        .ok_or_else(|| ShareDenial::NotFound.not_found_error())?;

    let game = share::game_metadata(&state.pool, share.game_id)
        .await?
        .ok_or_else(|| ShareDenial::NotFound.not_found_error())?;
    let version = share::resolve_version(&state.pool, share.game_id, share.version_id)
        .await?
        .ok_or_else(|| ShareDenial::NotFound.not_found_error())?;

    Ok(Json(ShareView {
        game,
        version: version.to_public(),
        expires_at: share.expires_at.as_deref().and_then(db::parse_time),
        has_password: share.password_hash.is_some(),
        remaining_downloads: share::remaining_downloads(&share),
    }))
}

/// Exchanges the link (and optional password) for a short-lived download
/// ticket. The token is rate-limited and never logged; passwords are only
/// verified, never stored or echoed.
async fn access(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    ApiPath(token): ApiPath<String>,
    body: Bytes,
) -> Result<Json<ShareAccessResponse>, ApiError> {
    let ip = Some(peer.ip());
    let token_hash = share::hash_token(&token);
    if !share::allow_attempt(&token_hash) {
        return Err(ApiError::too_many_requests(
            "rate_limited",
            "Too many access attempts; retry in a minute",
        ));
    }
    let password = parse_password(&body)?;
    let now = db::now();

    let link = share::find_by_token(&state.pool, &token_hash).await?;
    // Exactly one Argon2 verification per attempt, whatever the token state,
    // so the response time cannot reveal whether the link exists.
    let password_ok = share::verify_access(link.as_ref(), password.as_deref());
    let Some(link) = link else {
        record_denied(&state, None, ShareDenial::NotFound.reason(), ip);
        return Err(ShareDenial::NotFound.access_error());
    };

    if let Err(denial) = share::validate(&link, now) {
        record_denied(&state, Some(&link), denial.reason(), ip);
        return Err(denial.access_error());
    }

    if !password_ok {
        record_denied(&state, Some(&link), "bad_password", ip);
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_share_password",
            "This share link requires a password",
        ));
    }

    let Some(version) = share::resolve_version(&state.pool, link.game_id, link.version_id).await?
    else {
        record_denied(&state, Some(&link), "no_version", ip);
        return Err(ShareDenial::NotFound.access_error());
    };

    let Some(download_count) = share::claim_access(&state.pool, &link, now).await? else {
        // Another request changed the row between validation and the atomic
        // claim (or the expiry second ticked over); re-read to pick the reason.
        let denial = match share::get(&state.pool, link.id).await? {
            Some(current) => share::validate(&current, now)
                .err()
                .unwrap_or(ShareDenial::Exhausted),
            None => ShareDenial::NotFound,
        };
        record_denied(&state, Some(&link), denial.reason(), ip);
        return Err(denial.access_error());
    };

    let (url, expires_at) = share::ticket_url(&state, &link, version.id)?;
    tracing::info!(
        share = link.id,
        game = link.game_id,
        version = version.id,
        "share link accessed"
    );
    state.events.emit(
        Event::new(actions::SHARE_ACCESS, Actor::System)
            .target("share", link.id.to_string())
            .metadata(json!({
                "gameId": link.game_id,
                "versionId": version.id,
                "downloadCount": download_count,
            }))
            .ip(ip),
    );

    Ok(Json(ShareAccessResponse { url, expires_at }))
}

fn parse_password(body: &Bytes) -> Result<Option<String>, ApiError> {
    if body.is_empty() {
        return Ok(None);
    }
    let request: ShareAccessRequest = serde_json::from_slice(body).map_err(|_| {
        ApiError::bad_request(
            "invalid_body",
            "Expected a JSON object with an optional password",
        )
    })?;
    Ok(request.password)
}

fn record_denied(state: &AppState, share: Option<&ShareLinkRow>, reason: &str, ip: Option<IpAddr>) {
    let event = match share {
        Some(share) => Event::new(actions::SHARE_ACCESS_DENIED, Actor::System)
            .target("share", share.id.to_string())
            .metadata(json!({
                "gameId": share.game_id,
                "reason": reason,
            })),
        None => Event::new(actions::SHARE_ACCESS_DENIED, Actor::System)
            .metadata(json!({ "reason": reason })),
    };
    state.events.emit(event.ip(ip));
}

fn validate_label(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(label) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if label.chars().count() > MAX_LABEL_CHARS {
        return Err(ApiError::bad_request(
            "invalid_label",
            format!("Share labels are limited to {MAX_LABEL_CHARS} characters"),
        ));
    }
    if label.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            "invalid_label",
            "Share labels contain invalid characters",
        ));
    }
    Ok(Some(label.to_string()))
}
