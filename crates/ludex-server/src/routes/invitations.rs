//! Invitation links (#166).
//!
//! Administrators create expiring, single-use links; the plaintext token is
//! returned exactly once and only its SHA-256 hash is stored. Redemption is
//! public (the invitee has no account yet), rate-limited per address, and
//! audited. The redeemed account starts with the invited role and email, and
//! can immediately sign in — the response is the same [`AuthResponse`] the
//! login endpoint returns.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use ludex_core::models::Role;
use rand::RngCore;
use serde_json::json;

use crate::audit::actions;
use crate::auth::{hash_password, start_session, AuthUser, SessionInfo};
use crate::db;
use crate::error::ApiError;
use crate::events::{Actor, Event};
use crate::extract::{ApiPath, JsonBody};
use crate::models::{
    AcceptInvitationRequest, AuthResponse, CreateInvitationRequest, CreatedInvitation, Invitation,
    InvitationPreview, InvitationRow, InvitationStatus, UserRow,
};
use crate::state::AppState;
use crate::totp;

/// Default link lifetime, in days.
const DEFAULT_EXPIRY_DAYS: i64 = 7;
/// Longest link lifetime an administrator may request, in days.
const MAX_EXPIRY_DAYS: i64 = 90;
/// Default number of redemptions.
const DEFAULT_MAX_USES: i64 = 1;
/// Upper bound on redemptions for one link.
const MAX_USES_LIMIT: i64 = 100;
/// Bytes of entropy behind an invitation token.
const TOKEN_BYTES: usize = 24;

/// Invitation routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/invitations", get(list).post(create))
        // The segment is `id` everywhere: axum rejects two routes that differ
        // only by a parameter name. GET/POST read it as the plaintext token,
        // DELETE as the numeric invitation id.
        .route("/invitations/{id}", get(preview).delete(revoke))
        .route("/invitations/{id}/accept", post(accept))
}

/// Row shape used by the admin list, including the creator's username.
const INVITATION_COLUMNS: &str = "SELECT i.id, i.role, i.email, i.note, i.created_by,
        (SELECT username FROM users u WHERE u.id = i.created_by) AS created_by_username,
        i.created_at, i.expires_at, i.max_uses, i.uses, i.last_used_at, i.revoked_at
     FROM invitations i";

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<Invitation>>, ApiError> {
    user.require_admin_access()?;
    let rows = db::query_as::<InvitationRow>(format!(
        "{INVITATION_COLUMNS} ORDER BY i.created_at DESC, i.id DESC"
    ))
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;
    let now = Utc::now();
    Ok(Json(rows.iter().map(|row| row.to_public(now)).collect()))
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<CreateInvitationRequest>,
) -> Result<(StatusCode, Json<CreatedInvitation>), ApiError> {
    user.require_admin_access()?;
    let role = request.role.unwrap_or(Role::User);
    let email = super::validate_email(request.email)?;
    let note = request
        .note
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if let Some(note) = note.as_deref() {
        if note.chars().count() > 500 {
            return Err(ApiError::bad_request(
                "invalid_note",
                "The note must be at most 500 characters",
            ));
        }
    }
    let expires_in_days = request.expires_in_days.unwrap_or(DEFAULT_EXPIRY_DAYS);
    if !(1..=MAX_EXPIRY_DAYS).contains(&expires_in_days) {
        return Err(ApiError::bad_request(
            "invalid_expiry",
            format!("The link must expire in 1 to {MAX_EXPIRY_DAYS} days"),
        ));
    }
    let max_uses = request.max_uses.unwrap_or(DEFAULT_MAX_USES);
    if !(1..=MAX_USES_LIMIT).contains(&max_uses) {
        return Err(ApiError::bad_request(
            "invalid_max_uses",
            format!("A link may be redeemed 1 to {MAX_USES_LIMIT} times"),
        ));
    }

    let token = new_token();
    let now = Utc::now();
    let expires_at = now + Duration::days(expires_in_days);
    let id = db::query(
        "INSERT INTO invitations
            (token_hash, role, email, note, created_by, created_at, expires_at,
             max_uses, uses, last_used_at, revoked_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, NULL, NULL)",
    )
    .bind(totp::hash_secret(&token))
    .bind(role.as_str())
    .bind(email.as_deref())
    .bind(note.as_deref())
    .bind(user.id)
    .bind(db::db_time(now))
    .bind(db::db_time(expires_at))
    .bind(max_uses)
    .returning_id(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let row = fetch_row(&state, id).await?;
    let invitation = row.to_public(now);
    state.events.emit(
        Event::new(actions::INVITATION_CREATED, user.actor())
            .target("invitation", id.to_string())
            .metadata(json!({
                "role": role.as_str(),
                "email": invitation.email,
                "maxUses": max_uses,
                "expiresAt": invitation.expires_at,
            })),
    );

    Ok((
        StatusCode::CREATED,
        Json(CreatedInvitation {
            path: format!("/register?invite={token}"),
            invitation,
            token,
        }),
    ))
}

async fn revoke(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_admin_access()?;
    let updated = db::query(
        "UPDATE invitations SET revoked_at = ?
         WHERE id = ? AND revoked_at IS NULL",
    )
    .bind(db::db_time(db::now()))
    .bind(id)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?
    .rows_affected();
    if updated == 0 {
        // Either unknown or already revoked; both are "not pending".
        let exists = db::query_scalar::<i64>("SELECT COUNT(*) FROM invitations WHERE id = ?")
            .bind(id)
            .fetch_one(&state.pool)
            .await
            .map_err(ApiError::from)?;
        if exists == 0 {
            return Err(ApiError::not_found(
                "invitation_not_found",
                "Invitation not found",
            ));
        }
    }
    state.events.emit(
        Event::new(actions::INVITATION_REVOKED, user.actor())
            .target("invitation", id.to_string())
            .metadata(json!({})),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Public: shows the invitation details before the form is filled.
async fn preview(
    State(state): State<AppState>,
    ApiPath(token): ApiPath<String>,
) -> Result<Json<InvitationPreview>, ApiError> {
    let row = lookup(&state, &token).await?;
    let invitation = row.to_public(Utc::now());
    match invitation.status {
        InvitationStatus::Pending => {}
        InvitationStatus::Used => {
            return Err(ApiError::new(
                StatusCode::GONE,
                "invitation_used",
                "This invitation was already used; ask an administrator for a new one",
            ));
        }
        InvitationStatus::Expired => {
            return Err(ApiError::new(
                StatusCode::GONE,
                "invitation_expired",
                "This invitation expired; ask an administrator for a new one",
            ));
        }
        InvitationStatus::Revoked => {
            return Err(ApiError::new(
                StatusCode::GONE,
                "invitation_revoked",
                "This invitation was revoked; ask an administrator for a new one",
            ));
        }
    }
    Ok(Json(InvitationPreview {
        role: invitation.role,
        email: invitation.email,
        note: invitation.note,
        expires_at: invitation.expires_at,
        requires_password: true,
    }))
}

/// Public: redeems the invitation and signs the new account in.
async fn accept(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ApiPath(token): ApiPath<String>,
    JsonBody(request): JsonBody<AcceptInvitationRequest>,
) -> Result<(StatusCode, Json<AuthResponse>), ApiError> {
    let ip = peer.ip();
    if let Err(wait) = state.invitation_limits.check(&ip.to_string()) {
        return Err(ApiError::too_many_requests(
            "invitation_rate_limited",
            format!("Too many invitation attempts. Retry in {wait} seconds."),
        )
        .with_retry_after(wait));
    }
    let username = super::validate_username(&request.username)?;
    super::validate_password(&request.password)?;
    let password_hash = hash_password(&request.password)?;

    let row = lookup(&state, &token).await?;
    let invitation = row.to_public(Utc::now());
    match invitation.status {
        InvitationStatus::Pending => {}
        InvitationStatus::Used => {
            return Err(ApiError::new(
                StatusCode::GONE,
                "invitation_used",
                "This invitation was already used; ask an administrator for a new one",
            ));
        }
        InvitationStatus::Expired => {
            return Err(ApiError::new(
                StatusCode::GONE,
                "invitation_expired",
                "This invitation expired; ask an administrator for a new one",
            ));
        }
        InvitationStatus::Revoked => {
            return Err(ApiError::new(
                StatusCode::GONE,
                "invitation_revoked",
                "This invitation was revoked; ask an administrator for a new one",
            ));
        }
    }

    let now = Utc::now();
    let now_text = db::db_time(now);
    let mut transaction = state.pool.begin().await.map_err(ApiError::from)?;
    // The conditional update is the single-use guarantee: concurrent
    // redemptions of the same link cannot both increment `uses` past `max_uses`.
    let claimed = db::query(
        "UPDATE invitations SET uses = uses + 1, last_used_at = ?
         WHERE id = ? AND uses < max_uses AND revoked_at IS NULL AND expires_at > ?",
    )
    .bind(&now_text)
    .bind(invitation.id)
    .bind(&now_text)
    .execute(&mut transaction)
    .await
    .map_err(ApiError::from)?
    .rows_affected();
    if claimed != 1 {
        return Err(ApiError::new(
            StatusCode::GONE,
            "invitation_unavailable",
            "This invitation is no longer available; ask an administrator for a new one",
        ));
    }

    let created = db::query(
        "INSERT INTO users
            (username, password_hash, email, role, activated, avatar_path, created_at, updated_at)
         VALUES (?, ?, ?, ?, 1, NULL, ?, ?)",
    )
    .bind(&username)
    .bind(&password_hash)
    .bind(invitation.email.as_deref())
    .bind(invitation.role.as_str())
    .bind(&now_text)
    .bind(&now_text)
    .returning_id(&mut transaction)
    .await
    .map_err(|error| {
        if super::system::is_unique_violation(&error) {
            ApiError::conflict("username_taken", "Username is already taken")
        } else {
            ApiError::from(error)
        }
    })?;
    transaction.commit().await.map_err(ApiError::from)?;

    let row = db::query_as::<UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path,
                age_limit, created_at, updated_at, oidc_issuer, oidc_subject,
                onboarding_completed_at, onboarding_step,
                CAST(COALESCE((SELECT 1 FROM user_mfa m WHERE m.user_id = users.id AND m.enabled_at IS NOT NULL), 0) AS BIGINT) AS mfa_enabled,
                must_change_password
         FROM users WHERE id = ?",
    )
    .bind(created)
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;
    let account = row.to_public();

    state.events.emit(
        Event::new(actions::INVITATION_REDEEMED, Actor::User { id: created })
            .target("invitation", invitation.id.to_string())
            .metadata(json!({
                "username": account.username,
                "role": account.role.as_str(),
                "uses": invitation.uses + 1,
            }))
            .ip(Some(ip)),
    );

    let info = SessionInfo {
        user_agent: headers
            .get(axum::http::header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        ip: Some(ip.to_string()),
    };
    let (issued, session_id) = start_session(&state, created, account.role, &info).await?;
    state.events.emit(
        Event::new(actions::LOGIN_SUCCEEDED, Actor::User { id: created })
            .target("user", created.to_string())
            .metadata(json!({
                "username": account.username,
                "sessionId": session_id,
                "method": "invitation",
            }))
            .ip(Some(ip)),
    );
    Ok((
        StatusCode::CREATED,
        Json(AuthResponse::session(
            issued.token,
            account,
            Some(issued.expires_at),
        )),
    ))
}

async fn lookup(state: &AppState, token: &str) -> Result<InvitationRow, ApiError> {
    let hash = totp::hash_secret(token.trim());
    db::query_as::<InvitationRow>(format!("{INVITATION_COLUMNS} WHERE i.token_hash = ?"))
        .bind(&hash)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| {
            ApiError::not_found(
                "invitation_not_found",
                "This invitation link is not valid; ask an administrator for a new one",
            )
        })
}

async fn fetch_row(state: &AppState, id: i64) -> Result<InvitationRow, ApiError> {
    db::query_as::<InvitationRow>(format!("{INVITATION_COLUMNS} WHERE i.id = ?"))
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("invitation_not_found", "Invitation not found"))
}

/// A fresh URL-safe token. Only its hash is stored.
fn new_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_random_and_hashed() {
        let first = new_token();
        let second = new_token();
        assert_eq!(first.len(), TOKEN_BYTES * 2);
        assert_ne!(first, second);
        assert!(first.chars().all(|character| character.is_ascii_hexdigit()));
        assert_ne!(totp::hash_secret(&first), totp::hash_secret(&second));
    }
}
