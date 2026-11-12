//! Authentication routes: login, sessions, logout and password changes.
//!
//! Login is protected by a per-IP + username throttle (`LoginGuard`) and
//! issues a JWT with a `jti` backed by an `auth_sessions` row. Logout, session
//! revocation and password changes invalidate tokens server-side instead of
//! relying on clients to forget them.

use std::net::SocketAddr;
use std::sync::OnceLock;

use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde_json::json;

use crate::api_keys::Scope;
use crate::audit::actions;
use crate::auth::{
    hash_password, revoke_all_sessions, revoke_token, start_session, verify_password, AuthUser,
    SessionInfo,
};
use crate::db;
use crate::error::ApiError;
use crate::events::{Actor, Event};
use crate::extract::{ApiPath, JsonBody};
use crate::mfa;
use crate::models::{
    AuthResponse, AuthSession, AuthSessionRow, ChangePasswordRequest, LoginRequest,
    MfaDisableRequest, MfaLoginRequest, MfaStatus, OnboardingRequest, RecoveryCodes,
    RecoveryCodesRequest, TotpActivated, TotpActivationRequest, TotpEnrollment, UserResponse,
    UserRow,
};
use crate::sql::{bind_query, Bind};
use crate::state::AppState;

/// Authentication and session routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/login/mfa", post(login_mfa))
        .route("/auth/me", get(me))
        .route("/auth/onboarding", put(update_onboarding))
        .route("/auth/logout", post(logout))
        .route("/auth/change-password", post(change_password))
        .route("/me/password", post(change_password))
        .route("/auth/mfa", get(mfa_status))
        .route("/auth/mfa/totp/enroll", post(enroll_totp))
        .route("/auth/mfa/totp/activate", post(activate_totp))
        .route("/auth/mfa/disable", post(disable_mfa))
        .route("/auth/mfa/recovery-codes", post(regenerate_recovery_codes))
        .route(
            "/auth/sessions",
            get(sessions).delete(revoke_other_sessions),
        )
        .route("/auth/sessions/{id}", delete(revoke_session))
}

async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    JsonBody(request): JsonBody<LoginRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let ip = Some(peer.ip());
    let username = request.username.trim().to_string();
    let key = crate::security::LoginGuard::key(ip, &username);
    if let Err((wait, locked)) = state.login_guard.check(&key) {
        return Err(login_throttled(wait, locked));
    }

    let row = db::query_as::<UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path,
                age_limit, created_at, updated_at, oidc_issuer, oidc_subject,
                onboarding_completed_at, onboarding_step,
         CAST(COALESCE((SELECT 1 FROM user_mfa m WHERE m.user_id = users.id AND m.enabled_at IS NOT NULL), 0) AS BIGINT) AS mfa_enabled,
         must_change_password
         FROM users WHERE username = ?",
    )
    .bind(&username)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let Some(row) = row else {
        // Verify against a dummy hash so unknown users cost the same Argon2
        // work as a wrong password (no username enumeration by timing).
        let _ = verify_password(&request.password, dummy_password_hash());
        let (wait, locked) = state.login_guard.record_failure(&key);
        state.events.emit(
            Event::new(actions::LOGIN_FAILED, Actor::System)
                .target("user", &username)
                .metadata(json!({ "reason": "unknown_user" }))
                .ip(ip),
        );
        if locked {
            return Err(login_throttled(wait, true));
        }
        return Err(ApiError::invalid_credentials());
    };
    if !verify_password(&request.password, &row.password_hash) {
        let (wait, locked) = state.login_guard.record_failure(&key);
        state.events.emit(
            Event::new(actions::LOGIN_FAILED, Actor::User { id: row.id })
                .target("user", row.id.to_string())
                .metadata(json!({ "username": row.username, "reason": "bad_password" }))
                .ip(ip),
        );
        if locked {
            return Err(login_throttled(wait, true));
        }
        return Err(ApiError::invalid_credentials());
    }
    if row.activated == 0 {
        state.events.emit(
            Event::new(actions::LOGIN_FAILED, Actor::User { id: row.id })
                .target("user", row.id.to_string())
                .metadata(json!({ "username": row.username, "reason": "account_disabled" }))
                .ip(ip),
        );
        return Err(ApiError::forbidden(
            "account_disabled",
            "This account is disabled",
        ));
    }

    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    // #166: a password alone is not a session when a second factor is on. The
    // challenge is stored hashed, single-use, and carries no token.
    if mfa_enabled(&state, row.id).await? {
        let (challenge_token, expires_at) = mfa::create_challenge(
            &state,
            row.id,
            user_agent.as_deref(),
            Some(&peer.ip().to_string()),
        )
        .await?;
        state.events.emit(
            Event::new(actions::LOGIN_CHALLENGE, Actor::User { id: row.id })
                .target("user", row.id.to_string())
                .metadata(json!({ "username": row.username, "methods": ["totp", "recovery"] }))
                .ip(ip),
        );
        return Ok(Json(AuthResponse::challenge(challenge_token, expires_at)));
    }

    state.login_guard.record_success(&key);
    let user = row.to_public();
    let info = SessionInfo {
        user_agent,
        ip: Some(peer.ip().to_string()),
    };
    let (issued, session_id) = start_session(&state, user.id, user.role, &info).await?;
    state.events.emit(
        Event::new(actions::LOGIN_SUCCEEDED, Actor::User { id: user.id })
            .target("user", user.id.to_string())
            .metadata(json!({
                "username": user.username,
                "sessionId": session_id,
                "method": "password",
            }))
            .ip(ip),
    );
    Ok(Json(AuthResponse::session(
        issued.token,
        user,
        Some(issued.expires_at),
    )))
}

/// Whether the account must present a second factor.
async fn mfa_enabled(state: &AppState, user_id: i64) -> Result<bool, ApiError> {
    let row = db::query_as::<(Option<String>, Option<String>)>(
        "SELECT totp_secret, enabled_at FROM user_mfa WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(row.is_some_and(|(secret, enabled)| secret.is_some() && enabled.is_some()))
}

/// Exchanges a login challenge for a session (#166).
async fn login_mfa(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    JsonBody(request): JsonBody<MfaLoginRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let challenge = mfa::find_challenge(&state, &request.challenge_token).await?;
    // Six digits are brute-forceable if challenges are unlimited: cap the
    // attempts per address and account, independently of the per-challenge
    // budget.
    let attempt_key = format!("{}|{}", peer.ip(), challenge.user_id);
    if let Err(wait) = state.mfa_attempt_limits.check(&attempt_key) {
        return Err(ApiError::too_many_requests(
            "mfa_rate_limited",
            format!("Too many code attempts. Retry in {wait} seconds."),
        )
        .with_retry_after(wait));
    }
    let method = match mfa::verify_second_factor(&state, challenge.user_id, &request.code).await {
        Ok(method) => method,
        Err(error) => {
            mfa::record_failed_attempt(&state, challenge.id).await?;
            state.events.emit(
                Event::new(
                    actions::LOGIN_FAILED,
                    Actor::User {
                        id: challenge.user_id,
                    },
                )
                .target("user", challenge.user_id.to_string())
                .metadata(json!({ "reason": "invalid_mfa_code" }))
                .ip(Some(peer.ip())),
            );
            return Err(error);
        }
    };
    mfa::consume_challenge(&state, challenge.id).await?;

    let row = fetch_user_row(&state, challenge.user_id).await?;
    if row.activated == 0 {
        return Err(ApiError::forbidden(
            "account_disabled",
            "This account is disabled",
        ));
    }
    let user = row.to_public();
    // The password step succeeded; clear any failure counter now that the
    // second factor completed too.
    state
        .login_guard
        .record_success(&crate::security::LoginGuard::key(
            Some(peer.ip()),
            &user.username,
        ));
    let info = SessionInfo {
        user_agent: headers
            .get(axum::http::header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        ip: Some(peer.ip().to_string()),
    };
    let (issued, session_id) = start_session(&state, user.id, user.role, &info).await?;
    state.events.emit(
        Event::new(actions::LOGIN_SUCCEEDED, Actor::User { id: user.id })
            .target("user", user.id.to_string())
            .metadata(json!({
                "username": user.username,
                "sessionId": session_id,
                "method": method,
            }))
            .ip(Some(peer.ip())),
    );
    Ok(Json(AuthResponse::session(
        issued.token,
        user,
        Some(issued.expires_at),
    )))
}

fn dummy_password_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        hash_password("ludex-timing-equaliser-not-a-real-password")
            .unwrap_or_else(|_| "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$aGFzaA".to_string())
    })
}

fn login_throttled(wait: u64, locked: bool) -> ApiError {
    let message = if locked {
        format!("Too many failed login attempts. Try again in {wait} seconds.")
    } else {
        format!("Too many login attempts. Try again in {wait} seconds.")
    };
    ApiError::too_many_requests("login_throttled", message).with_retry_after(wait)
}

async fn me(State(state): State<AppState>, user: AuthUser) -> Result<Json<UserResponse>, ApiError> {
    let row = db::query_as::<UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path,
                age_limit, created_at, updated_at, oidc_issuer, oidc_subject,
                onboarding_completed_at, onboarding_step,
         CAST(COALESCE((SELECT 1 FROM user_mfa m WHERE m.user_id = users.id AND m.enabled_at IS NOT NULL), 0) AS BIGINT) AS mfa_enabled,
         must_change_password
         FROM users WHERE id = ?",
    )
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(ApiError::unauthorized)?;
    Ok(Json(UserResponse {
        user: row.to_public(),
    }))
}

/// Self-service onboarding preference (#119).
///
/// Only `onboardingCompletedAt` and `onboardingStep` are writable here; the
/// administrator user PATCH ignores them. The response is the refreshed account.
async fn update_onboarding(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<OnboardingRequest>,
) -> Result<Json<UserResponse>, ApiError> {
    user.require_scope(Scope::Write)?;
    if let Some(step) = request.step {
        if !(0..=20).contains(&step) {
            return Err(ApiError::bad_request(
                "invalid_onboarding_step",
                "Onboarding step must be between 0 and 20",
            ));
        }
    }

    let mut sets = Vec::new();
    let mut binds = Vec::new();
    if let Some(completed) = request.completed {
        sets.push("onboarding_completed_at = ?");
        binds.push(Bind::OptText(completed.then(|| db::db_time(db::now()))));
    }
    if let Some(step) = request.step {
        sets.push("onboarding_step = ?");
        binds.push(Bind::Int(step));
    }
    if !sets.is_empty() {
        sets.push("updated_at = ?");
        binds.push(Bind::Text(db::db_time(db::now())));
        binds.push(Bind::Int(user.id));
        let sql = format!("UPDATE users SET {} WHERE id = ?", sets.join(", "));
        bind_query(db::query(&sql), &binds)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?;
    }

    let row = db::query_as::<UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path,
                age_limit, created_at, updated_at, oidc_issuer, oidc_subject,
                onboarding_completed_at, onboarding_step,
         CAST(COALESCE((SELECT 1 FROM user_mfa m WHERE m.user_id = users.id AND m.enabled_at IS NOT NULL), 0) AS BIGINT) AS mfa_enabled,
         must_change_password
         FROM users WHERE id = ?",
    )
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(ApiError::unauthorized)?;
    Ok(Json(UserResponse {
        user: row.to_public(),
    }))
}

async fn logout(State(state): State<AppState>, user: AuthUser) -> Result<StatusCode, ApiError> {
    if let Some(jti) = user.jti.as_deref() {
        revoke_token(&state, jti, user.id, "logout").await?;
        state.events.emit(
            Event::new(actions::SESSION_REVOKED, user.actor())
                .target("user", user.id.to_string())
                .metadata(json!({ "reason": "logout" })),
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn sessions(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<AuthSession>>, ApiError> {
    let rows = db::query_as::<AuthSessionRow>(
        "SELECT id, jti, user_agent, ip, created_at, last_seen_at, expires_at, revoked_at
         FROM auth_sessions WHERE user_id = ? ORDER BY created_at DESC, id DESC",
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(Json(
        rows.iter()
            .map(|row| row.to_public(user.jti.as_deref()))
            .collect(),
    ))
}

async fn revoke_session(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    let row = db::query_as::<(i64, i64, String, Option<String>)>(
        "SELECT id, user_id, jti, revoked_at FROM auth_sessions WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("session_not_found", "Session not found"))?;
    if row.1 != user.id {
        user.require_admin()?;
    }
    if row.3.is_none() {
        revoke_token(&state, &row.2, row.1, "session_revoked").await?;
        state.events.emit(
            Event::new(actions::SESSION_REVOKED, user.actor())
                .target("user", row.1.to_string())
                .metadata(json!({ "sessionId": row.0, "reason": "session_revoked" })),
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn change_password(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<ChangePasswordRequest>,
) -> Result<StatusCode, ApiError> {
    user.require_scope(Scope::Write)?;
    let row = db::query_as::<UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path,
                age_limit, created_at, updated_at, oidc_issuer, oidc_subject,
                onboarding_completed_at, onboarding_step,
         CAST(COALESCE((SELECT 1 FROM user_mfa m WHERE m.user_id = users.id AND m.enabled_at IS NOT NULL), 0) AS BIGINT) AS mfa_enabled,
         must_change_password
         FROM users WHERE id = ?",
    )
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(ApiError::unauthorized)?;

    if !verify_password(&request.current_password, &row.password_hash) {
        return Err(ApiError::bad_request(
            "invalid_password",
            "Current password is incorrect",
        ));
    }
    super::validate_password(&request.new_password)?;
    let password_hash = hash_password(&request.new_password)?;
    db::query(
        "UPDATE users SET password_hash = ?, must_change_password = 0, updated_at = ? WHERE id = ?",
    )
    .bind(password_hash)
    .bind(db::db_time(db::now()))
    .bind(user.id)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;

    // Every other session dies with the old password; the caller keeps working
    // with the token it just used. Tokens without a `jti` are invalidated too.
    if let Some(jti) = user.jti.as_deref() {
        crate::auth::revoke_other_sessions(&state, user.id, jti, "password_changed").await?;
    } else {
        revoke_all_sessions(&state, user.id, "password_changed").await?;
    }

    state.events.emit(
        Event::new(actions::USER_PASSWORD_CHANGED, user.actor())
            .target("user", user.id.to_string())
            .metadata(json!({ "username": user.username })),
    );
    Ok(StatusCode::NO_CONTENT)
}

/* ----------------------------- second factor (#166) ---------------------- */

async fn mfa_status(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<MfaStatus>, ApiError> {
    Ok(Json(mfa::status(&state, user.id).await?))
}

/// Starts a TOTP enrolment and returns the secret exactly once.
async fn enroll_totp(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<TotpEnrollment>, ApiError> {
    user.require_scope(Scope::Write)?;
    let issuer = db::server_name(&state.pool, &state.config.server_name).await;
    let enrollment = mfa::start_enrollment(&state, user.id, &issuer, &user.username).await?;
    state.events.emit(
        Event::new(actions::MFA_ENROLLMENT_STARTED, user.actor())
            .target("user", user.id.to_string())
            .metadata(json!({ "method": "totp" })),
    );
    Ok(Json(enrollment))
}

async fn activate_totp(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<TotpActivationRequest>,
) -> Result<Json<TotpActivated>, ApiError> {
    user.require_scope(Scope::Write)?;
    let recovery_codes = mfa::activate(&state, user.id, &request.code).await?;
    let status = mfa::status(&state, user.id).await?;
    state.events.emit(
        Event::new(actions::MFA_ENABLED, user.actor())
            .target("user", user.id.to_string())
            .metadata(json!({
                "method": "totp",
                "recoveryCodes": recovery_codes.len(),
            })),
    );
    Ok(Json(TotpActivated {
        mfa: status,
        recovery_codes,
    }))
}

async fn disable_mfa(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<MfaDisableRequest>,
) -> Result<Json<MfaStatus>, ApiError> {
    user.require_scope(Scope::Write)?;
    mfa::require_password(&state, user.id, &request.password).await?;
    mfa::disable(&state, user.id).await?;
    // A removed factor changes what the account accepts: other devices sign in
    // again with the new rules.
    if let Some(jti) = user.jti.as_deref() {
        crate::auth::revoke_other_sessions(&state, user.id, jti, "mfa_disabled").await?;
    }
    state.events.emit(
        Event::new(actions::MFA_DISABLED, user.actor())
            .target("user", user.id.to_string())
            .metadata(json!({ "method": "totp" })),
    );
    Ok(Json(mfa::status(&state, user.id).await?))
}

async fn regenerate_recovery_codes(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<RecoveryCodesRequest>,
) -> Result<Json<RecoveryCodes>, ApiError> {
    user.require_scope(Scope::Write)?;
    mfa::require_password(&state, user.id, &request.password).await?;
    if !mfa::status(&state, user.id).await?.enabled {
        return Err(ApiError::bad_request(
            "mfa_not_enabled",
            "Enable two-factor authentication before generating recovery codes",
        ));
    }
    let codes = mfa::regenerate_codes(&state, user.id).await?;
    state.events.emit(
        Event::new(actions::MFA_RECOVERY_CODES_ROTATED, user.actor())
            .target("user", user.id.to_string())
            .metadata(json!({ "codes": codes.len() })),
    );
    Ok(Json(RecoveryCodes { codes }))
}

/// Signs out every other session of the caller.
async fn revoke_other_sessions(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    user.require_scope(Scope::Write)?;
    let revoked = match user.jti.as_deref() {
        Some(jti) => {
            crate::auth::revoke_other_sessions(&state, user.id, jti, "user_revoked").await?
        }
        // An API key has no session to keep; revoke them all.
        None => revoke_all_sessions(&state, user.id, "user_revoked").await?,
    };
    state.events.emit(
        Event::new(actions::SESSION_REVOKED, user.actor())
            .target("user", user.id.to_string())
            .metadata(json!({ "sessions": revoked, "reason": "user_revoked" })),
    );
    Ok(Json(json!({ "revoked": revoked })))
}

async fn fetch_user_row(state: &AppState, id: i64) -> Result<UserRow, ApiError> {
    db::query_as::<UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path,
                age_limit, created_at, updated_at, oidc_issuer, oidc_subject,
                onboarding_completed_at, onboarding_step,
                CAST(COALESCE((SELECT 1 FROM user_mfa m WHERE m.user_id = users.id AND m.enabled_at IS NOT NULL), 0) AS BIGINT) AS mfa_enabled,
                must_change_password
         FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(ApiError::unauthorized)
}
