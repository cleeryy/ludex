//! Password hashing, JWT access tokens, download tickets and the `AuthUser`
//! request extractor.

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::Response;
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use ludex_core::models::Role;
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::api_keys::{self, Scope};
use crate::db;
use crate::error::ApiError;
use crate::events::Actor;
use crate::state::AppState;

/// Access token lifetime used by bootstrap tokens (the login path uses
/// `Config::jwt_ttl_minutes`).
const ACCESS_TOKEN_SECONDS: i64 = 7 * 24 * 60 * 60;
/// Download ticket lifetime.
const TICKET_SECONDS: i64 = 15 * 60;

/// Claims embedded in a normal API access token.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AccessClaims {
    sub: String,
    role: String,
    exp: i64,
    iat: i64,
    kind: String,
    /// Unique token id, used for revocation and session listing. Optional so
    /// tokens issued before `0011_api_security.sql` keep working.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    jti: Option<String>,
}

/// Claims embedded in a short-lived download ticket.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TicketClaims {
    sub: String,
    exp: i64,
    iat: i64,
    kind: String,
    game_id: i64,
    version_id: i64,
}

/// Authenticated caller, resolved from a Bearer token and the database.
#[derive(Debug, Clone)]
pub struct AuthUser {
    /// Account id.
    pub id: i64,
    /// Login name.
    pub username: String,
    /// Role from the database (authoritative over the token claim).
    pub role: Role,
    /// API key id when the request used `Authorization: Bearer ldx_...`.
    pub api_key_id: Option<i64>,
    /// Scopes of the API key; `None` for JWT sessions, which are unrestricted.
    pub scopes: Option<Vec<Scope>>,
    /// Session/token id (`jti`) for JWT credentials; `None` for API keys.
    pub jti: Option<String>,
    /// Maximum age rating this account may access; `None` = unrestricted (#39).
    pub age_limit: Option<u8>,
}

impl AuthUser {
    /// Whether the caller is an administrator.
    #[must_use]
    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }

    /// Whether the credential carries a capability. JWTs carry every scope.
    #[must_use]
    pub fn has_scope(&self, required: Scope) -> bool {
        match &self.scopes {
            None => true,
            Some(scopes) => scopes.iter().any(|scope| scope.allows(required)),
        }
    }

    /// Rejects the request unless the credential carries `required`.
    pub fn require_scope(&self, required: Scope) -> Result<(), ApiError> {
        if self.has_scope(required) {
            Ok(())
        } else {
            Err(ApiError::forbidden(
                "insufficient_scope",
                format!("This API key is missing the '{}' scope", required.as_str()),
            ))
        }
    }

    /// Audit actor for this caller.
    #[must_use]
    pub fn actor(&self) -> Actor {
        match self.api_key_id {
            Some(id) => Actor::ApiKey { id },
            None => Actor::User { id: self.id },
        }
    }

    /// Rejects the request unless the caller is an administrator.
    pub fn require_admin(&self) -> Result<(), ApiError> {
        if self.role == Role::Admin {
            Ok(())
        } else {
            Err(ApiError::forbidden(
                "forbidden",
                "Administrator role required",
            ))
        }
    }

    /// Rejects the request unless the caller is an administrator whose
    /// credential carries the `admin` scope.
    pub fn require_admin_access(&self) -> Result<(), ApiError> {
        self.require_admin()?;
        self.require_scope(Scope::Admin)
    }

    /// Rejects the request unless the caller can edit the library.
    pub fn require_editor(&self) -> Result<(), ApiError> {
        if matches!(self.role, Role::Admin | Role::Editor) {
            self.require_scope(Scope::Write)
        } else {
            Err(ApiError::forbidden("forbidden", "Editor role required"))
        }
    }
}

/// Hashes a password with Argon2id and a random salt.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|error| anyhow::anyhow!("hash password: {error}"))?;
    Ok(hash.to_string())
}

/// Verifies a password against an Argon2 hash.
#[must_use]
pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Generates a random alphanumeric password for auto-provisioned accounts.
#[must_use]
pub fn generate_password() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

/// A freshly issued access token and its server-side metadata.
#[derive(Debug, Clone)]
pub struct IssuedToken {
    /// Signed JWT.
    pub token: String,
    /// Unique token id, stored in `auth_sessions`.
    pub jti: String,
    /// Expiry timestamp.
    pub expires_at: DateTime<Utc>,
}

/// Issues an access token with the given lifetime and a fresh `jti`.
pub fn issue_access_token_with_ttl(
    secret: &[u8],
    user_id: i64,
    role: Role,
    ttl: Duration,
) -> anyhow::Result<IssuedToken> {
    let issued = Utc::now();
    let expires_at = issued + ttl;
    let jti = uuid::Uuid::new_v4().to_string();
    let claims = AccessClaims {
        sub: user_id.to_string(),
        role: role.as_str().to_string(),
        exp: expires_at.timestamp(),
        iat: issued.timestamp(),
        kind: "access".to_string(),
        jti: Some(jti.clone()),
    };
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret),
    )
    .map_err(|error| anyhow::anyhow!("encode access token: {error}"))?;
    Ok(IssuedToken {
        token,
        jti,
        expires_at,
    })
}

/// Issues a 7-day access token (bootstrap and tests).
pub fn issue_access_token(secret: &[u8], user_id: i64, role: Role) -> anyhow::Result<String> {
    issue_access_token_with_ttl(
        secret,
        user_id,
        role,
        Duration::seconds(ACCESS_TOKEN_SECONDS),
    )
    .map(|issued| issued.token)
}

/// Client metadata recorded with a login session.
#[derive(Debug, Clone, Default)]
pub struct SessionInfo {
    /// Submitted `User-Agent`, truncated before storage.
    pub user_agent: Option<String>,
    /// Client IP address, textual form.
    pub ip: Option<String>,
}

/// Creates a session row and issues its token.
pub async fn start_session(
    state: &AppState,
    user_id: i64,
    role: Role,
    info: &SessionInfo,
) -> Result<(IssuedToken, i64), ApiError> {
    let ttl = Duration::minutes(
        i64::try_from(state.config.jwt_ttl_minutes).unwrap_or(ACCESS_TOKEN_SECONDS / 60),
    );
    let issued = issue_access_token_with_ttl(&state.jwt_secret, user_id, role, ttl)?;
    let now = db::db_time(db::now());
    let expires_at = db::db_time(issued.expires_at);
    let user_agent = info
        .user_agent
        .as_deref()
        .map(|value| value.chars().take(400).collect::<String>());
    let result = sqlx::query(
        "INSERT INTO auth_sessions
            (user_id, jti, user_agent, ip, created_at, last_seen_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(&issued.jti)
    .bind(user_agent)
    .bind(info.ip.as_deref())
    .bind(&now)
    .bind(&now)
    .bind(&expires_at)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    Ok((issued, result.last_insert_rowid()))
}

/// Revokes one token id. Idempotent.
pub async fn revoke_token(
    state: &AppState,
    jti: &str,
    user_id: i64,
    reason: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO token_revocations (jti, user_id, reason, revoked_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(jti) DO UPDATE SET reason = excluded.reason",
    )
    .bind(jti)
    .bind(user_id)
    .bind(reason)
    .bind(db::db_time(db::now()))
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    sqlx::query("UPDATE auth_sessions SET revoked_at = COALESCE(revoked_at, ?) WHERE jti = ?")
        .bind(db::db_time(db::now()))
        .bind(jti)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    Ok(())
}

/// Revokes every session of an account except `keep_jti`.
///
/// Used after a password change so the calling session keeps working while
/// every other device is signed out.
pub async fn revoke_other_sessions(
    state: &AppState,
    user_id: i64,
    keep_jti: &str,
    reason: &str,
) -> Result<u64, ApiError> {
    let now_text = db::db_time(db::now());
    let active = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM auth_sessions
         WHERE user_id = ? AND jti != ? AND revoked_at IS NULL AND expires_at > ?",
    )
    .bind(user_id)
    .bind(keep_jti)
    .bind(&now_text)
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;
    sqlx::query(
        "INSERT INTO token_revocations (jti, user_id, reason, revoked_at)
         SELECT jti, user_id, ?, ? FROM auth_sessions
         WHERE user_id = ? AND jti != ? AND revoked_at IS NULL
         ON CONFLICT(jti) DO UPDATE SET reason = excluded.reason",
    )
    .bind(reason)
    .bind(&now_text)
    .bind(user_id)
    .bind(keep_jti)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    sqlx::query(
        "UPDATE auth_sessions SET revoked_at = ?
         WHERE user_id = ? AND jti != ? AND revoked_at IS NULL",
    )
    .bind(&now_text)
    .bind(user_id)
    .bind(keep_jti)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(u64::try_from(active).unwrap_or(0))
}

/// Revokes every active session of an account and invalidates older tokens.
///
/// Returns the number of sessions that were still active.
pub async fn revoke_all_sessions(
    state: &AppState,
    user_id: i64,
    reason: &str,
) -> Result<u64, ApiError> {
    let now = db::now();
    let now_text = db::db_time(now);
    let active = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM auth_sessions
         WHERE user_id = ? AND revoked_at IS NULL AND expires_at > ?",
    )
    .bind(user_id)
    .bind(&now_text)
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;
    sqlx::query(
        "INSERT INTO token_revocations (jti, user_id, reason, revoked_at)
         SELECT jti, user_id, ?, ? FROM auth_sessions
         WHERE user_id = ? AND revoked_at IS NULL
         ON CONFLICT(jti) DO UPDATE SET reason = excluded.reason",
    )
    .bind(reason)
    .bind(&now_text)
    .bind(user_id)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    sqlx::query("UPDATE auth_sessions SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL")
        .bind(&now_text)
        .bind(user_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    // Invalidates every JWT issued before this instant, including tokens
    // without a `jti` (issued before the sessions table existed).
    sqlx::query("UPDATE users SET tokens_valid_after = ?, updated_at = ? WHERE id = ?")
        .bind(&now_text)
        .bind(&now_text)
        .bind(user_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    Ok(u64::try_from(active).unwrap_or(0))
}

/// Issues a 15-minute download ticket for a specific version.
pub fn issue_download_ticket(
    secret: &[u8],
    user_id: i64,
    game_id: i64,
    version_id: i64,
) -> anyhow::Result<(String, DateTime<Utc>)> {
    let issued = Utc::now();
    let expires_at = issued + Duration::seconds(TICKET_SECONDS);
    let claims = TicketClaims {
        sub: user_id.to_string(),
        exp: expires_at.timestamp(),
        iat: issued.timestamp(),
        kind: "download".to_string(),
        game_id,
        version_id,
    };
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret),
    )
    .map_err(|error| anyhow::anyhow!("encode download ticket: {error}"))?;
    Ok((token, expires_at))
}

/// Decoded download ticket.
#[derive(Debug, Clone, Copy)]
pub struct DownloadTicket {
    /// Requesting account.
    pub user_id: i64,
    /// Game the ticket was issued for.
    pub game_id: i64,
    /// Version the ticket was issued for.
    pub version_id: i64,
}

/// Decodes and validates a download ticket.
pub fn decode_download_ticket(secret: &[u8], token: &str) -> Option<DownloadTicket> {
    let validation = Validation::new(Algorithm::HS256);
    let data =
        decode::<TicketClaims>(token, &DecodingKey::from_secret(secret), &validation).ok()?;
    if data.claims.kind != "download" {
        return None;
    }
    Some(DownloadTicket {
        user_id: data.claims.sub.parse().ok()?,
        game_id: data.claims.game_id,
        version_id: data.claims.version_id,
    })
}

fn decode_access(secret: &[u8], token: &str) -> Option<AccessClaims> {
    let validation = Validation::new(Algorithm::HS256);
    let data =
        decode::<AccessClaims>(token, &DecodingKey::from_secret(secret), &validation).ok()?;
    (data.claims.kind == "access").then_some(data.claims)
}

/// Extracts a Bearer token from an `Authorization` header value.
#[must_use]
pub fn bearer_token(headers: &axum::http::HeaderMap) -> Option<String> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let token = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// Resolves an access token into an [`AuthUser`], checking the database.
///
/// `ldx_`-prefixed tokens are API keys; everything else is a JWT.
pub async fn authenticate_token(state: &AppState, token: &str) -> Result<AuthUser, ApiError> {
    if token.starts_with(api_keys::TOKEN_PREFIX) {
        let key = api_keys::authenticate(state, token)
            .await?
            .ok_or_else(ApiError::unauthorized)?;
        return load_auth_user(state, key.user_id, Some(key.key_id), Some(key.scopes)).await;
    }

    let claims = decode_access(&state.jwt_secret, token).ok_or_else(ApiError::unauthorized)?;
    let user_id = claims
        .sub
        .parse::<i64>()
        .map_err(|_| ApiError::unauthorized())?;
    let jti = claims.jti.clone();
    let issued_at = DateTime::from_timestamp(claims.iat, 0).unwrap_or_else(Utc::now);

    let row = sqlx::query_as::<_, RevocationRow>(
        "SELECT u.id, u.username, u.role, u.activated, u.age_limit, u.tokens_valid_after,
                s.revoked_at AS session_revoked,
                (SELECT 1 FROM token_revocations r WHERE r.jti = ?) AS token_revoked
         FROM users u
         LEFT JOIN auth_sessions s ON s.jti = ? AND s.user_id = u.id
         WHERE u.id = ?",
    )
    .bind(jti.as_deref())
    .bind(jti.as_deref())
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(ApiError::unauthorized)?;

    if row.token_revoked != 0 || row.session_revoked.is_some() {
        return Err(ApiError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "session_revoked",
            "This session was revoked; sign in again",
        ));
    }
    if let Some(valid_after) = row.tokens_valid_after.as_deref().and_then(db::parse_time) {
        if issued_at < valid_after {
            return Err(ApiError::new(
                axum::http::StatusCode::UNAUTHORIZED,
                "session_revoked",
                "This session is no longer valid; sign in again",
            ));
        }
    }
    if row.activated == 0 {
        return Err(ApiError::forbidden(
            "account_disabled",
            "This account is disabled",
        ));
    }

    if let Some(jti) = jti.as_deref() {
        touch_session(state, jti).await;
    }

    Ok(AuthUser {
        id: row.id,
        username: row.username,
        role: row.role.parse::<Role>().unwrap_or(Role::User),
        api_key_id: None,
        scopes: None,
        jti,
        age_limit: row.age_limit.and_then(|value| u8::try_from(value).ok()),
    })
}

/// Row used by [`authenticate_token`] to check revocation in one query.
#[derive(Debug, sqlx::FromRow)]
struct RevocationRow {
    id: i64,
    username: String,
    role: String,
    activated: i64,
    age_limit: Option<i64>,
    tokens_valid_after: Option<String>,
    session_revoked: Option<String>,
    token_revoked: i64,
}

/// Updates `last_seen_at` at most once a minute per token.
async fn touch_session(state: &AppState, jti: &str) {
    let now = Utc::now().timestamp();
    {
        let mut seen = state
            .session_touch
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(previous) = seen.get(jti) {
            if now - previous < 60 {
                return;
            }
        }
        if seen.len() > 10_000 {
            seen.retain(|_, timestamp| now - *timestamp < 3600);
        }
        seen.insert(jti.to_string(), now);
    }
    let _ = sqlx::query("UPDATE auth_sessions SET last_seen_at = ? WHERE jti = ?")
        .bind(db::db_time(db::now()))
        .bind(jti)
        .execute(&state.pool)
        .await;
}

async fn load_auth_user(
    state: &AppState,
    user_id: i64,
    api_key_id: Option<i64>,
    scopes: Option<Vec<Scope>>,
) -> Result<AuthUser, ApiError> {
    let row = sqlx::query_as::<_, crate::models::UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path,
                age_limit, created_at, updated_at, oidc_issuer, oidc_subject
         FROM users WHERE id = ?",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(ApiError::unauthorized)?;

    if row.activated == 0 {
        return Err(ApiError::forbidden(
            "account_disabled",
            "This account is disabled",
        ));
    }

    Ok(AuthUser {
        id: row.id,
        username: row.username,
        role: row.role.parse::<Role>().unwrap_or(Role::User),
        api_key_id,
        scopes,
        jti: None,
        age_limit: row.age_limit.and_then(|value| u8::try_from(value).ok()),
    })
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer_token(&parts.headers).ok_or_else(ApiError::unauthorized)?;
        authenticate_token(state, &token).await
    }
}

/// Middleware guarding routers that are built outside this crate (ingest).
///
/// Requires a valid JWT and the `admin` or `editor` role, matching the library
/// scan policy.
pub async fn editor_only(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let token = bearer_token(request.headers()).ok_or_else(ApiError::unauthorized)?;
    let user = authenticate_token(&state, &token).await?;
    user.require_editor()?;
    request.extensions_mut().insert(user);
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_roundtrip() {
        let hash = hash_password("correct horse battery staple").expect("hash");
        assert!(verify_password("correct horse battery staple", &hash));
        assert!(!verify_password("wrong", &hash));
        assert!(!verify_password("", "not-a-hash"));
    }

    #[test]
    fn generated_passwords_are_long_and_random() {
        let first = generate_password();
        let second = generate_password();
        assert_eq!(first.len(), 32);
        assert_ne!(first, second);
        assert!(first.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn access_token_roundtrip() {
        let secret = b"test-secret";
        let token = issue_access_token(secret, 42, Role::Editor).expect("token");
        let claims = decode_access(secret, &token).expect("claims");
        assert_eq!(claims.sub, "42");
        assert_eq!(claims.role, "editor");
        assert_eq!(claims.kind, "access");
        assert!(decode_access(b"other-secret", &token).is_none());
    }

    #[test]
    fn ticket_roundtrip_and_kind_separation() {
        let secret = b"test-secret";
        let (ticket, expires_at) = issue_download_ticket(secret, 7, 3, 9).expect("ticket");
        let decoded = decode_download_ticket(secret, &ticket).expect("decode");
        assert_eq!(decoded.user_id, 7);
        assert_eq!(decoded.game_id, 3);
        assert_eq!(decoded.version_id, 9);
        assert!(expires_at > Utc::now());

        let access = issue_access_token(secret, 7, Role::User).expect("access");
        assert!(decode_download_ticket(secret, &access).is_none());
    }

    #[test]
    fn bearer_header_parsing() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(AUTHORIZATION, "Bearer abc".parse().expect("header"));
        assert_eq!(bearer_token(&headers).as_deref(), Some("abc"));

        headers.insert(AUTHORIZATION, "Basic abc".parse().expect("header"));
        assert!(bearer_token(&headers).is_none());
    }
}
