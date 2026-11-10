//! OpenID Connect authorization-code flow with PKCE.
//!
//! The implementation is deliberately small: discovery, authorization URL
//! construction, token exchange and ID-token verification against the
//! provider's JWKS. It never stores provider tokens and never logs them.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result as AnyResult};
use axum::http::{HeaderMap, StatusCode};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use jsonwebtoken::jwk::{Jwk, JwkSet, PublicKeyUse};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use ludex_core::models::Role;
use rand::RngCore;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use url::Url;

use crate::config::Config;
use crate::db;
use crate::error::ApiError;
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
    /// Builds a client from configuration. Returns `None` when SSO is disabled.
    pub fn from_config(config: &Config, http: Client) -> AnyResult<Option<Self>> {
        let Some(issuer) = config
            .oidc_issuer
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };
        let client_id = config
            .oidc_client_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("LUDEX_OIDC_CLIENT_ID is required when OIDC is enabled")?;
        let issuer = normalize_issuer(issuer).context("invalid LUDEX_OIDC_ISSUER")?;
        let public_base_url = config
            .oidc_public_base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(validate_public_base)
            .transpose()
            .context("invalid LUDEX_OIDC_PUBLIC_BASE_URL")?;
        let redirect_url = config
            .oidc_redirect_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(validate_redirect_template)
            .transpose()
            .context("invalid LUDEX_OIDC_REDIRECT_URL")?;
        let scopes = parse_scopes(&config.oidc_scopes);
        let groups_claim = {
            let value = config.oidc_groups_claim.trim();
            if value.is_empty() {
                "groups".to_string()
            } else {
                value.to_string()
            }
        };
        let admin_groups = config
            .oidc_admin_groups
            .iter()
            .map(|group| group.trim())
            .filter(|group| !group.is_empty())
            .map(str::to_string)
            .collect();

        Ok(Some(Self {
            issuer,
            client_id: client_id.to_string(),
            client_secret: config
                .oidc_client_secret
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            redirect_url,
            public_base_url,
            scopes,
            display_name: config.oidc_display_name.clone(),
            auto_create: config.oidc_auto_create,
            default_role: config.oidc_default_role,
            admin_groups,
            groups_claim,
            http,
            discovery: Arc::new(Mutex::new(None)),
            jwks: Arc::new(Mutex::new(None)),
            store: Arc::new(OidcStore::default()),
        }))
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
        let url = format!("{}/.well-known/openid-configuration", self.issuer);
        let response = self
            .http
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| discovery_error())?;
        if !response.status().is_success() {
            return Err(discovery_error());
        }
        let document: DiscoveryDocument = response.json().await.map_err(|_| discovery_error())?;
        validate_discovery(&self.issuer, &document)?;
        *lock(&self.discovery) = Some(document.clone());
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
    Ok(result.last_insert_rowid())
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
}
