//! TOTP second factor and login challenges (#166).
//!
//! A local account with 2FA enabled never receives a session from the password
//! step: `POST /api/auth/login` stores a hashed, single-use challenge and
//! returns it without a token. Only a valid TOTP or recovery code exchanges the
//! challenge for a session.
//!
//! The TOTP secret is written to `user_mfa` in the clear (the server must be
//! able to compute the current code), but it is returned by exactly one
//! endpoint, `POST /api/auth/mfa/totp/enroll`, and never appears in an audit
//! event, a log line, or any read endpoint.

use chrono::{DateTime, Duration, Utc};
use rand::RngCore;

use crate::auth::verify_password;
use crate::db;
use crate::error::ApiError;
use crate::models::{MfaStatus, TotpEnrollment};
use crate::state::AppState;
use crate::totp;

/// How long a pending login challenge stays valid.
pub const CHALLENGE_TTL_MINUTES: i64 = 5;
/// Wrong codes accepted before the challenge is burned.
pub const CHALLENGE_MAX_ATTEMPTS: i64 = 5;
/// Accepted clock drift, in 30-second steps.
pub const DRIFT_STEPS: i64 = 1;

/// Row of `user_mfa`.
#[derive(Debug, sqlx::FromRow)]
struct MfaRow {
    totp_secret: Option<String>,
    enabled_at: Option<String>,
}

/// Refreshed status of the caller's second factor.
pub async fn status(state: &AppState, user_id: i64) -> Result<MfaStatus, ApiError> {
    let row = fetch_row(state, user_id).await?;
    let recovery_codes_remaining = db::query_scalar::<i64>(
        "SELECT COUNT(*) FROM mfa_recovery_codes WHERE user_id = ? AND used_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;
    let enabled = row
        .as_ref()
        .is_some_and(|row| row.enabled_at.is_some() && row.totp_secret.is_some());
    let pending = row
        .as_ref()
        .is_some_and(|row| row.enabled_at.is_none() && row.totp_secret.is_some());
    Ok(MfaStatus {
        enabled,
        recovery_codes_remaining,
        enabled_at: row
            .as_ref()
            .and_then(|row| row.enabled_at.as_deref())
            .and_then(db::parse_time),
        methods: if enabled {
            vec!["totp".to_string(), "recovery_codes".to_string()]
        } else {
            Vec::new()
        },
        pending,
    })
}

/// Starts (or restarts) a TOTP enrolment. Returns the secret exactly once.
pub async fn start_enrollment(
    state: &AppState,
    user_id: i64,
    issuer: &str,
    account: &str,
) -> Result<TotpEnrollment, ApiError> {
    if status(state, user_id).await?.enabled {
        return Err(ApiError::conflict(
            "mfa_already_enabled",
            "Two-factor authentication is already enabled; disable it before enrolling again",
        ));
    }
    let secret = totp::generate_secret();
    let now = db::db_time(db::now());
    db::query(
        "INSERT INTO user_mfa (user_id, totp_secret, enabled_at, last_used_step, created_at, updated_at)
         VALUES (?, ?, NULL, NULL, ?, ?)
         ON CONFLICT(user_id) DO UPDATE SET
             totp_secret = excluded.totp_secret,
             enabled_at = NULL,
             last_used_step = NULL,
             updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(&secret)
    .bind(&now)
    .bind(&now)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;

    Ok(TotpEnrollment {
        otpauth_uri: totp::provisioning_uri(issuer, account, &secret),
        secret,
        issuer: issuer.to_string(),
        account: account.to_string(),
        digits: totp::DIGITS,
        period: totp::PERIOD,
    })
}

/// Confirms a pending enrolment and issues the first recovery codes.
pub async fn activate(state: &AppState, user_id: i64, code: &str) -> Result<Vec<String>, ApiError> {
    let row = fetch_row(state, user_id).await?.ok_or_else(|| {
        ApiError::bad_request(
            "mfa_not_started",
            "Start the authenticator setup before entering a code",
        )
    })?;
    if row.enabled_at.is_some() {
        return Err(ApiError::conflict(
            "mfa_already_enabled",
            "Two-factor authentication is already enabled",
        ));
    }
    let secret = row.totp_secret.ok_or_else(|| {
        ApiError::bad_request("mfa_not_started", "Start the authenticator setup first")
    })?;
    totp::verify(&secret, code, Utc::now().timestamp(), DRIFT_STEPS).ok_or_else(invalid_code)?;
    let now = db::db_time(db::now());
    // The activation code is not a login: do not consume its time step, or
    // signing in immediately afterwards would look like a replay.
    db::query(
        "UPDATE user_mfa SET enabled_at = ?, last_used_step = NULL, updated_at = ? WHERE user_id = ?",
    )
    .bind(&now)
    .bind(&now)
    .bind(user_id)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    regenerate_codes(state, user_id).await
}

/// Removes the second factor and every recovery code. The caller must have
/// re-checked the account password.
pub async fn disable(state: &AppState, user_id: i64) -> Result<(), ApiError> {
    db::query("DELETE FROM user_mfa WHERE user_id = ?")
        .bind(user_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    db::query("DELETE FROM mfa_recovery_codes WHERE user_id = ?")
        .bind(user_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    Ok(())
}

/// Replaces the recovery-code set, returning the new plaintext codes once.
pub async fn regenerate_codes(state: &AppState, user_id: i64) -> Result<Vec<String>, ApiError> {
    let codes = totp::generate_recovery_codes(totp::RECOVERY_CODE_COUNT);
    let now = db::db_time(db::now());
    db::query("DELETE FROM mfa_recovery_codes WHERE user_id = ?")
        .bind(user_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    for code in &codes {
        db::query(
            "INSERT INTO mfa_recovery_codes (user_id, code_hash, created_at, used_at)
             VALUES (?, ?, ?, NULL)",
        )
        .bind(user_id)
        .bind(totp::hash_secret(&totp::normalize_recovery_code(code)))
        .bind(&now)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    }
    Ok(codes)
}

/// Creates a single-use login challenge for an account with 2FA.
pub async fn create_challenge(
    state: &AppState,
    user_id: i64,
    user_agent: Option<&str>,
    ip: Option<&str>,
) -> Result<(String, DateTime<Utc>), ApiError> {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token = hex::encode(bytes);
    let issued = Utc::now();
    let expires_at = issued + Duration::minutes(CHALLENGE_TTL_MINUTES);
    let user_agent = user_agent.map(|value| value.chars().take(400).collect::<String>());
    // Drop spent and expired challenges for this account so the table cannot
    // grow without bound.
    db::query(
        "DELETE FROM mfa_challenges WHERE user_id = ? AND (used_at IS NOT NULL OR expires_at < ?)",
    )
    .bind(user_id)
    .bind(db::db_time(issued))
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    db::query(
        "INSERT INTO mfa_challenges
            (user_id, token_hash, user_agent, ip, attempts, created_at, expires_at, used_at)
         VALUES (?, ?, ?, ?, 0, ?, ?, NULL)",
    )
    .bind(user_id)
    .bind(totp::hash_secret(&token))
    .bind(user_agent)
    .bind(ip)
    .bind(db::db_time(issued))
    .bind(db::db_time(expires_at))
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    Ok((token, expires_at))
}

/// A pending challenge resolved from its plaintext token.
#[derive(Debug)]
pub struct Challenge {
    /// Challenge row id.
    pub id: i64,
    /// Account the challenge belongs to.
    pub user_id: i64,
}

/// Resolves a challenge token, rejecting used, unknown and expired rows.
pub async fn find_challenge(state: &AppState, token: &str) -> Result<Challenge, ApiError> {
    let row = db::query_as::<(i64, i64, i64, Option<String>, Option<String>)>(
        "SELECT id, user_id, attempts, expires_at, used_at
         FROM mfa_challenges WHERE token_hash = ?",
    )
    .bind(totp::hash_secret(token.trim()))
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "invalid_challenge",
            "This sign-in challenge is unknown; sign in again",
        )
    })?;
    let (id, user_id, attempts, expires_at, used_at) = row;
    if used_at.is_some() {
        return Err(ApiError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "challenge_used",
            "This sign-in challenge was already used; sign in again",
        ));
    }
    if attempts >= CHALLENGE_MAX_ATTEMPTS {
        return Err(ApiError::too_many_requests(
            "mfa_attempts_exceeded",
            "Too many incorrect codes; sign in again",
        ));
    }
    let expired = expires_at
        .as_deref()
        .and_then(db::parse_time)
        .is_none_or(|expires| expires <= Utc::now());
    if expired {
        return Err(ApiError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "challenge_expired",
            "This sign-in challenge expired; sign in again",
        ));
    }
    Ok(Challenge { id, user_id })
}

/// Counts a wrong code and burns the challenge after the fifth failure.
pub async fn record_failed_attempt(state: &AppState, challenge_id: i64) -> Result<(), ApiError> {
    let now = db::db_time(db::now());
    db::query("UPDATE mfa_challenges SET attempts = attempts + 1 WHERE id = ?")
        .bind(challenge_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    // Burn the row like a used challenge once the budget is spent.
    db::query(
        "UPDATE mfa_challenges SET used_at = ? WHERE id = ? AND attempts >= ? AND used_at IS NULL",
    )
    .bind(&now)
    .bind(challenge_id)
    .bind(CHALLENGE_MAX_ATTEMPTS)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(())
}

/// Marks a challenge redeemed so it cannot be replayed.
pub async fn consume_challenge(state: &AppState, challenge_id: i64) -> Result<(), ApiError> {
    db::query("UPDATE mfa_challenges SET used_at = ? WHERE id = ? AND used_at IS NULL")
        .bind(db::db_time(db::now()))
        .bind(challenge_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    Ok(())
}

/// Verifies a TOTP code or a recovery code.
///
/// Returns the method that matched (`totp` or `recovery`) so the caller can
/// audit it. TOTP codes cannot be replayed: the accepted time step is stored.
pub async fn verify_second_factor(
    state: &AppState,
    user_id: i64,
    code: &str,
) -> Result<&'static str, ApiError> {
    if let Some(method) = verify_totp(state, user_id, code).await? {
        return Ok(method);
    }
    if verify_recovery_code(state, user_id, code).await? {
        return Ok("recovery");
    }
    Err(invalid_code())
}

async fn verify_totp(
    state: &AppState,
    user_id: i64,
    code: &str,
) -> Result<Option<&'static str>, ApiError> {
    let normalized = totp::normalize_code(code);
    if normalized.len() != totp::DIGITS as usize
        || !normalized.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Ok(None);
    }
    let Some(row) = fetch_row(state, user_id).await? else {
        return Ok(None);
    };
    let Some(secret) = row.totp_secret.filter(|_| row.enabled_at.is_some()) else {
        return Ok(None);
    };
    let Some(step) = totp::verify(&secret, &normalized, Utc::now().timestamp(), DRIFT_STEPS) else {
        return Ok(None);
    };
    let now = db::db_time(db::now());
    let accepted = db::query(
        "UPDATE user_mfa SET last_used_step = ?, updated_at = ?
         WHERE user_id = ? AND (last_used_step IS NULL OR last_used_step < ?)",
    )
    .bind(step)
    .bind(&now)
    .bind(user_id)
    .bind(step)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?
    .rows_affected();
    Ok((accepted == 1).then_some("totp"))
}

async fn verify_recovery_code(
    state: &AppState,
    user_id: i64,
    code: &str,
) -> Result<bool, ApiError> {
    let hash = totp::hash_secret(&totp::normalize_recovery_code(code));
    let id = db::query_scalar::<i64>(
        "SELECT id FROM mfa_recovery_codes
         WHERE user_id = ? AND code_hash = ? AND used_at IS NULL",
    )
    .bind(user_id)
    .bind(&hash)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?;
    let Some(id) = id else {
        return Ok(false);
    };
    let consumed =
        db::query("UPDATE mfa_recovery_codes SET used_at = ? WHERE id = ? AND used_at IS NULL")
            .bind(db::db_time(db::now()))
            .bind(id)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?
            .rows_affected();
    Ok(consumed == 1)
}

/// Re-checks the account password before a sensitive MFA change.
pub async fn require_password(
    state: &AppState,
    user_id: i64,
    password: &str,
) -> Result<(), ApiError> {
    let hash = db::query_scalar::<String>("SELECT password_hash FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(ApiError::unauthorized)?;
    if !verify_password(password, &hash) {
        return Err(ApiError::bad_request(
            "invalid_password",
            "Current password is incorrect",
        ));
    }
    Ok(())
}

async fn fetch_row(state: &AppState, user_id: i64) -> Result<Option<MfaRow>, ApiError> {
    db::query_as::<MfaRow>("SELECT totp_secret, enabled_at FROM user_mfa WHERE user_id = ?")
        .bind(user_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)
}

fn invalid_code() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::UNAUTHORIZED,
        "invalid_mfa_code",
        "That code is not valid; check the authenticator app and try again",
    )
}
