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

use crate::error::ApiError;
use crate::state::AppState;

/// Access token lifetime.
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
}

impl AuthUser {
    /// Whether the caller is an administrator.
    #[must_use]
    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
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

    /// Rejects the request unless the caller can edit the library.
    pub fn require_editor(&self) -> Result<(), ApiError> {
        if matches!(self.role, Role::Admin | Role::Editor) {
            Ok(())
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

/// Issues a 7-day access token.
pub fn issue_access_token(secret: &[u8], user_id: i64, role: Role) -> anyhow::Result<String> {
    let issued = Utc::now();
    let claims = AccessClaims {
        sub: user_id.to_string(),
        role: role.as_str().to_string(),
        exp: (issued + Duration::seconds(ACCESS_TOKEN_SECONDS)).timestamp(),
        iat: issued.timestamp(),
        kind: "access".to_string(),
    };
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret),
    )
    .map_err(|error| anyhow::anyhow!("encode access token: {error}"))
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
pub async fn authenticate_token(state: &AppState, token: &str) -> Result<AuthUser, ApiError> {
    let claims = decode_access(&state.jwt_secret, token).ok_or_else(ApiError::unauthorized)?;
    let row = sqlx::query_as::<_, crate::models::UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path, created_at, updated_at
         FROM users WHERE id = ?",
    )
    .bind(claims.sub.parse::<i64>().map_err(|_| ApiError::unauthorized())?)
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
