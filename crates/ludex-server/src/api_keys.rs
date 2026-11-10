//! API key generation and bearer authentication.
//!
//! Keys are random 32-byte tokens rendered as `ldx_<hex>`. Only a SHA-256 of
//! the full token is stored: the secret has full entropy (it is not a
//! user-chosen password), so a fast hash is both safe and cheap enough to run
//! on every request. A short `prefix` makes the lookup a single indexed read
//! and is safe to show in the UI.

use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::db;
use crate::error::ApiError;
use crate::state::AppState;

/// Prefix of every Ludex API key.
pub const TOKEN_PREFIX: &str = "ldx_";
/// Number of leading token characters stored as the lookup prefix.
const PREFIX_LEN: usize = 16;
/// Number of random bytes in a key.
const TOKEN_BYTES: usize = 32;

/// Capability granted to an API key. `admin` implies `write`, which implies
/// `read`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Read-only access to the API.
    Read,
    /// Read plus mutating library/save operations.
    Write,
    /// Full access, including administrator endpoints.
    Admin,
}

impl Scope {
    /// Stable lowercase name stored in the database and returned by the API.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Write => "write",
            Scope::Admin => "admin",
        }
    }

    /// Parses a scope name.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "read" => Some(Scope::Read),
            "write" => Some(Scope::Write),
            "admin" => Some(Scope::Admin),
            _ => None,
        }
    }

    /// Whether a key holding `self` satisfies a requirement for `required`.
    #[must_use]
    pub fn allows(self, required: Scope) -> bool {
        self >= required
    }
}

/// A freshly generated key, before it is written to the database.
pub struct GeneratedKey {
    /// Plaintext token, returned to the caller exactly once.
    pub token: String,
    /// Non-secret lookup prefix.
    pub prefix: String,
    /// SHA-256 of the full token, hex encoded.
    pub secret_hash: String,
}

/// Generates a new random API key.
#[must_use]
pub fn generate() -> GeneratedKey {
    let mut bytes = [0_u8; TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token = format!("{TOKEN_PREFIX}{}", hex::encode(bytes));
    let prefix = token.get(..PREFIX_LEN).unwrap_or(TOKEN_PREFIX).to_string();
    let secret_hash = hash_token(&token);
    GeneratedKey {
        token,
        prefix,
        secret_hash,
    }
}

/// Hashes a token for storage or comparison.
#[must_use]
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Constant-time comparison between a presented token and a stored hash.
#[must_use]
pub fn verify_token(token: &str, stored_hash: &str) -> bool {
    let computed = hash_token(token);
    computed.as_bytes().ct_eq(stored_hash.as_bytes()).into()
}

/// Parses the JSON array stored in `api_keys.scopes`, dropping unknown values.
#[must_use]
pub fn parse_scopes(raw: &str) -> Vec<Scope> {
    let names = serde_json::from_str::<Vec<String>>(raw).unwrap_or_default();
    let mut scopes: Vec<Scope> = names.iter().filter_map(|name| Scope::parse(name)).collect();
    scopes.sort();
    scopes.dedup();
    scopes
}

/// Serialises scopes for the `api_keys.scopes` column.
#[must_use]
pub fn scopes_json(scopes: &[Scope]) -> String {
    let names: Vec<&str> = scopes.iter().map(|scope| scope.as_str()).collect();
    serde_json::to_string(&names).unwrap_or_else(|_| "[]".to_string())
}

/// A validated API key presented on a request.
#[derive(Debug, Clone)]
pub struct ApiKeyAuth {
    /// Key row id, used as the audit actor id.
    pub key_id: i64,
    /// Owning account.
    pub user_id: i64,
    /// Scopes granted to the key.
    pub scopes: Vec<Scope>,
}

impl ApiKeyAuth {
    /// Whether the key carries a given capability.
    #[must_use]
    pub fn allows(&self, required: Scope) -> bool {
        self.scopes.iter().any(|scope| scope.allows(required))
    }
}

/// Resolves an `ldx_` token into an [`ApiKeyAuth`].
///
/// Returns `Ok(None)` for unknown, revoked or malformed tokens so the caller
/// can answer with the same generic 401 as a bad JWT.
pub async fn authenticate(state: &AppState, token: &str) -> Result<Option<ApiKeyAuth>, ApiError> {
    if !token.starts_with(TOKEN_PREFIX) {
        return Ok(None);
    }
    let Some(prefix) = token.get(..PREFIX_LEN) else {
        return Ok(None);
    };
    let row = sqlx::query_as::<_, (i64, i64, String, String, Option<String>)>(
        "SELECT id, user_id, secret_hash, scopes, revoked_at FROM api_keys WHERE prefix = ?",
    )
    .bind(prefix)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let Some((key_id, user_id, secret_hash, scopes, revoked_at)) = row else {
        return Ok(None);
    };
    if revoked_at.is_some() || !verify_token(token, &secret_hash) {
        return Ok(None);
    }

    // Usage tracking is best effort: a failed timestamp update must not reject
    // an otherwise valid request.
    let now = db::db_time(db::now());
    if let Err(error) = sqlx::query("UPDATE api_keys SET last_used_at = ? WHERE id = ?")
        .bind(&now)
        .bind(key_id)
        .execute(&state.pool)
        .await
    {
        tracing::warn!(error = %error, key = key_id, "could not update API key usage");
    }

    Ok(Some(ApiKeyAuth {
        key_id,
        user_id,
        scopes: parse_scopes(&scopes),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_have_prefix_and_hash() {
        let first = generate();
        let second = generate();
        assert!(first.token.starts_with(TOKEN_PREFIX));
        assert_eq!(first.token.len(), TOKEN_PREFIX.len() + TOKEN_BYTES * 2);
        assert_eq!(first.prefix.len(), PREFIX_LEN);
        assert!(first.token.starts_with(&first.prefix));
        assert_eq!(first.secret_hash, hash_token(&first.token));
        assert_ne!(first.token, second.token);
        assert_ne!(first.secret_hash, second.secret_hash);
    }

    #[test]
    fn token_verification_is_exact() {
        let key = generate();
        assert!(verify_token(&key.token, &key.secret_hash));
        assert!(!verify_token(&format!("{}x", key.token), &key.secret_hash));
        assert!(!verify_token(&key.token, &hash_token("other")));
    }

    #[test]
    fn scopes_imply_lower_capabilities() {
        assert!(Scope::Admin.allows(Scope::Read));
        assert!(Scope::Admin.allows(Scope::Write));
        assert!(Scope::Admin.allows(Scope::Admin));
        assert!(Scope::Write.allows(Scope::Read));
        assert!(!Scope::Write.allows(Scope::Admin));
        assert!(!Scope::Read.allows(Scope::Write));
    }

    #[test]
    fn scope_json_roundtrip_drops_unknown_values() {
        let raw = scopes_json(&[Scope::Read, Scope::Write, Scope::Admin]);
        assert_eq!(raw, r#"["read","write","admin"]"#);
        assert_eq!(
            parse_scopes(r#"["admin","read","nope"]"#),
            vec![Scope::Read, Scope::Admin]
        );
        assert!(parse_scopes("not json").is_empty());
    }
}
