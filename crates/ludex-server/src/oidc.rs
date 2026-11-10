//! OpenID Connect authorization-code flow with PKCE.
//!
//! The implementation is deliberately small: discovery, authorization URL
//! construction, token exchange and ID-token verification against the
//! provider's JWKS. It never stores provider tokens and never logs them.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{Context, Result as AnyResult};
use axum::http::{HeaderMap, StatusCode};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hkdf::Hkdf;
use jsonwebtoken::jwk::{Jwk, JwkSet, PublicKeyUse};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use ludex_core::models::Role;
use rand::RngCore;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use subtle::ConstantTimeEq;
use url::Url;

use crate::audit::actions;
use crate::config::Config;
use crate::db;
use crate::error::ApiError;
use crate::events::{Actor, Event};
use crate::state::AppState;

/// Pending authorization request lifetime.
const PENDING_TTL: Duration = Duration::from_secs(10 * 60);
/// One-time exchange code lifetime.
const CODE_TTL: Duration = Duration::from_secs(60);
/// Maximum pending authorization requests kept in memory.
const MAX_PENDING: usize = 1024;
/// Maximum one-time codes kept in memory.
const MAX_CODES: usize = 1024;
/// JWKS cache lifetime.
const JWKS_TTL: Duration = Duration::from_secs(60 * 60);
/// Timeout for the discovery request made when saving or testing settings.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
/// HKDF salt for the client-secret encryption key.
const SECRET_HKDF_SALT: &[u8] = b"ludex-oidc-settings-v1";
/// HKDF info for the client-secret encryption key.
const SECRET_HKDF_INFO: &[u8] = b"oidc-client-secret";
/// Additional authenticated data bound to every encrypted secret.
const SECRET_AAD: &[u8] = b"ludex.oidc.client-secret.v1";
/// Prefix of the stored ciphertext format.
const SECRET_PREFIX: &str = "v1.";

/// OIDC provider settings, as saved from the admin UI **without** the secret.
///
/// [`client_secret`](Self::client_secret) is only populated in memory; it is
/// never serialised by any endpoint. `secret_configured` tells the UI whether
/// a secret is stored at rest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OidcSettings {
    /// Whether SSO is offered on the login page.
    pub enabled: bool,
    /// Issuer URL (the base of the discovery document).
    pub issuer: Option<String>,
    /// OAuth client id.
    pub client_id: Option<String>,
    /// Registered redirect URI; relative paths are resolved against the
    /// public base.
    pub redirect_url: Option<String>,
    /// Public origin used to build the redirect URI.
    pub public_base_url: Option<String>,
    /// Space-separated scopes.
    pub scopes: String,
    /// Label of the login button.
    pub display_name: String,
    /// Create local accounts for unknown identities.
    pub auto_create: bool,
    /// Role for provisioned accounts that are not in an admin group.
    pub default_role: Role,
    /// Claim (dot-separated path) holding group/role values.
    pub groups_claim: String,
    /// Group/role values that grant the admin role.
    pub admin_groups: Vec<String>,
    /// Plaintext client secret. In memory only, never serialised.
    #[serde(skip)]
    pub client_secret: Option<String>,
    /// Whether a secret is stored at rest.
    #[serde(default)]
    pub secret_configured: bool,
}

impl Default for OidcSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            issuer: None,
            client_id: None,
            redirect_url: None,
            public_base_url: None,
            scopes: "openid profile email".to_string(),
            display_name: "SSO".to_string(),
            auto_create: true,
            default_role: Role::User,
            groups_claim: "groups".to_string(),
            admin_groups: Vec::new(),
            client_secret: None,
            secret_configured: false,
        }
    }
}

/// Settings loaded from the `oidc_settings` row.
#[derive(Debug, Clone)]
pub struct StoredOidcSettings {
    /// Non-secret fields plus the decrypted secret when available.
    pub settings: OidcSettings,
    /// Ciphertext kept so a save can preserve the secret without re-entering it.
    pub encrypted_secret: Option<String>,
    /// Whether the stored ciphertext could not be decrypted (server secret
    /// rotated).
    pub secret_error: bool,
}

/// Settings after applying environment precedence.
#[derive(Debug, Clone)]
pub struct ResolvedOidcSettings {
    /// Effective settings, secret decrypted where applicable.
    pub settings: OidcSettings,
    /// camelCase names of the fields pinned by `LUDEX_OIDC_*`.
    pub env_locked_fields: Vec<String>,
    /// Ciphertext of the stored secret, when the row is the source.
    pub encrypted_secret: Option<String>,
    /// Whether the stored secret exists but cannot be decrypted.
    pub secret_error: bool,
    /// Whether a saved row exists at all.
    pub stored: bool,
}

/// Discovery endpoints returned by the test/save endpoints.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OidcDiscoveryInfo {
    /// Provider issuer echoed by the document.
    pub issuer: String,
    /// Authorization endpoint.
    pub authorization_endpoint: String,
    /// Token endpoint.
    pub token_endpoint: String,
    /// JWKS endpoint.
    pub jwks_uri: String,
    /// Supported ID-token signing algorithms.
    pub id_token_signing_alg_values_supported: Vec<String>,
    /// Supported token endpoint authentication methods.
    pub token_endpoint_auth_methods_supported: Vec<String>,
}

/// Last discovery check recorded for the admin status card.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OidcStatus {
    /// Whether the last check succeeded. Only meaningful when `checked_at` is set.
    pub ok: bool,
    /// Issuer of the last check.
    pub issuer: Option<String>,
    /// RFC 3339 timestamp of the last check.
    pub checked_at: Option<String>,
    /// Provider error from the last failed check.
    pub error: Option<String>,
}

impl OidcStatus {
    /// Initial status for a freshly started server (no network check yet).
    #[must_use]
    pub fn initial(client: Option<&OidcClient>) -> Self {
        Self {
            ok: client.is_some(),
            issuer: client.map(|client| client.issuer.clone()),
            checked_at: None,
            error: None,
        }
    }
}

/// Derives the AES-256-GCM key from the server secret.
fn derive_secret_key(server_secret: &[u8]) -> [u8; 32] {
    let hkdf = Hkdf::<Sha256>::new(Some(SECRET_HKDF_SALT), server_secret);
    let mut key = [0_u8; 32];
    hkdf.expand(SECRET_HKDF_INFO, &mut key)
        .expect("32 bytes is a valid HKDF output length");
    key
}

/// Encrypts a client secret with a key derived from the server secret.
///
/// The returned value is `v1.<base64url(nonce || ciphertext)>`; the nonce is
/// random for every write, so saving the same secret twice produces different
/// ciphertext.
pub fn encrypt_secret(server_secret: &[u8], plaintext: &str) -> AnyResult<String> {
    let cipher = Aes256Gcm::new_from_slice(&derive_secret_key(server_secret))
        .context("build AES-256-GCM cipher")?;
    let mut nonce_bytes = [0_u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: plaintext.as_bytes(),
                aad: SECRET_AAD,
            },
        )
        .map_err(|_| anyhow::anyhow!("encrypt OIDC client secret"))?;
    let mut blob = Vec::with_capacity(12 + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    Ok(format!("{SECRET_PREFIX}{}", URL_SAFE_NO_PAD.encode(blob)))
}

/// Decrypts a secret produced by [`encrypt_secret`].
pub fn decrypt_secret(server_secret: &[u8], stored: &str) -> AnyResult<String> {
    let encoded = stored
        .strip_prefix(SECRET_PREFIX)
        .context("unsupported OIDC secret format")?;
    let blob = URL_SAFE_NO_PAD
        .decode(encoded)
        .context("decode OIDC secret")?;
    if blob.len() <= 12 {
        anyhow::bail!("OIDC secret ciphertext is truncated");
    }
    let (nonce_bytes, ciphertext) = blob.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(&derive_secret_key(server_secret))
        .context("build AES-256-GCM cipher")?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(nonce_bytes),
            Payload {
                msg: ciphertext,
                aad: SECRET_AAD,
            },
        )
        .map_err(|_| anyhow::anyhow!("decrypt OIDC client secret (server secret changed?)"))?;
    String::from_utf8(plaintext).context("OIDC secret is not valid UTF-8")
}

/// Names of the settings that differ between two snapshots. The secret is
/// never compared by value: callers pass `secret_changed` explicitly.
#[must_use]
pub fn changed_fields(
    previous: &OidcSettings,
    next: &OidcSettings,
    secret_changed: bool,
) -> Vec<String> {
    let mut fields = Vec::new();
    let mut push = |name: &str, changed: bool| {
        if changed {
            fields.push(name.to_string());
        }
    };
    push("enabled", previous.enabled != next.enabled);
    push(
        "issuer",
        previous.issuer.as_deref().map(str::trim) != next.issuer.as_deref().map(str::trim),
    );
    push(
        "clientId",
        previous.client_id.as_deref().map(str::trim) != next.client_id.as_deref().map(str::trim),
    );
    push(
        "redirectUrl",
        previous.redirect_url.as_deref().map(str::trim)
            != next.redirect_url.as_deref().map(str::trim),
    );
    push(
        "publicBaseUrl",
        previous.public_base_url.as_deref().map(str::trim)
            != next.public_base_url.as_deref().map(str::trim),
    );
    push("scopes", previous.scopes.trim() != next.scopes.trim());
    push(
        "displayName",
        previous.display_name.trim() != next.display_name.trim(),
    );
    push("autoCreate", previous.auto_create != next.auto_create);
    push("defaultRole", previous.default_role != next.default_role);
    push(
        "groupsClaim",
        previous.groups_claim.trim() != next.groups_claim.trim(),
    );
    push("adminGroups", previous.admin_groups != next.admin_groups);
    if secret_changed {
        fields.push("clientSecret".to_string());
    }
    fields
}

/// Loads the saved row and decrypts the secret when possible.
///
/// A decryption failure is not an error here: `secret_error` is set so the
/// admin UI can ask for the secret again instead of losing the whole config.
pub async fn load_settings(
    pool: &SqlitePool,
    server_secret: &[u8],
) -> AnyResult<Option<StoredOidcSettings>> {
    let row = sqlx::query_as::<_, OidcSettingsRow>(
        "SELECT enabled, issuer, client_id, redirect_url, public_base_url, scopes,
                display_name, auto_create, default_role, groups_claim, admin_groups,
                client_secret_enc, updated_at
         FROM oidc_settings WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .context("read OIDC settings")?;
    let Some(row) = row else {
        return Ok(None);
    };

    let mut settings = OidcSettings {
        enabled: row.enabled != 0,
        issuer: row.issuer,
        client_id: row.client_id,
        redirect_url: row.redirect_url,
        public_base_url: row.public_base_url,
        scopes: row.scopes,
        display_name: row.display_name,
        auto_create: row.auto_create != 0,
        default_role: row.default_role.parse().unwrap_or(Role::User),
        groups_claim: row.groups_claim,
        admin_groups: db::parse_json_list(&row.admin_groups),
        client_secret: None,
        secret_configured: row.client_secret_enc.is_some(),
    };

    let mut secret_error = false;
    if let Some(encrypted) = &row.client_secret_enc {
        match decrypt_secret(server_secret, encrypted) {
            Ok(plaintext) => settings.client_secret = Some(plaintext),
            Err(error) => {
                tracing::warn!(error = %error, "cannot decrypt the stored OIDC client secret");
                secret_error = true;
            }
        }
    }

    Ok(Some(StoredOidcSettings {
        settings,
        encrypted_secret: row.client_secret_enc,
        secret_error,
    }))
}

/// Writes the single settings row. `encrypted_secret` is `None` when no secret
/// is configured (public PKCE client or cleared).
pub async fn save_settings(
    pool: &SqlitePool,
    settings: &OidcSettings,
    encrypted_secret: Option<&str>,
) -> AnyResult<()> {
    sqlx::query(
        "INSERT INTO oidc_settings
            (id, enabled, issuer, client_id, redirect_url, public_base_url, scopes,
             display_name, auto_create, default_role, groups_claim, admin_groups,
             client_secret_enc, updated_at)
         VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
             enabled = excluded.enabled,
             issuer = excluded.issuer,
             client_id = excluded.client_id,
             redirect_url = excluded.redirect_url,
             public_base_url = excluded.public_base_url,
             scopes = excluded.scopes,
             display_name = excluded.display_name,
             auto_create = excluded.auto_create,
             default_role = excluded.default_role,
             groups_claim = excluded.groups_claim,
             admin_groups = excluded.admin_groups,
             client_secret_enc = excluded.client_secret_enc,
             updated_at = excluded.updated_at",
    )
    .bind(i64::from(settings.enabled))
    .bind(settings.issuer.as_deref())
    .bind(settings.client_id.as_deref())
    .bind(settings.redirect_url.as_deref())
    .bind(settings.public_base_url.as_deref())
    .bind(&settings.scopes)
    .bind(&settings.display_name)
    .bind(i64::from(settings.auto_create))
    .bind(settings.default_role.as_str())
    .bind(&settings.groups_claim)
    .bind(db::dump_json_list(&settings.admin_groups))
    .bind(encrypted_secret)
    .bind(db::db_time(db::now()))
    .execute(pool)
    .await
    .context("write OIDC settings")?;
    Ok(())
}

/// Deletes the saved row, restoring environment-only configuration.
pub async fn delete_settings(pool: &SqlitePool) -> AnyResult<()> {
    sqlx::query("DELETE FROM oidc_settings WHERE id = 1")
        .execute(pool)
        .await
        .context("delete OIDC settings")?;
    Ok(())
}

#[derive(Debug, sqlx::FromRow)]
struct OidcSettingsRow {
    enabled: i64,
    issuer: Option<String>,
    client_id: Option<String>,
    redirect_url: Option<String>,
    public_base_url: Option<String>,
    scopes: String,
    display_name: String,
    auto_create: i64,
    default_role: String,
    groups_claim: String,
    admin_groups: String,
    client_secret_enc: Option<String>,
    #[allow(dead_code)]
    updated_at: String,
}

/// Static configuration as settings. Used when no row is saved (environment
/// only or desktop) and by [`OidcClient::from_config`].
#[must_use]
pub fn settings_from_config(config: &Config) -> OidcSettings {
    let secret = non_empty(config.oidc_client_secret.as_deref());
    OidcSettings {
        enabled: non_empty(config.oidc_issuer.as_deref()).is_some(),
        issuer: config.oidc_issuer.clone(),
        client_id: config.oidc_client_id.clone(),
        redirect_url: config.oidc_redirect_url.clone(),
        public_base_url: config.oidc_public_base_url.clone(),
        scopes: config.oidc_scopes.clone(),
        display_name: config.oidc_display_name.clone(),
        auto_create: config.oidc_auto_create,
        default_role: config.oidc_default_role,
        groups_claim: config.oidc_groups_claim.clone(),
        admin_groups: config.oidc_admin_groups.clone(),
        secret_configured: secret.is_some(),
        client_secret: secret,
    }
}

/// Applies `LUDEX_OIDC_*` precedence over the saved row, per field.
#[must_use]
pub fn resolve_settings(
    stored: Option<&StoredOidcSettings>,
    config: &Config,
) -> ResolvedOidcSettings {
    // With no saved row the static configuration (environment variables, or a
    // programmatic `Config` in tests and the desktop app) is the baseline.
    let mut settings = stored
        .map(|stored| stored.settings.clone())
        .unwrap_or_else(|| settings_from_config(config));
    let locked = config.oidc_env_locked;

    let mut locked_fields = Vec::new();
    if locked.issuer {
        if let Some(issuer) = non_empty(config.oidc_issuer.as_deref()) {
            settings.issuer = Some(issuer);
            settings.enabled = true;
        }
        locked_fields.push("issuer".to_string());
        locked_fields.push("enabled".to_string());
    }
    if locked.client_id {
        settings.client_id = non_empty(config.oidc_client_id.as_deref());
        locked_fields.push("clientId".to_string());
    }
    if locked.client_secret {
        settings.client_secret = non_empty(config.oidc_client_secret.as_deref());
        settings.secret_configured = settings.client_secret.is_some();
        locked_fields.push("clientSecret".to_string());
    }
    if locked.redirect_url {
        settings.redirect_url = non_empty(config.oidc_redirect_url.as_deref());
        locked_fields.push("redirectUrl".to_string());
    }
    if locked.public_base_url {
        settings.public_base_url = non_empty(config.oidc_public_base_url.as_deref());
        locked_fields.push("publicBaseUrl".to_string());
    }
    if locked.scopes {
        settings.scopes = config.oidc_scopes.clone();
        locked_fields.push("scopes".to_string());
    }
    if locked.display_name {
        settings.display_name = config.oidc_display_name.clone();
        locked_fields.push("displayName".to_string());
    }
    if locked.auto_create {
        settings.auto_create = config.oidc_auto_create;
        locked_fields.push("autoCreate".to_string());
    }
    if locked.default_role {
        settings.default_role = config.oidc_default_role;
        locked_fields.push("defaultRole".to_string());
    }
    if locked.groups_claim {
        settings.groups_claim = config.oidc_groups_claim.clone();
        locked_fields.push("groupsClaim".to_string());
    }
    if locked.admin_groups {
        settings.admin_groups = config.oidc_admin_groups.clone();
        locked_fields.push("adminGroups".to_string());
    }

    if !locked.client_secret {
        if let Some(stored) = stored {
            settings.client_secret = stored.settings.client_secret.clone();
            settings.secret_configured = stored.encrypted_secret.is_some();
        }
    }

    ResolvedOidcSettings {
        settings,
        env_locked_fields: locked_fields,
        encrypted_secret: stored.and_then(|stored| stored.encrypted_secret.clone()),
        secret_error: stored.is_some_and(|stored| stored.secret_error),
        stored: stored.is_some(),
    }
}

/// Trims an optional string, treating blank as absent.
#[must_use]
pub fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Validates settings submitted from the admin UI.
///
/// `strict` enforces HTTPS for the issuer (except loopback hosts) and is only
/// applied to values typed in the UI; environment values keep their historical
/// (laxer) validation so existing deployments restart exactly as before.
pub fn validate_settings(settings: &OidcSettings, strict_https: bool) -> Result<(), ApiError> {
    let issuer = non_empty(settings.issuer.as_deref());
    match issuer {
        Some(issuer) => {
            let normalized = normalize_issuer(&issuer).map_err(|error| {
                ApiError::bad_request(
                    "invalid_oidc_issuer",
                    format!("Invalid issuer URL: {error}"),
                )
            })?;
            if strict_https {
                require_https_issuer(&normalized)?;
            }
        }
        None if settings.enabled => {
            return Err(ApiError::bad_request(
                "invalid_oidc_issuer",
                "An issuer URL is required when SSO is enabled",
            ));
        }
        None => {}
    }

    if settings.enabled && non_empty(settings.client_id.as_deref()).is_none() {
        return Err(ApiError::bad_request(
            "invalid_oidc_client_id",
            "A client ID is required when SSO is enabled",
        ));
    }
    if let Some(client_id) = non_empty(settings.client_id.as_deref()) {
        if client_id.chars().count() > 512 {
            return Err(ApiError::bad_request(
                "invalid_oidc_client_id",
                "The client ID must be at most 512 characters",
            ));
        }
    }
    if let Some(redirect) = non_empty(settings.redirect_url.as_deref()) {
        validate_redirect_template(&redirect).map_err(|error| {
            ApiError::bad_request(
                "invalid_oidc_redirect_url",
                format!("Invalid redirect URL: {error}"),
            )
        })?;
    }
    if let Some(base) = non_empty(settings.public_base_url.as_deref()) {
        validate_public_base(&base).map_err(|error| {
            ApiError::bad_request(
                "invalid_oidc_public_base_url",
                format!("Invalid public base URL: {error}"),
            )
        })?;
    }
    if settings.groups_claim.trim().is_empty() {
        return Err(ApiError::bad_request(
            "invalid_oidc_groups_claim",
            "The groups claim must not be empty",
        ));
    }
    if parse_scopes(&settings.scopes).len() > 32 {
        return Err(ApiError::bad_request(
            "invalid_oidc_scopes",
            "Too many scopes requested",
        ));
    }
    Ok(())
}

/// Rejects a plain-HTTP issuer, except on loopback hosts used in development.
fn require_https_issuer(value: &str) -> Result<(), ApiError> {
    let url = Url::parse(value).map_err(|_| {
        ApiError::bad_request("invalid_oidc_issuer", "The issuer must be an absolute URL")
    })?;
    let loopback = url
        .host_str()
        .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1"));
    if url.scheme() != "https" && !loopback {
        return Err(ApiError::bad_request(
            "insecure_oidc_issuer",
            "The issuer must use HTTPS (plain HTTP is only allowed on localhost)",
        ));
    }
    Ok(())
}

/// One in-flight authorization request.
#[derive(Clone)]
struct PendingLogin {
    state: String,
    nonce: String,
    code_verifier: String,
    redirect_uri: String,
    created_at: Instant,
}

/// One one-time code returned to the SPA.
#[derive(Clone)]
struct OneTimeCode {
    code: String,
    user_id: i64,
    expires_at: Instant,
}

/// In-memory state and code stores.
#[derive(Default)]
struct OidcStore {
    pending: Mutex<Vec<PendingLogin>>,
    codes: Mutex<Vec<OneTimeCode>>,
}

impl OidcStore {
    fn insert_pending(&self, login: PendingLogin) -> Result<(), ApiError> {
        let mut pending = lock(&self.pending);
        pending.retain(|entry| entry.created_at.elapsed() < PENDING_TTL);
        if pending.len() >= MAX_PENDING {
            return Err(ApiError::too_many_requests(
                "oidc_busy",
                "Too many pending SSO logins; try again shortly",
            ));
        }
        pending.push(login);
        Ok(())
    }

    fn take_pending(&self, state: &str) -> Option<PendingLogin> {
        let mut pending = lock(&self.pending);
        pending.retain(|entry| entry.created_at.elapsed() < PENDING_TTL);
        let index = pending
            .iter()
            .position(|entry| bool::from(entry.state.as_bytes().ct_eq(state.as_bytes())))?;
        Some(pending.remove(index))
    }

    fn insert_code(&self, code: &str, user_id: i64) -> Result<(), ApiError> {
        let mut codes = lock(&self.codes);
        let now = Instant::now();
        codes.retain(|entry| entry.expires_at > now);
        if codes.len() >= MAX_CODES {
            return Err(ApiError::too_many_requests(
                "oidc_busy",
                "Too many pending SSO exchanges; try again shortly",
            ));
        }
        codes.push(OneTimeCode {
            code: code.to_string(),
            user_id,
            expires_at: now + CODE_TTL,
        });
        Ok(())
    }

    fn take_code(&self, code: &str) -> Option<i64> {
        let mut codes = lock(&self.codes);
        let now = Instant::now();
        codes.retain(|entry| entry.expires_at > now);
        let index = codes
            .iter()
            .position(|entry| bool::from(entry.code.as_bytes().ct_eq(code.as_bytes())))?;
        Some(codes.remove(index).user_id)
    }
}

/// OIDC client and in-memory flow state.
#[derive(Clone)]
pub struct OidcClient {
    issuer: String,
    client_id: String,
    client_secret: Option<String>,
    redirect_url: Option<String>,
    public_base_url: Option<String>,
    scopes: Vec<String>,
    display_name: String,
    auto_create: bool,
    default_role: Role,
    admin_groups: Vec<String>,
    groups_claim: String,
    http: Client,
    /// Dedicated client for discovery: short timeout, no redirects.
    discovery_http: Client,
    discovery: Arc<Mutex<Option<DiscoveryDocument>>>,
    jwks: Arc<Mutex<Option<CachedJwks>>>,
    store: Arc<OidcStore>,
}

#[derive(Debug, Clone)]
struct CachedJwks {
    set: JwkSet,
    fetched_at: Instant,
}

#[derive(Debug, Clone, Deserialize)]
struct DiscoveryDocument {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    #[serde(default)]
    id_token_signing_alg_values_supported: Vec<String>,
    #[serde(default)]
    token_endpoint_auth_methods_supported: Vec<String>,
}

impl From<&DiscoveryDocument> for OidcDiscoveryInfo {
    fn from(document: &DiscoveryDocument) -> Self {
        Self {
            issuer: document.issuer.clone(),
            authorization_endpoint: document.authorization_endpoint.clone(),
            token_endpoint: document.token_endpoint.clone(),
            jwks_uri: document.jwks_uri.clone(),
            id_token_signing_alg_values_supported: document
                .id_token_signing_alg_values_supported
                .clone(),
            token_endpoint_auth_methods_supported: document
                .token_endpoint_auth_methods_supported
                .clone(),
        }
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
}

/// Identity extracted from a verified ID token.
#[derive(Debug, Clone)]
pub struct OidcIdentity {
    /// Issuer that signed the token.
    pub issuer: String,
    /// Stable subject identifier.
    pub subject: String,
    /// Verified e-mail address, when present and verified.
    pub email: Option<String>,
    /// Provider-preferred username, when present.
    pub preferred_username: Option<String>,
    /// Group/role values used for admin mapping.
    pub groups: Vec<String>,
}

impl OidcClient {
    /// Builds a client from static configuration (environment or desktop).
    /// Returns `None` when SSO is disabled.
    pub fn from_config(config: &Config, http: Client) -> AnyResult<Option<Self>> {
        let settings = settings_from_config(config);
        Self::from_settings(&settings, http).context("invalid LUDEX_OIDC_* configuration")
    }

    /// Builds a client from effective (resolved) settings. Returns `None` when
    /// SSO is disabled in the settings.
    pub fn from_settings(settings: &OidcSettings, http: Client) -> AnyResult<Option<Self>> {
        if !settings.enabled {
            return Ok(None);
        }
        let issuer = non_empty(settings.issuer.as_deref())
            .context("an OIDC issuer is required when SSO is enabled")?;
        let client_id = non_empty(settings.client_id.as_deref())
            .context("an OIDC client ID is required when SSO is enabled")?;
        let issuer = normalize_issuer(&issuer).context("invalid OIDC issuer")?;
        let public_base_url = non_empty(settings.public_base_url.as_deref())
            .map(|value| validate_public_base(&value))
            .transpose()
            .context("invalid OIDC public base URL")?;
        let redirect_url = non_empty(settings.redirect_url.as_deref())
            .map(|value| validate_redirect_template(&value))
            .transpose()
            .context("invalid OIDC redirect URL")?;
        let groups_claim = settings.groups_claim.trim().to_string();
        let admin_groups = settings
            .admin_groups
            .iter()
            .map(|group| group.trim())
            .filter(|group| !group.is_empty())
            .map(str::to_string)
            .collect();
        let discovery_http = Client::builder()
            .user_agent(concat!("Ludex/", env!("CARGO_PKG_VERSION")))
            .timeout(DISCOVERY_TIMEOUT)
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("build OIDC discovery client")?;

        Ok(Some(Self {
            issuer,
            client_id,
            client_secret: non_empty(settings.client_secret.as_deref()),
            redirect_url,
            public_base_url,
            scopes: parse_scopes(&settings.scopes),
            display_name: settings.display_name.trim().to_string(),
            auto_create: settings.auto_create,
            default_role: settings.default_role,
            admin_groups,
            groups_claim: if groups_claim.is_empty() {
                "groups".to_string()
            } else {
                groups_claim
            },
            http,
            discovery_http,
            discovery: Arc::new(Mutex::new(None)),
            jwks: Arc::new(Mutex::new(None)),
            store: Arc::new(OidcStore::default()),
        }))
    }

    /// Issuer currently configured.
    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Runs discovery with a fresh client, returning actionable provider
    /// errors for the admin save/test endpoints.
    pub async fn probe(&self) -> Result<OidcDiscoveryInfo, String> {
        let document = self.fetch_discovery().await?;
        Ok((&document).into())
    }

    /// Display name for the web UI button.
    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Builds an authorization URL and stores the matching state/nonce/PKCE
    /// verifier for ten minutes.
    pub async fn begin(&self, headers: &HeaderMap) -> Result<String, ApiError> {
        let discovery = self.discovery().await?;
        let state = random_token();
        let nonce = random_token();
        let code_verifier = random_token();
        let code_challenge = pkce_challenge(&code_verifier);
        let redirect_uri = self.redirect_uri(headers)?;

        let mut url = Url::parse(&discovery.authorization_endpoint).map_err(|_| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "oidc_discovery_failed",
                "The identity provider returned an invalid authorization endpoint",
            )
        })?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("scope", &self.scopes.join(" "))
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge", &code_challenge)
            .append_pair("code_challenge_method", "S256");

        self.store.insert_pending(PendingLogin {
            state,
            nonce,
            code_verifier,
            redirect_uri,
            created_at: Instant::now(),
        })?;
        Ok(url.to_string())
    }

    /// Validates state, exchanges the code and verifies the ID token.
    pub async fn complete(&self, code: &str, state: &str) -> Result<OidcIdentity, ApiError> {
        let pending = self.store.take_pending(state).ok_or_else(|| {
            ApiError::bad_request("invalid_state", "The SSO state is invalid or expired")
        })?;
        let discovery = self.discovery().await?;
        let token = self.exchange_code(&discovery, code, &pending).await?;
        self.verify_id_token(&discovery, &token.id_token, &pending.nonce)
            .await
    }

    /// Issues a one-time code for the SPA.
    pub fn issue_code(&self, user_id: i64) -> Result<String, ApiError> {
        let code = random_token();
        self.store.insert_code(&code, user_id)?;
        Ok(code)
    }

    /// Consumes a one-time code. Returns `None` when it is unknown, expired or
    /// already used.
    #[must_use]
    pub fn take_code(&self, code: &str) -> Option<i64> {
        self.store.take_code(code)
    }

    /// URL the browser is redirected to after a successful callback.
    #[must_use]
    pub fn callback_location(&self, code: &str) -> String {
        let path = format!("/#/login?oidc_code={code}");
        match &self.public_base_url {
            Some(base) => format!("{base}{path}"),
            None => path,
        }
    }

    fn redirect_uri(&self, headers: &HeaderMap) -> Result<String, ApiError> {
        let path = self
            .redirect_url
            .as_deref()
            .unwrap_or("/api/auth/oidc/callback");
        if path.starts_with('/') {
            let base = self.public_base(headers)?;
            Ok(format!("{base}{path}"))
        } else {
            Ok(path.to_string())
        }
    }

    fn public_base(&self, headers: &HeaderMap) -> Result<String, ApiError> {
        if let Some(base) = &self.public_base_url {
            return Ok(base.clone());
        }
        let scheme = header_value(headers, "x-forwarded-proto")
            .unwrap_or_else(|| "http".to_string())
            .to_ascii_lowercase();
        if scheme != "http" && scheme != "https" {
            return Err(redirect_error());
        }
        let host = header_value(headers, "x-forwarded-host")
            .or_else(|| header_value(headers, "host"))
            .ok_or_else(redirect_error)?;
        validate_origin(&format!("{scheme}://{host}")).ok_or_else(redirect_error)
    }

    async fn discovery(&self) -> Result<DiscoveryDocument, ApiError> {
        if let Some(document) = lock(&self.discovery).clone() {
            return Ok(document);
        }
        let document = self.fetch_discovery().await.map_err(|error| {
            tracing::warn!(issuer = %self.issuer, error = %error, "OIDC discovery failed");
            discovery_error()
        })?;
        *lock(&self.discovery) = Some(document.clone());
        Ok(document)
    }

    /// Fetches and validates the discovery document, returning the raw reason
    /// on failure. Used by the admin endpoints so an operator sees what broke.
    async fn fetch_discovery(&self) -> Result<DiscoveryDocument, String> {
        let url = format!("{}/.well-known/openid-configuration", self.issuer);
        let response = self
            .discovery_http
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|error| format!("Could not reach {url}: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("{url} returned HTTP {status}"));
        }
        let document: DiscoveryDocument = response
            .json()
            .await
            .map_err(|error| format!("{url} did not return a valid JSON document: {error}"))?;
        validate_discovery(&self.issuer, &document).map_err(|_| {
            format!(
                "{url} is invalid: the provider issuer or one of its endpoints does not match {}",
                self.issuer
            )
        })?;
        Ok(document)
    }

    async fn jwks(&self, force: bool) -> Result<JwkSet, ApiError> {
        if !force {
            if let Some(cached) = lock(&self.jwks).as_ref() {
                if cached.fetched_at.elapsed() < JWKS_TTL {
                    return Ok(cached.set.clone());
                }
            }
        }
        let discovery = self.discovery().await?;
        let response = self
            .http
            .get(&discovery.jwks_uri)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| jwks_error())?;
        if !response.status().is_success() {
            return Err(jwks_error());
        }
        let set: JwkSet = response.json().await.map_err(|_| jwks_error())?;
        if set.keys.is_empty() {
            return Err(jwks_error());
        }
        *lock(&self.jwks) = Some(CachedJwks {
            set: set.clone(),
            fetched_at: Instant::now(),
        });
        Ok(set)
    }

    async fn exchange_code(
        &self,
        discovery: &DiscoveryDocument,
        code: &str,
        pending: &PendingLogin,
    ) -> Result<TokenResponse, ApiError> {
        let mut form = vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code.to_string()),
            ("redirect_uri", pending.redirect_uri.clone()),
            ("client_id", self.client_id.clone()),
            ("code_verifier", pending.code_verifier.clone()),
        ];
        let use_post = discovery
            .token_endpoint_auth_methods_supported
            .iter()
            .any(|method| method == "client_secret_post")
            && !discovery
                .token_endpoint_auth_methods_supported
                .iter()
                .any(|method| method == "client_secret_basic");
        if use_post {
            if let Some(secret) = &self.client_secret {
                form.push(("client_secret", secret.clone()));
            }
        }

        let mut request = self
            .http
            .post(&discovery.token_endpoint)
            .header(reqwest::header::ACCEPT, "application/json")
            .form(&form);
        if !use_post {
            if let Some(secret) = &self.client_secret {
                request = request.basic_auth(&self.client_id, Some(secret));
            }
        }
        let response = request.send().await.map_err(|_| provider_unavailable())?;
        let status = response.status();
        if !status.is_success() {
            return if status.is_client_error() {
                Err(ApiError::new(
                    StatusCode::UNAUTHORIZED,
                    "oidc_token_exchange_failed",
                    "The identity provider rejected the authorization code",
                ))
            } else {
                Err(provider_unavailable())
            };
        }
        let token: TokenResponse = response.json().await.map_err(|_| provider_invalid())?;
        if token.id_token.trim().is_empty() {
            return Err(provider_invalid());
        }
        Ok(token)
    }

    async fn verify_id_token(
        &self,
        discovery: &DiscoveryDocument,
        id_token: &str,
        expected_nonce: &str,
    ) -> Result<OidcIdentity, ApiError> {
        let header = decode_header(id_token).map_err(|_| invalid_token())?;
        let alg_name = format!("{:?}", header.alg);
        let allowed_algorithms: Vec<String> =
            if discovery.id_token_signing_alg_values_supported.is_empty() {
                vec!["RS256".to_string()]
            } else {
                discovery.id_token_signing_alg_values_supported.clone()
            };
        if !allowed_algorithms.iter().any(|alg| alg == &alg_name)
            || matches!(
                header.alg,
                Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512
            )
        {
            return Err(invalid_token());
        }

        let mut set = self.jwks(false).await?;
        let mut candidates = candidate_keys(&set, header.kid.as_deref());
        if candidates.is_empty() && header.kid.is_some() {
            set = self.jwks(true).await?;
            candidates = candidate_keys(&set, header.kid.as_deref());
        }
        if candidates.is_empty() {
            return Err(invalid_token());
        }

        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&[self.client_id.as_str()]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);

        let mut decoded: Option<Value> = None;
        for jwk in &candidates {
            if let Some(usage) = &jwk.common.public_key_use {
                if usage != &PublicKeyUse::Signature {
                    continue;
                }
            }
            if let Some(key_algorithm) = &jwk.common.key_algorithm {
                if key_algorithm.to_string() != alg_name {
                    continue;
                }
            }
            let Ok(key) = DecodingKey::from_jwk(jwk) else {
                continue;
            };
            if let Ok(data) = decode::<Value>(id_token, &key, &validation) {
                decoded = Some(data.claims);
                break;
            }
        }
        let claims = decoded.ok_or_else(invalid_token)?;

        let multiple_audiences = claims
            .get("aud")
            .and_then(Value::as_array)
            .is_some_and(|audiences| audiences.len() > 1);
        match claims.get("azp").and_then(Value::as_str) {
            Some(azp) if azp != self.client_id => return Err(invalid_token()),
            None if multiple_audiences => return Err(invalid_token()),
            _ => {}
        }

        let nonce = claims
            .get("nonce")
            .and_then(Value::as_str)
            .ok_or_else(invalid_token)?;
        if !bool::from(nonce.as_bytes().ct_eq(expected_nonce.as_bytes())) {
            return Err(invalid_token());
        }

        let subject = claims
            .get("sub")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|subject| !subject.is_empty())
            .ok_or_else(invalid_token)?
            .to_string();
        let email_verified = claim_bool(claims.get("email_verified"));
        let email = claims
            .get("email")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|email| !email.is_empty())
            .map(str::to_string);
        let email = if email_verified { email } else { None };
        let preferred_username = claims
            .get("preferred_username")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|username| !username.is_empty())
            .map(str::to_string);
        let groups = claim_strings(claim_at(&claims, &self.groups_claim));

        Ok(OidcIdentity {
            issuer: self.issuer.clone(),
            subject,
            email,
            preferred_username,
            groups,
        })
    }
}

fn candidate_keys(set: &JwkSet, kid: Option<&str>) -> Vec<Jwk> {
    match kid {
        Some(kid) => set.find(kid).cloned().into_iter().collect(),
        None => set.keys.clone(),
    }
}

fn claim_at<'a>(claims: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = claims;
    for segment in path.split('.') {
        if segment.is_empty() {
            return None;
        }
        current = current.get(segment)?;
    }
    Some(current)
}

fn claim_strings(value: Option<&Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let mut values = Vec::new();
    let mut push = |candidate: &str| {
        let candidate = candidate.trim();
        if !candidate.is_empty()
            && !values
                .iter()
                .any(|existing: &String| existing.eq_ignore_ascii_case(candidate))
        {
            values.push(candidate.to_string());
        }
    };
    match value {
        Value::String(text) => {
            for part in text.split(|character: char| character == ',' || character.is_whitespace())
            {
                push(part);
            }
        }
        Value::Array(items) => {
            for item in items {
                if let Some(text) = item.as_str() {
                    push(text);
                }
            }
        }
        _ => {}
    }
    values
}

fn claim_bool(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(value)) => *value,
        Some(Value::String(value)) => value.eq_ignore_ascii_case("true"),
        Some(Value::Number(value)) => value.as_i64() == Some(1),
        _ => false,
    }
}

fn role_for(default_role: Role, admin_groups: &[String], groups: &[String]) -> Role {
    if admin_groups
        .iter()
        .any(|admin| groups.iter().any(|group| group.eq_ignore_ascii_case(admin)))
    {
        Role::Admin
    } else {
        default_role
    }
}

/// Links or provisions a local user for a verified OIDC identity.
pub async fn provision_user(
    state: &AppState,
    client: &OidcClient,
    identity: &OidcIdentity,
) -> Result<i64, ApiError> {
    if let Some(user) = user_by_subject(state, &identity.issuer, &identity.subject).await? {
        ensure_active(&user)?;
        if user.email.is_none() {
            if let Some(email) = &identity.email {
                sqlx::query("UPDATE users SET email = ?, updated_at = ? WHERE id = ?")
                    .bind(email)
                    .bind(db::db_time(db::now()))
                    .bind(user.id)
                    .execute(&state.pool)
                    .await
                    .map_err(ApiError::from)?;
            }
        }
        state.events.emit(
            Event::new(actions::OIDC_LOGIN, Actor::System)
                .target("user", user.id.to_string())
                .metadata(serde_json::json!({
                    "issuer": identity.issuer,
                })),
        );
        return Ok(user.id);
    }

    if let Some(email) = &identity.email {
        let matches = users_by_email(state, email).await?;
        match matches.as_slice() {
            [] => {}
            [user] => {
                ensure_active(user)?;
                if user
                    .oidc_issuer
                    .as_deref()
                    .is_some_and(|issuer| issuer != identity.issuer)
                    || user
                        .oidc_subject
                        .as_deref()
                        .is_some_and(|subject| subject != identity.subject)
                {
                    return Err(ApiError::conflict(
                        "oidc_account_conflict",
                        "This email is already linked to another SSO identity",
                    ));
                }
                sqlx::query(
                    "UPDATE users SET oidc_issuer = ?, oidc_subject = ?, updated_at = ?
                     WHERE id = ?",
                )
                .bind(&identity.issuer)
                .bind(&identity.subject)
                .bind(db::db_time(db::now()))
                .bind(user.id)
                .execute(&state.pool)
                .await
                .map_err(ApiError::from)?;
                state.events.emit(
                    Event::new(actions::OIDC_LINKED, Actor::System)
                        .target("user", user.id.to_string())
                        .metadata(serde_json::json!({
                            "issuer": identity.issuer,
                        })),
                );
                return Ok(user.id);
            }
            _ => {
                return Err(ApiError::conflict(
                    "oidc_email_ambiguous",
                    "Several accounts use this email address; an administrator must link the SSO identity",
                ));
            }
        }
    }

    if !client.auto_create {
        return Err(ApiError::forbidden(
            "oidc_user_not_provisioned",
            "No local account is linked to this SSO identity",
        ));
    }

    let role = role_for(client.default_role, &client.admin_groups, &identity.groups);
    let base = default_username(identity);
    let username = crate::unique_username(state, &base).await?;
    let password_hash = crate::auth::hash_password(&crate::auth::generate_password())?;
    let now = db::db_time(db::now());

    let result = sqlx::query(
        "INSERT INTO users
            (username, password_hash, email, role, activated, avatar_path,
             oidc_issuer, oidc_subject, created_at, updated_at)
         VALUES (?, ?, ?, ?, 1, NULL, ?, ?, ?, ?)",
    )
    .bind(&username)
    .bind(password_hash)
    .bind(&identity.email)
    .bind(role.as_str())
    .bind(&identity.issuer)
    .bind(&identity.subject)
    .bind(&now)
    .bind(&now)
    .execute(&state.pool)
    .await
    .map_err(|error| {
        if crate::routes::system::is_unique_violation(&error) {
            ApiError::conflict(
                "oidc_provision_failed",
                "An account with this identity already exists",
            )
        } else {
            ApiError::from(error)
        }
    })?;
    let id = result.last_insert_rowid();
    state.events.emit(
        Event::new(actions::USER_CREATED, Actor::System)
            .target("user", id.to_string())
            .metadata(serde_json::json!({
                "username": username,
                "role": role.as_str(),
                "source": "oidc",
                "issuer": identity.issuer,
            })),
    );
    Ok(id)
}

#[derive(Debug, sqlx::FromRow)]
struct OidcUserRow {
    id: i64,
    email: Option<String>,
    oidc_issuer: Option<String>,
    oidc_subject: Option<String>,
    activated: i64,
}

async fn user_by_subject(
    state: &AppState,
    issuer: &str,
    subject: &str,
) -> Result<Option<OidcUserRow>, ApiError> {
    sqlx::query_as::<_, OidcUserRow>(
        "SELECT id, email, oidc_issuer, oidc_subject, activated
         FROM users WHERE oidc_issuer = ? AND oidc_subject = ?",
    )
    .bind(issuer)
    .bind(subject)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)
}

async fn users_by_email(state: &AppState, email: &str) -> Result<Vec<OidcUserRow>, ApiError> {
    sqlx::query_as::<_, OidcUserRow>(
        "SELECT id, email, oidc_issuer, oidc_subject, activated
         FROM users WHERE email IS NOT NULL AND lower(email) = lower(?)",
    )
    .bind(email)
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)
}

fn ensure_active(user: &OidcUserRow) -> Result<(), ApiError> {
    if user.activated == 0 {
        return Err(ApiError::forbidden(
            "account_disabled",
            "This account is disabled",
        ));
    }
    Ok(())
}

fn default_username(identity: &OidcIdentity) -> String {
    let candidate = identity
        .preferred_username
        .clone()
        .or_else(|| {
            identity
                .email
                .as_deref()
                .and_then(|email| email.split('@').next())
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            format!(
                "oidc-{}",
                identity.subject.chars().take(16).collect::<String>()
            )
        });
    sanitize_username(&candidate)
}

fn sanitize_username(value: &str) -> String {
    let mut username: String = value
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ' ') {
                character
            } else {
                '_'
            }
        })
        .collect();
    username = username.trim().trim_matches('.').to_string();
    username.truncate(64);
    if username.is_empty() {
        "oidc-user".to_string()
    } else {
        username
    }
}

fn has_unsafe_url_chars(value: &str) -> bool {
    value
        .chars()
        .any(|character| character.is_control() || character == '\\')
}

fn normalize_issuer(value: &str) -> AnyResult<String> {
    let trimmed = value.trim().trim_end_matches('/');
    if has_unsafe_url_chars(trimmed) {
        anyhow::bail!("issuer contains invalid characters");
    }
    let url = Url::parse(trimmed).context("issuer must be an absolute URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        anyhow::bail!("issuer must be an http(s) URL without query or fragment");
    }
    Ok(trimmed.to_string())
}

fn validate_public_base(value: &str) -> AnyResult<String> {
    let trimmed = value.trim().trim_end_matches('/');
    if has_unsafe_url_chars(trimmed) {
        anyhow::bail!("public base URL contains invalid characters");
    }
    let url = Url::parse(trimmed).context("public base URL must be absolute")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        anyhow::bail!("public base URL must be an http(s) origin without query or fragment");
    }
    Ok(trimmed.to_string())
}

fn validate_redirect_template(value: &str) -> AnyResult<String> {
    let trimmed = value.trim();
    if has_unsafe_url_chars(trimmed) {
        anyhow::bail!("redirect URL contains invalid characters");
    }
    if trimmed.starts_with('/') {
        if trimmed.starts_with("//") || trimmed.contains('#') {
            anyhow::bail!(
                "relative redirect URL must start with a single '/' and have no fragment"
            );
        }
        return Ok(trimmed.to_string());
    }
    let url = Url::parse(trimmed).context("redirect URL must be absolute or start with '/'")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        anyhow::bail!("redirect URL must be an http(s) URL without fragment");
    }
    Ok(trimmed.to_string())
}

fn validate_discovery(
    configured_issuer: &str,
    document: &DiscoveryDocument,
) -> Result<(), ApiError> {
    let issuer_matches = normalize_issuer(&document.issuer)
        .map(|issuer| issuer == configured_issuer)
        .unwrap_or(false);
    if !issuer_matches {
        return Err(discovery_error());
    }
    for endpoint in [
        &document.authorization_endpoint,
        &document.token_endpoint,
        &document.jwks_uri,
    ] {
        if !valid_endpoint(endpoint) {
            return Err(discovery_error());
        }
    }
    Ok(())
}

fn valid_endpoint(value: &str) -> bool {
    if has_unsafe_url_chars(value) {
        return false;
    }
    Url::parse(value).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
    })
}

fn validate_origin(value: &str) -> Option<String> {
    if has_unsafe_url_chars(value) {
        return None;
    }
    let url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(',').next().unwrap_or("").trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_scopes(value: &str) -> Vec<String> {
    let mut scopes = Vec::new();
    for scope in value.split_whitespace() {
        if !scopes.iter().any(|existing| existing == scope) {
            scopes.push(scope.to_string());
        }
    }
    if !scopes.iter().any(|scope| scope == "openid") {
        scopes.insert(0, "openid".to_string());
    }
    scopes
}

fn random_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn invalid_token() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "oidc_invalid_token",
        "The identity provider returned an invalid ID token",
    )
}

fn discovery_error() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "oidc_discovery_failed",
        "The identity provider discovery document is unavailable or invalid",
    )
}

fn jwks_error() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "oidc_jwks_failed",
        "The identity provider public keys are unavailable",
    )
}

fn provider_unavailable() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "oidc_provider_unavailable",
        "The identity provider is unavailable",
    )
}

fn provider_invalid() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "oidc_provider_error",
        "The identity provider returned an invalid response",
    )
}

fn redirect_error() -> ApiError {
    ApiError::bad_request(
        "oidc_redirect_failed",
        "Cannot determine the public server URL; set LUDEX_OIDC_PUBLIC_BASE_URL",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(state: &str) -> PendingLogin {
        PendingLogin {
            state: state.to_string(),
            nonce: "nonce".to_string(),
            code_verifier: "verifier".to_string(),
            redirect_uri: "http://localhost/callback".to_string(),
            created_at: Instant::now(),
        }
    }

    #[test]
    fn pkce_challenge_matches_rfc_vector() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn pending_states_are_single_use_and_compare_safely() {
        let store = OidcStore::default();
        store.insert_pending(pending("state-one")).expect("insert");
        assert!(store.take_pending("state-wrong").is_none());
        assert!(store.take_pending("state-one").is_some());
        assert!(store.take_pending("state-one").is_none());
    }

    #[test]
    fn expired_pending_states_are_rejected() {
        let store = OidcStore::default();
        let mut login = pending("expired");
        login.created_at = Instant::now() - PENDING_TTL - Duration::from_secs(1);
        store.insert_pending(login).expect("insert");
        assert!(store.take_pending("expired").is_none());
    }

    #[test]
    fn one_time_codes_are_single_use_and_expire() {
        let store = OidcStore::default();
        store.insert_code("code-one", 7).expect("insert");
        assert_eq!(store.take_code("code-one"), Some(7));
        assert_eq!(store.take_code("code-one"), None);

        let mut code = OneTimeCode {
            code: "expired".to_string(),
            user_id: 9,
            expires_at: Instant::now() + CODE_TTL,
        };
        code.expires_at = Instant::now() - Duration::from_secs(1);
        lock(&store.codes).push(code);
        assert_eq!(store.take_code("expired"), None);
    }

    #[test]
    fn group_claim_paths_and_role_mapping() {
        let claims = serde_json::json!({
            "groups": ["users", "ludex-admins"],
            "realm_access": {"roles": ["offline_access", "ludex-editors"]}
        });
        assert_eq!(
            claim_strings(claim_at(&claims, "groups")),
            vec!["users".to_string(), "ludex-admins".to_string()]
        );
        assert_eq!(
            claim_strings(claim_at(&claims, "realm_access.roles")),
            vec!["offline_access".to_string(), "ludex-editors".to_string()]
        );
        assert!(claim_at(&claims, "missing.path").is_none());

        let admins = vec!["ludex-admins".to_string()];
        assert_eq!(
            role_for(Role::User, &admins, &["LUDEX-ADMINS".to_string()]),
            Role::Admin
        );
        assert_eq!(
            role_for(Role::Editor, &admins, &["users".to_string()]),
            Role::Editor
        );
    }

    #[test]
    fn usernames_are_sanitised() {
        assert_eq!(sanitize_username("  Alice <admin>  "), "Alice _admin_");
        assert_eq!(sanitize_username("..."), "oidc-user");
        assert!(sanitize_username(&"a".repeat(100)).len() <= 64);
    }

    #[test]
    fn config_validation_rejects_unsafe_redirects() {
        assert!(validate_redirect_template("//evil.example/cb").is_err());
        assert!(validate_redirect_template("javascript:alert(1)").is_err());
        assert!(validate_redirect_template("/api/auth/oidc/callback").is_ok());
        assert!(validate_redirect_template("https://ludex.example/cb").is_ok());
        assert!(validate_public_base("https://ludex.example/").is_ok());
        assert!(validate_public_base("https://ludex.example/?next=evil").is_err());
    }

    #[test]
    fn secrets_encrypt_round_trip_and_never_serialise() {
        let server_secret = b"server-secret-used-for-settings-encryption";
        let secret = "client-secret-value-123";

        let first = encrypt_secret(server_secret, secret).expect("encrypt");
        let second = encrypt_secret(server_secret, secret).expect("encrypt again");
        assert!(first.starts_with(SECRET_PREFIX));
        assert!(!first.contains(secret));
        assert_ne!(first, second, "every write uses a fresh nonce");
        assert_eq!(
            decrypt_secret(server_secret, &first).expect("decrypt"),
            secret
        );
        assert!(decrypt_secret(b"another-server-secret", &first).is_err());
        assert!(decrypt_secret(server_secret, "garbage").is_err());

        let settings = OidcSettings {
            client_secret: Some(secret.to_string()),
            secret_configured: true,
            ..OidcSettings::default()
        };
        let json = serde_json::to_string(&settings).expect("serialise");
        assert!(!json.contains(secret));
        assert!(!json.contains("clientSecret"));
        assert!(json.contains("\"secretConfigured\":true"));
    }

    #[tokio::test]
    async fn stored_secret_is_encrypted_on_disk() {
        let temp = tempfile::tempdir().expect("tempdir");
        let database_url = format!(
            "sqlite://{}?mode=rwc",
            temp.path().join("ludex.db").display()
        );
        let pool = crate::db::connect(&database_url).await.expect("connect");
        let server_secret = b"server-secret-used-for-settings-encryption";
        let secret = "plaintext-client-secret";
        let encrypted = encrypt_secret(server_secret, secret).expect("encrypt");
        let settings = OidcSettings {
            enabled: true,
            issuer: Some("https://idp.example.com".to_string()),
            client_id: Some("ludex".to_string()),
            client_secret: Some(secret.to_string()),
            secret_configured: true,
            ..OidcSettings::default()
        };
        save_settings(&pool, &settings, Some(&encrypted))
            .await
            .expect("save");
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&pool)
            .await
            .expect("checkpoint");
        pool.close().await;

        let bytes = std::fs::read(temp.path().join("ludex.db")).expect("read db");
        let haystack = String::from_utf8_lossy(&bytes);
        assert!(
            !haystack.contains(secret),
            "the OIDC client secret must never be stored in plaintext"
        );

        let pool = crate::db::connect(&database_url).await.expect("reconnect");
        let stored = load_settings(&pool, server_secret)
            .await
            .expect("load")
            .expect("row");
        assert_eq!(stored.settings.client_secret.as_deref(), Some(secret));
        assert!(stored.encrypted_secret.is_some());
        assert!(stored.settings.secret_configured);
        pool.close().await;
    }

    #[test]
    fn environment_wins_per_field_over_saved_settings() {
        let mut config = Config::for_local(
            std::path::PathBuf::from("/tmp/games"),
            std::path::PathBuf::from("/tmp/data"),
            std::path::PathBuf::from("/tmp/media"),
        );
        config.oidc_issuer = Some("https://env.example.com".to_string());
        config.oidc_client_id = Some("env-client".to_string());
        config.oidc_display_name = "Env provider".to_string();
        config.oidc_env_locked.issuer = true;
        config.oidc_env_locked.client_id = true;
        config.oidc_env_locked.display_name = true;

        let stored_settings = OidcSettings {
            enabled: false,
            issuer: Some("https://db.example.com".to_string()),
            client_id: Some("db-client".to_string()),
            display_name: "DB provider".to_string(),
            scopes: "openid profile".to_string(),
            client_secret: Some("db-secret".to_string()),
            secret_configured: true,
            ..OidcSettings::default()
        };
        let stored = StoredOidcSettings {
            settings: stored_settings.clone(),
            encrypted_secret: Some("v1.ciphertext".to_string()),
            secret_error: false,
        };

        let resolved = resolve_settings(Some(&stored), &config);
        assert_eq!(
            resolved.settings.issuer.as_deref(),
            Some("https://env.example.com")
        );
        assert!(resolved.settings.enabled, "an env issuer enables SSO");
        assert_eq!(resolved.settings.client_id.as_deref(), Some("env-client"));
        assert_eq!(resolved.settings.display_name, "Env provider");
        assert_eq!(
            resolved.settings.scopes, stored_settings.scopes,
            "fields that are not pinned keep the saved value"
        );
        assert!(resolved
            .env_locked_fields
            .iter()
            .any(|field| field == "issuer"));
        assert!(resolved
            .env_locked_fields
            .iter()
            .any(|field| field == "enabled"));
        assert!(resolved
            .env_locked_fields
            .iter()
            .any(|field| field == "clientId"));
        assert!(!resolved
            .env_locked_fields
            .iter()
            .any(|field| field == "scopes"));

        // With nothing saved and nothing pinned, SSO stays off.
        let bare = Config::for_local(
            std::path::PathBuf::from("/tmp/games"),
            std::path::PathBuf::from("/tmp/data"),
            std::path::PathBuf::from("/tmp/media"),
        );
        let resolved = resolve_settings(None, &bare);
        assert!(!resolved.settings.enabled);
        assert!(resolved.env_locked_fields.is_empty());
    }

    #[test]
    fn settings_validation_requires_https_except_on_loopback() {
        let base = OidcSettings {
            enabled: true,
            issuer: Some("http://idp.example.com".to_string()),
            client_id: Some("ludex".to_string()),
            ..OidcSettings::default()
        };
        assert!(validate_settings(&base, true).is_err());
        assert!(validate_settings(&base, false).is_ok());

        let loopback = OidcSettings {
            issuer: Some("http://localhost:9000".to_string()),
            ..base.clone()
        };
        assert!(validate_settings(&loopback, true).is_ok());
        let loopback_ip = OidcSettings {
            issuer: Some("http://127.0.0.1:9000".to_string()),
            ..base.clone()
        };
        assert!(validate_settings(&loopback_ip, true).is_ok());

        let no_client = OidcSettings {
            client_id: None,
            ..base.clone()
        };
        assert!(validate_settings(&no_client, true).is_err());

        let bad_redirect = OidcSettings {
            issuer: Some("https://idp.example.com".to_string()),
            redirect_url: Some("javascript:alert(1)".to_string()),
            ..base.clone()
        };
        assert!(validate_settings(&bad_redirect, true).is_err());

        let empty_claim = OidcSettings {
            issuer: Some("https://idp.example.com".to_string()),
            groups_claim: "  ".to_string(),
            ..base.clone()
        };
        assert!(validate_settings(&empty_claim, true).is_err());

        let disabled_blank = OidcSettings {
            enabled: false,
            issuer: None,
            client_id: None,
            ..OidcSettings::default()
        };
        assert!(validate_settings(&disabled_blank, true).is_ok());
    }

    #[test]
    fn changed_fields_only_lists_names() {
        let previous = OidcSettings {
            enabled: true,
            issuer: Some("https://idp.example.com".to_string()),
            client_id: Some("ludex".to_string()),
            display_name: "SSO".to_string(),
            admin_groups: vec!["admins".to_string()],
            ..OidcSettings::default()
        };
        let next = OidcSettings {
            enabled: false,
            issuer: Some("https://other.example.com".to_string()),
            client_id: Some("ludex".to_string()),
            display_name: "Authentik".to_string(),
            admin_groups: vec!["admins".to_string(), "operators".to_string()],
            ..OidcSettings::default()
        };
        let fields = changed_fields(&previous, &next, true);
        assert_eq!(
            fields,
            vec![
                "enabled".to_string(),
                "issuer".to_string(),
                "displayName".to_string(),
                "adminGroups".to_string(),
                "clientSecret".to_string(),
            ]
        );
        assert!(!fields.iter().any(|field| field == "clientId"));
    }
}
