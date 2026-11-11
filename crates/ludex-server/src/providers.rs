//! Community metadata providers (#28).
//!
//! A provider is an HTTP sidecar, never a plugin that runs inside the server:
//! the administrator configures a base URL, an optional bearer token, a
//! priority, an enabled flag and an optional field mapping. IGDB stays the
//! built-in default and is tried first; enabled providers run afterwards in
//! ascending priority order, then Steam, then the file-name fallback.
//!
//! Contract (documented in `docs/INTEGRATIONS.md`):
//!
//! ```text
//! GET {base_url}/search?title=<urlencoded>&year=<optional>
//! Accept: application/json
//! Authorization: Bearer <token>            (only when configured)
//!
//! {"results": [{
//!   "id": "...", "title": "...", "description": "...",
//!   "releaseDate": "2019-11-12", "coverUrl": "...", "backgroundUrl": "...",
//!   "screenshots": ["..."], "genres": ["..."], "developers": ["..."],
//!   "publishers": ["..."], "tags": ["..."], "rating": 85, "ageRating": 12,
//!   "providerUrl": "..."
//! }]}
//! ```
//!
//! Optional `GET {base_url}/games/{id}` returns one object (or a `results`
//! envelope with one entry) for later refreshes.
//!
//! Security: server-side fetch only, 10 s timeout, redirects disabled (a
//! redirect could downgrade HTTPS), private/loopback targets refused unless
//! `LUDEX_PROVIDER_ALLOW_PRIVATE=true`, response bodies capped at 2 MiB and
//! parsed defensively.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use ludex_core::models::ProviderRef;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::db;
use crate::db::Database;
use crate::error::ApiError;
use crate::metadata::{score_candidate, strip_html, MatchContext, ProviderMetadata};

/// Per-request timeout for a provider call.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum accepted response body size (2 MiB).
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
/// How long provider fetch history is kept, in days.
pub const FETCH_RETENTION_DAYS: i64 = 30;
/// Maximum number of candidates accepted from one search.
const MAX_CANDIDATES: usize = 50;
/// Maximum number of entries kept in a list field.
const MAX_LIST_ITEMS: usize = 32;
/// Maximum length of a string field taken from a provider.
const MAX_FIELD_CHARS: usize = 4_000;

/// HKDF salt shared with the OIDC settings encryption (the key material is
/// the same server secret; the info string separates the purposes).
const TOKEN_HKDF_SALT: &[u8] = b"ludex-oidc-settings-v1";
/// HKDF info for the provider-token encryption key.
const TOKEN_HKDF_INFO: &[u8] = b"metadata-provider-token";
/// Additional authenticated data bound to every encrypted provider token.
const TOKEN_AAD: &[u8] = b"ludex.metadata-provider.token.v1";

/// Canonical fields a mapping may target.
pub const CANONICAL_FIELDS: &[&str] = &[
    "id",
    "title",
    "description",
    "releaseDate",
    "coverUrl",
    "backgroundUrl",
    "screenshots",
    "genres",
    "developers",
    "publishers",
    "tags",
    "rating",
    "ageRating",
    "providerUrl",
];

/// Canonical field -> dot path in the provider response.
pub type Mapping = BTreeMap<String, String>;

/// One metadata candidate returned by a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCandidate {
    /// Provider-side identifier, used for `GET /games/{id}` refreshes.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Long description (HTML stripped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Release date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<DateTime<Utc>>,
    /// Cover image URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
    /// Background/hero image URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_url: Option<String>,
    /// Screenshot URLs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub screenshots: Vec<String>,
    /// Genre labels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    /// Developer names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub developers: Vec<String>,
    /// Publisher names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub publishers: Vec<String>,
    /// Free-form tags (shown by the admin test dialog only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Rating, 0-100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    /// Minimum age, when the provider publishes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_rating: Option<u8>,
    /// Public provider page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_url: Option<String>,
}

impl ProviderCandidate {
    /// Converts a candidate into the provider-independent metadata shape.
    #[must_use]
    pub fn to_metadata(&self, slug: &str) -> ProviderMetadata {
        ProviderMetadata {
            description: self
                .description
                .as_deref()
                .map(strip_html)
                .filter(|value| !value.is_empty()),
            release_date: self.release_date,
            cover_url: self.cover_url.clone(),
            background_url: self.background_url.clone(),
            screenshots: self.screenshots.clone(),
            trailers: Vec::new(),
            genres: self.genres.clone(),
            developers: self.developers.clone(),
            publishers: self.publishers.clone(),
            rating: self.rating.map(|value| value.clamp(0.0, 100.0)),
            age_rating: self.age_rating,
            early_access: None,
            alt_titles: Vec::new(),
            max_players: None,
            provider: ProviderRef {
                slug: slug.to_string(),
                id: self.id.clone(),
                url: self.provider_url.clone(),
            },
        }
    }
}

/// Why a provider call failed. The message is safe to show to an admin.
#[derive(Debug)]
pub enum ProviderError {
    /// The stored configuration is invalid (bad URL, bad mapping).
    InvalidConfig(String),
    /// The URL failed validation.
    InvalidUrl(String),
    /// The target resolves to a private/loopback address or does not resolve.
    BlockedTarget(String),
    /// The request could not be sent.
    Request(String),
    /// The request exceeded the timeout.
    Timeout,
    /// The provider answered with a non-2xx status.
    Status(u16, String),
    /// The body exceeded [`MAX_RESPONSE_BYTES`].
    TooLarge,
    /// The body was not the documented JSON envelope.
    Decode(String),
}

impl ProviderError {
    /// HTTP status returned by the provider, when a response was received.
    #[must_use]
    pub fn status_code(&self) -> Option<i64> {
        match self {
            Self::Status(code, _) => Some(i64::from(*code)),
            Self::TooLarge | Self::Decode(_) => Some(200),
            _ => None,
        }
    }
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig(message) => {
                write!(formatter, "invalid provider configuration: {message}")
            }
            Self::InvalidUrl(message) => write!(formatter, "invalid provider URL: {message}"),
            Self::BlockedTarget(message) => write!(formatter, "{message}"),
            Self::Request(message) => write!(formatter, "provider request failed: {message}"),
            Self::Timeout => formatter.write_str("provider request timed out"),
            Self::Status(code, message) => {
                write!(formatter, "provider returned HTTP {code}: {message}")
            }
            Self::TooLarge => write!(
                formatter,
                "provider response exceeds {} bytes",
                MAX_RESPONSE_BYTES
            ),
            Self::Decode(message) => write!(formatter, "invalid provider response: {message}"),
        }
    }
}

impl std::error::Error for ProviderError {}

impl From<ProviderError> for ApiError {
    fn from(error: ProviderError) -> Self {
        match error {
            ProviderError::InvalidConfig(message) | ProviderError::InvalidUrl(message) => {
                Self::bad_request("invalid_provider", message)
            }
            ProviderError::BlockedTarget(message) => {
                Self::bad_request("provider_target_not_allowed", message)
            }
            other => Self::new(
                axum::http::StatusCode::BAD_GATEWAY,
                "provider_request_failed",
                other.to_string(),
            ),
        }
    }
}

/// Resolved runtime configuration for one provider.
#[derive(Clone)]
pub struct MetadataProviderConfig {
    /// Database id.
    pub id: i64,
    /// Administrator-visible name.
    pub name: String,
    /// Slug stored in `games.provider_slug`.
    pub slug: String,
    /// Base URL without a trailing slash.
    pub base_url: String,
    /// Decrypted bearer token, never serialised.
    pub token: Option<String>,
    /// Lower runs first.
    pub priority: i64,
    /// Canonical field -> response dot path.
    pub mapping: Mapping,
    /// Request timeout (shortened by tests).
    pub timeout: Duration,
    /// Allow private/loopback targets (tests only).
    pub allow_private: bool,
}

impl std::fmt::Debug for MetadataProviderConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetadataProviderConfig")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("slug", &self.slug)
            .field("base_url", &self.base_url)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("priority", &self.priority)
            .field("mapping", &self.mapping)
            .finish()
    }
}

impl MetadataProviderConfig {
    /// Builds a config from a stored row, decrypting the token.
    pub fn from_record(
        record: &ProviderRecord,
        server_secret: &[u8],
        allow_private: bool,
    ) -> Result<Self, ProviderError> {
        let token = match record.token_enc.as_deref() {
            Some(encrypted) => Some(
                crate::secrets::decrypt(
                    server_secret,
                    TOKEN_HKDF_SALT,
                    TOKEN_HKDF_INFO,
                    TOKEN_AAD,
                    encrypted,
                )
                .map_err(|error| ProviderError::InvalidConfig(error.to_string()))?,
            ),
            None => None,
        };
        let base_url = validate_base_url(&record.base_url, allow_private)
            .map(|url| url.trim_end_matches('/').to_string())
            .map_err(ProviderError::InvalidConfig)?;
        let mapping = mapping_from_json(&record.mapping_json)
            .map_err(|error| ProviderError::InvalidConfig(error.message))?;
        Ok(Self {
            id: record.id,
            name: record.name.clone(),
            slug: slugify(&record.name),
            base_url,
            token,
            priority: record.priority,
            mapping,
            timeout: REQUEST_TIMEOUT,
            allow_private,
        })
    }
}

/// One `metadata_providers` row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ProviderRecord {
    /// Database id.
    pub id: i64,
    /// Unique name.
    pub name: String,
    /// Base URL.
    pub base_url: String,
    /// Encrypted bearer token.
    pub token_enc: Option<String>,
    /// Lower runs first.
    pub priority: i64,
    /// 0/1 enabled flag.
    pub enabled: i64,
    /// JSON mapping object.
    pub mapping_json: String,
    /// Creation timestamp.
    pub created_at: String,
    /// Update timestamp.
    pub updated_at: String,
}

/// A provider as returned by the admin API. The token is never included.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetadataProviderView {
    /// Database id.
    pub id: i64,
    /// Unique name.
    pub name: String,
    /// Base URL.
    pub base_url: String,
    /// Whether a token is stored at rest.
    pub secret_configured: bool,
    /// Lower runs first.
    pub priority: i64,
    /// Whether the provider participates in enrichment.
    pub enabled: bool,
    /// Canonical field -> dot path.
    #[schema(value_type = Object)]
    pub mapping: Mapping,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Update timestamp.
    pub updated_at: DateTime<Utc>,
}

impl ProviderRecord {
    /// Public representation. The ciphertext and plaintext stay server-side.
    #[must_use]
    pub fn to_view(&self) -> MetadataProviderView {
        MetadataProviderView {
            id: self.id,
            name: self.name.clone(),
            base_url: self.base_url.clone(),
            secret_configured: self.token_enc.is_some(),
            priority: self.priority,
            enabled: self.enabled != 0,
            mapping: mapping_from_json(&self.mapping_json).unwrap_or_default(),
            created_at: db::parse_time(&self.created_at).unwrap_or_else(Utc::now),
            updated_at: db::parse_time(&self.updated_at).unwrap_or_else(Utc::now),
        }
    }
}

/// Result of the admin test dialog.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTestResponse {
    /// Whether the call succeeded.
    pub ok: bool,
    /// Candidates returned by the provider (possibly empty).
    pub candidates: Vec<ProviderCandidate>,
    /// Human-readable error, when `ok` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// HTTP status, when a response was received.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<i64>,
    /// Round-trip duration in milliseconds.
    pub elapsed_ms: u64,
}

/// Loads every provider row, enabled or not, ordered by priority.
pub async fn load_records(pool: &Database) -> Result<Vec<ProviderRecord>, sqlx::Error> {
    db::query_as::<ProviderRecord>(
        "SELECT id, name, base_url, token_enc, priority, enabled, mapping_json, created_at, updated_at
         FROM metadata_providers
         ORDER BY priority, id",
    )
    .fetch_all(pool)
    .await
}

/// Loads the enabled providers as runtime configs. A provider whose token
/// cannot be decrypted (server secret rotated) is skipped with a warning so it
/// can never break enrichment.
pub async fn load_enabled(
    pool: &Database,
    server_secret: &[u8],
    allow_private: bool,
) -> Vec<MetadataProviderConfig> {
    let records = match load_records(pool).await {
        Ok(records) => records,
        Err(error) => {
            tracing::error!(error = %error, "could not load metadata providers");
            return Vec::new();
        }
    };
    records
        .iter()
        .filter(|record| record.enabled != 0)
        .filter_map(|record| {
            match MetadataProviderConfig::from_record(record, server_secret, allow_private) {
                Ok(config) => Some(config),
                Err(error) => {
                    tracing::warn!(
                        provider = %record.name,
                        error = %error,
                        "metadata provider is disabled until its configuration is fixed"
                    );
                    None
                }
            }
        })
        .collect()
}

/// Searches one provider.
pub async fn search(
    config: &MetadataProviderConfig,
    title: &str,
    year: Option<u16>,
) -> Result<Vec<ProviderCandidate>, ProviderError> {
    let query = sanitise_query(title);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let mut url = endpoint(config, &["search"])?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("title", &query);
        if let Some(year) = year {
            pairs.append_pair("year", &year.to_string());
        }
    }
    let value = get_json(config, url).await?;
    parse_candidates(&value, &config.mapping)
}

/// Fetches one provider game by id (for later refreshes).
pub async fn fetch_by_id(
    config: &MetadataProviderConfig,
    id: &str,
) -> Result<Option<ProviderCandidate>, ProviderError> {
    if id.trim().is_empty() {
        return Ok(None);
    }
    let url = endpoint(config, &["games", id])?;
    let value = get_json(config, url).await?;
    let mapping = &config.mapping;
    if let Some(item) = value
        .get("results")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
    {
        return Ok(parse_candidate(item, mapping));
    }
    Ok(parse_candidate(&value, mapping))
}

/// Validates a base URL and resolves it, refusing private targets by default.
pub async fn resolve_target(
    raw: &str,
    allow_private: bool,
) -> Result<(Url, Vec<SocketAddr>), String> {
    let url = Url::parse(raw).map_err(|error| format!("invalid URL: {error}"))?;
    if url.query().is_some() || url.fragment().is_some() {
        return Err("provider base URLs must not contain a query or fragment".to_string());
    }
    let addrs = resolve_request_target(&url, allow_private).await?;
    Ok((url, addrs))
}

/// Resolves the host of an already-built request URL and applies the SSRF
/// policy. Query strings are allowed here; only the base URL rejects them.
async fn resolve_request_target(url: &Url, allow_private: bool) -> Result<Vec<SocketAddr>, String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("provider URLs must use http or https".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("provider URLs must not contain credentials".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "provider URL must include a host".to_string())?;
    let port = url.port_or_known_default().unwrap_or(80);
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| format!("could not resolve {host}: {error}"))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("could not resolve {host}"));
    }
    if !allow_private
        && addrs
            .iter()
            .any(|addr| crate::webhooks::is_private_ip(addr.ip()))
    {
        return Err("provider targets must be public addresses".to_string());
    }
    Ok(addrs)
}

/// Synchronous validation used at save time (no network).
pub fn validate_base_url(raw: &str, allow_private: bool) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("Base URL is required".to_string());
    }
    if raw.len() > 2048 {
        return Err("Base URL is limited to 2048 characters".to_string());
    }
    let url = Url::parse(raw).map_err(|error| format!("invalid URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Base URL must use http or https".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Base URL must not contain credentials".to_string());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("Base URL must not contain a query or fragment".to_string());
    }
    if url.host_str().is_none() {
        return Err("Base URL must include a host".to_string());
    }
    if !allow_private && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1")) {
        return Err("Provider targets must be public addresses".to_string());
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

/// Validates an administrator-supplied mapping object.
pub fn validate_mapping(value: &Value) -> Result<Mapping, ApiError> {
    let Some(object) = value.as_object() else {
        return Err(ApiError::bad_request(
            "invalid_provider_mapping",
            "The mapping must be a JSON object",
        ));
    };
    if object.len() > CANONICAL_FIELDS.len() {
        return Err(ApiError::bad_request(
            "invalid_provider_mapping",
            "The mapping has too many fields",
        ));
    }
    let mut mapping = Mapping::new();
    for (key, path) in object {
        if !CANONICAL_FIELDS.contains(&key.as_str()) {
            return Err(ApiError::bad_request(
                "invalid_provider_mapping",
                format!("Unknown canonical field in mapping: {key}"),
            ));
        }
        let Some(path) = path.as_str() else {
            return Err(ApiError::bad_request(
                "invalid_provider_mapping",
                format!("The mapping path for {key} must be a string"),
            ));
        };
        validate_path(key, path)?;
        mapping.insert(key.clone(), path.trim().to_string());
    }
    Ok(mapping)
}

fn validate_path(key: &str, path: &str) -> Result<(), ApiError> {
    let path = path.trim();
    if path.is_empty() || path.len() > 200 {
        return Err(ApiError::bad_request(
            "invalid_provider_mapping",
            format!("The mapping path for {key} must be 1-200 characters"),
        ));
    }
    if path.split('.').any(|segment| {
        segment.is_empty()
            || !segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }) {
        return Err(ApiError::bad_request(
            "invalid_provider_mapping",
            format!("The mapping path for {key} must be dot-separated object keys"),
        ));
    }
    Ok(())
}

/// Parses and validates a stored `mapping_json` value.
pub fn mapping_from_json(raw: &str) -> Result<Mapping, ApiError> {
    if raw.trim().is_empty() {
        return Ok(Mapping::new());
    }
    let value: Value = serde_json::from_str(raw).map_err(|error| {
        ApiError::bad_request(
            "invalid_provider_mapping",
            format!("The mapping is not valid JSON: {error}"),
        )
    })?;
    validate_mapping(&value)
}

/// Validates an administrator-supplied provider name.
pub fn validate_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_provider_name",
            "Provider name is required",
        ));
    }
    if name.chars().count() > 64 {
        return Err(ApiError::bad_request(
            "invalid_provider_name",
            "Provider name must be at most 64 characters",
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            "invalid_provider_name",
            "Provider name contains invalid characters",
        ));
    }
    Ok(name.to_string())
}

/// Stable slug stored in `games.provider_slug`.
#[must_use]
pub fn slugify(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "provider".to_string()
    } else {
        slug.chars().take(64).collect()
    }
}

fn sanitise_query(title: &str) -> String {
    title
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(120)
        .collect()
}

fn endpoint(config: &MetadataProviderConfig, segments: &[&str]) -> Result<Url, ProviderError> {
    let mut url = Url::parse(&config.base_url)
        .map_err(|error| ProviderError::InvalidConfig(error.to_string()))?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|()| ProviderError::InvalidUrl("base URL cannot be a base".to_string()))?;
        path.pop_if_empty();
        for segment in segments {
            path.push(segment);
        }
    }
    Ok(url)
}

/// Fetches and decodes one JSON document, bounded in size and time.
async fn get_json(config: &MetadataProviderConfig, url: Url) -> Result<Value, ProviderError> {
    let addrs = resolve_request_target(&url, config.allow_private)
        .await
        .map_err(ProviderError::BlockedTarget)?;
    let host = url
        .host_str()
        .ok_or_else(|| ProviderError::InvalidUrl("missing host".to_string()))?;
    let target = addrs
        .iter()
        .find(|address| address.is_ipv4())
        .unwrap_or(&addrs[0]);
    let client = Client::builder()
        .timeout(config.timeout)
        .connect_timeout(config.timeout.min(Duration::from_secs(5)))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("Ludex/", env!("CARGO_PKG_VERSION")))
        .resolve(host, *target)
        .build()
        .map_err(|error| ProviderError::Request(format!("could not build HTTP client: {error}")))?;

    let mut request = client.get(url).header("Accept", "application/json");
    if let Some(token) = config.token.as_deref() {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let response = request
        .send()
        .await
        .map_err(|error| map_request_error(&error))?;
    let status = response.status();
    if !status.is_success() {
        let snippet = read_limited(response, 2_048)
            .await
            .map(|bytes| {
                String::from_utf8_lossy(&bytes)
                    .chars()
                    .take(200)
                    .collect::<String>()
            })
            .unwrap_or_default();
        return Err(ProviderError::Status(
            status.as_u16(),
            snippet.trim().to_string(),
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(ProviderError::TooLarge);
    }
    let bytes = read_limited(response, MAX_RESPONSE_BYTES).await?;
    serde_json::from_slice(&bytes).map_err(|error| ProviderError::Decode(error.to_string()))
}

fn map_request_error(error: &reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        ProviderError::Timeout
    } else if error.is_builder() {
        ProviderError::InvalidUrl(error.to_string())
    } else {
        ProviderError::Request(error.to_string())
    }
}

/// Reads a body in chunks, refusing to allocate beyond `limit` bytes.
async fn read_limited(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, ProviderError> {
    let mut body = Vec::new();
    loop {
        let chunk = response
            .chunk()
            .await
            .map_err(|error| map_request_error(&error))?;
        let Some(chunk) = chunk else {
            return Ok(body);
        };
        if body.len() + chunk.len() > limit {
            return Err(ProviderError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
}

/// Parses the documented `{"results": [...]}` envelope.
pub fn parse_candidates(
    value: &Value,
    mapping: &Mapping,
) -> Result<Vec<ProviderCandidate>, ProviderError> {
    let results = value
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| ProviderError::Decode("missing `results` array".to_string()))?;
    Ok(results
        .iter()
        .take(MAX_CANDIDATES)
        .filter_map(|item| parse_candidate(item, mapping))
        .collect())
}

fn parse_candidate(item: &Value, mapping: &Mapping) -> Option<ProviderCandidate> {
    if !item.is_object() {
        return None;
    }
    let id = field(item, mapping, "id").and_then(as_string)?;
    let title = field(item, mapping, "title").and_then(as_string)?;
    Some(ProviderCandidate {
        id,
        title,
        description: field(item, mapping, "description").and_then(as_string),
        release_date: field(item, mapping, "releaseDate")
            .and_then(as_string)
            .and_then(|value| db::parse_date(&value)),
        cover_url: field(item, mapping, "coverUrl").and_then(as_string),
        background_url: field(item, mapping, "backgroundUrl").and_then(as_string),
        screenshots: field(item, mapping, "screenshots")
            .map(as_string_list)
            .unwrap_or_default(),
        genres: field(item, mapping, "genres")
            .map(as_string_list)
            .unwrap_or_default(),
        developers: field(item, mapping, "developers")
            .map(as_string_list)
            .unwrap_or_default(),
        publishers: field(item, mapping, "publishers")
            .map(as_string_list)
            .unwrap_or_default(),
        tags: field(item, mapping, "tags")
            .map(as_string_list)
            .unwrap_or_default(),
        rating: field(item, mapping, "rating").and_then(as_f64),
        age_rating: field(item, mapping, "ageRating").and_then(as_age),
        provider_url: field(item, mapping, "providerUrl").and_then(as_string),
    })
}

/// Resolves a canonical field: an explicit mapping path wins over the
/// canonical key, and a missing path falls back to the canonical key.
fn field<'a>(item: &'a Value, mapping: &Mapping, key: &str) -> Option<&'a Value> {
    mapping
        .get(key)
        .and_then(|path| value_at_path(item, path))
        .or_else(|| item.get(key))
}

fn value_at_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

fn as_string(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(text) => text.trim().to_string(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    if text.is_empty() {
        None
    } else {
        Some(text.chars().take(MAX_FIELD_CHARS).collect())
    }
}

fn as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|number| number.is_finite())
}

fn as_age(value: &Value) -> Option<u8> {
    let age = match value {
        Value::Number(number) => number.as_u64().and_then(|age| u8::try_from(age).ok()),
        Value::String(text) => {
            let digits: String = text.chars().filter(char::is_ascii_digit).collect();
            digits.parse::<u8>().ok()
        }
        _ => None,
    }?;
    (age <= 21).then_some(age)
}

fn as_string_list(value: &Value) -> Vec<String> {
    let items: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter_map(|item| match item {
            Value::Object(object) => object.get("name").and_then(as_string),
            other => as_string(other),
        })
        .map(|text| text.chars().take(500).collect::<String>())
        .filter(|text| !text.is_empty())
        .filter(|text| seen.insert(text.to_ascii_lowercase()))
        .take(MAX_LIST_ITEMS)
        .collect()
}

/// Picks the best candidate for `title`/`year` using the shared scorer.
#[must_use]
pub fn best_candidate<'a>(
    candidates: &'a [ProviderCandidate],
    title: &str,
    year: Option<u16>,
) -> Option<&'a ProviderCandidate> {
    candidates
        .iter()
        .filter_map(|candidate| {
            let candidate_year = candidate
                .release_date
                .map(|date| date.format("%Y").to_string())
                .and_then(|value| value.parse::<u16>().ok());
            let score = score_candidate(
                title,
                &candidate.title,
                &MatchContext {
                    query_year: year,
                    candidate_year,
                    has_pc: false,
                    web_only: false,
                    has_release_date: candidate.release_date.is_some(),
                    category: None,
                },
            )?;
            Some((score, candidate))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, candidate)| candidate)
}

/// Records one provider fetch for observability. Never fails the caller.
pub async fn record_fetch(
    pool: &Database,
    provider_id: i64,
    game_id: Option<i64>,
    result: Result<usize, &ProviderError>,
) {
    let (ok, status_code, error) = match result {
        Ok(_) => (true, Some(200_i64), None),
        Err(error) => (
            false,
            error.status_code(),
            Some(error.to_string().chars().take(500).collect::<String>()),
        ),
    };
    let now = db::db_time(db::now());
    let insert = db::query(
        "INSERT INTO provider_fetches (provider_id, game_id, ok, status_code, error, fetched_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(provider_id)
    .bind(game_id)
    .bind(i64::from(ok))
    .bind(status_code)
    .bind(error.as_deref())
    .bind(&now)
    .execute(pool)
    .await;
    if let Err(error) = insert {
        tracing::warn!(provider_id, error = %error, "could not record provider fetch");
        return;
    }
    let cutoff = db::db_time(db::now() - chrono::Duration::days(FETCH_RETENTION_DAYS));
    if let Err(error) = db::query("DELETE FROM provider_fetches WHERE fetched_at < ?")
        .bind(cutoff)
        .execute(pool)
        .await
    {
        tracing::warn!(error = %error, "could not prune provider fetch history");
    }
}

/// Encrypts a provider token for storage.
pub fn encrypt_token(server_secret: &[u8], token: &str) -> Result<String, ApiError> {
    crate::secrets::encrypt(
        server_secret,
        TOKEN_HKDF_SALT,
        TOKEN_HKDF_INFO,
        TOKEN_AAD,
        token,
    )
    .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Instant;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn mapping(pairs: &[(&str, &str)]) -> Mapping {
        pairs
            .iter()
            .map(|(key, path)| ((*key).to_string(), (*path).to_string()))
            .collect()
    }

    #[test]
    fn canonical_response_parses_defensively() {
        let value = json!({
            "results": [
                {
                    "id": 42,
                    "title": "Hades",
                    "description": "<p>Defy the god of the dead</p>",
                    "releaseDate": "2020-09-17",
                    "coverUrl": "https://cdn.example/cover.jpg",
                    "screenshots": ["https://cdn.example/1.jpg", "https://cdn.example/1.jpg"],
                    "genres": [{"name": "Roguelike"}, "Action"],
                    "rating": "93",
                    "ageRating": "PEGI 12"
                },
                {"id": "missing-title"},
                "not an object"
            ]
        });
        let candidates = parse_candidates(&value, &Mapping::new()).expect("parse");
        assert_eq!(candidates.len(), 1);
        let candidate = &candidates[0];
        assert_eq!(candidate.id, "42");
        assert_eq!(
            candidate.description.as_deref(),
            Some("<p>Defy the god of the dead</p>")
        );
        assert_eq!(
            candidate.genres,
            vec!["Roguelike".to_string(), "Action".to_string()]
        );
        assert_eq!(candidate.screenshots.len(), 1);
        assert_eq!(candidate.rating, Some(93.0));
        assert_eq!(candidate.age_rating, Some(12));
        let metadata = candidate.to_metadata("my-provider");
        assert_eq!(
            metadata.description.as_deref(),
            Some("Defy the god of the dead")
        );
        assert_eq!(metadata.provider.slug, "my-provider");
    }

    #[test]
    fn mapping_resolves_nested_paths_and_falls_back() {
        let value = json!({
            "results": [{
                "id": 7,
                "data": {
                    "name": "Celeste",
                    "images": {"cover": "https://cdn.example/celeste.jpg"},
                    "release": "2018-01-25"
                }
            }]
        });
        let mapping = mapping(&[
            ("title", "data.name"),
            ("coverUrl", "data.images.cover"),
            ("releaseDate", "data.release"),
            ("description", "data.missing"),
        ]);
        let candidates = parse_candidates(&value, &mapping).expect("parse");
        assert_eq!(candidates[0].title, "Celeste");
        assert_eq!(
            candidates[0].cover_url.as_deref(),
            Some("https://cdn.example/celeste.jpg")
        );
        assert_eq!(
            candidates[0].release_date.map(|date| date.to_string()),
            Some("2018-01-25 00:00:00 UTC".to_string())
        );
        assert!(candidates[0].description.is_none());
    }

    #[test]
    fn mapping_validation_rejects_unknown_fields_and_bad_paths() {
        assert!(validate_mapping(&json!({"title": "data.name"})).is_ok());
        assert!(validate_mapping(&json!({"nope": "data.name"})).is_err());
        assert!(validate_mapping(&json!({"title": "data..name"})).is_err());
        assert!(validate_mapping(&json!({"title": "data[0].name"})).is_err());
        assert!(validate_mapping(&json!({"title": 3})).is_err());
        assert!(validate_mapping(&json!(["title"])).is_err());
        assert!(mapping_from_json("not json").is_err());
        assert_eq!(mapping_from_json("{}").expect("empty"), Mapping::new());
    }

    #[test]
    fn slugs_are_stable_and_url_safe() {
        assert_eq!(slugify("My Provider!"), "my-provider");
        assert_eq!(slugify("  "), "provider");
        assert_eq!(slugify("IGDB-like_2"), "igdb-like-2");
    }

    #[test]
    fn base_url_validation_accepts_public_and_rejects_credentials() {
        assert_eq!(
            validate_base_url("https://metadata.example.com/api/", false).expect("ok"),
            "https://metadata.example.com/api"
        );
        assert!(validate_base_url("ftp://metadata.example.com", true).is_err());
        assert!(validate_base_url("https://user:pass@metadata.example.com", true).is_err());
        assert!(validate_base_url("https://metadata.example.com/?x=1", true).is_err());
        assert!(validate_base_url("", true).is_err());
        assert!(validate_base_url("http://localhost:8080", false).is_err());
    }

    #[test]
    fn best_candidate_requires_a_confident_match() {
        let candidates = vec![
            ProviderCandidate {
                id: "1".to_string(),
                title: "Hades".to_string(),
                description: None,
                release_date: Some(db::parse_date("2020-09-17").expect("date")),
                cover_url: None,
                background_url: None,
                screenshots: Vec::new(),
                genres: Vec::new(),
                developers: Vec::new(),
                publishers: Vec::new(),
                tags: Vec::new(),
                rating: None,
                age_rating: None,
                provider_url: None,
            },
            ProviderCandidate {
                id: "2".to_string(),
                title: "Hades II".to_string(),
                description: None,
                release_date: None,
                cover_url: None,
                background_url: None,
                screenshots: Vec::new(),
                genres: Vec::new(),
                developers: Vec::new(),
                publishers: Vec::new(),
                tags: Vec::new(),
                rating: None,
                age_rating: None,
                provider_url: None,
            },
        ];
        assert_eq!(
            best_candidate(&candidates, "Hades", Some(2020)).map(|c| c.id.as_str()),
            Some("1")
        );
        assert!(best_candidate(&candidates, "Celeste", None).is_none());
    }

    /// Minimal HTTP/1.1 server used by the network-behaviour tests.
    async fn mock_server(
        handler: fn(&str) -> &'static str,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let address = listener.local_addr().expect("address");
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buffer = [0_u8; 4096];
                    let read = socket.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                    let response = handler(&request);
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        (format!("http://{address}"), handle)
    }

    fn test_config(base_url: &str) -> MetadataProviderConfig {
        MetadataProviderConfig {
            id: 1,
            name: "Fixture".to_string(),
            slug: "fixture".to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            token: Some("secret-token".to_string()),
            priority: 10,
            mapping: Mapping::new(),
            timeout: Duration::from_millis(500),
            allow_private: true,
        }
    }

    #[tokio::test]
    async fn search_sends_token_and_query_and_parses_results() {
        fn handler(request: &str) -> &'static str {
            assert!(
                request.contains("GET /search?title=Hades&year=2020"),
                "{request}"
            );
            assert!(
                request.contains("authorization: Bearer secret-token"),
                "{request}"
            );
            concat!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n",
                "{\"results\":[{\"id\":\"9\",\"title\":\"Hades\",\"coverUrl\":\"https://x/c.jpg\"}]}"
            )
        }
        let (base, _handle) = mock_server(handler).await;
        let config = test_config(&base);
        let candidates = search(&config, "Hades", Some(2020)).await.expect("search");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].title, "Hades");
    }

    #[tokio::test]
    async fn oversized_response_is_rejected() {
        let body = "x".repeat(MAX_RESPONSE_BYTES + 10);
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = [0_u8; 1024];
                let _ = socket.read(&mut buffer).await;
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        let config = test_config(&format!("http://{address}"));
        let error = search(&config, "Hades", None)
            .await
            .expect_err("must reject");
        assert!(matches!(error, ProviderError::TooLarge), "{error}");
    }

    #[tokio::test]
    async fn slow_provider_times_out() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = [0_u8; 1024];
                let _ = socket.read(&mut buffer).await;
                tokio::time::sleep(Duration::from_secs(2)).await;
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 12\r\n\r\n{\"results\":[]}")
                    .await;
            }
        });
        let mut config = test_config(&format!("http://{address}"));
        config.timeout = Duration::from_millis(150);
        let started = Instant::now();
        let error = search(&config, "Hades", None)
            .await
            .expect_err("must time out");
        assert!(matches!(error, ProviderError::Timeout), "{error}");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn private_targets_are_refused_unless_allowed() {
        let config = test_config("http://127.0.0.1:9");
        let mut blocked = config.clone();
        blocked.allow_private = false;
        let error = search(&blocked, "Hades", None)
            .await
            .expect_err("must refuse");
        assert!(matches!(error, ProviderError::BlockedTarget(_)), "{error}");
        assert!(resolve_target("ftp://example.com/x", true).await.is_err());
    }

    #[tokio::test]
    async fn http_errors_are_reported_with_status() {
        fn handler(_: &str) -> &'static str {
            "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 4\r\n\r\nboom"
        }
        let (base, _handle) = mock_server(handler).await;
        let config = test_config(&base);
        let error = search(&config, "Hades", None).await.expect_err("must fail");
        match error {
            ProviderError::Status(code, message) => {
                assert_eq!(code, 503);
                assert!(message.contains("boom"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn fetch_by_id_accepts_single_object_and_envelope() {
        fn handler(request: &str) -> &'static str {
            assert!(request.contains("GET /games/42"), "{request}");
            "HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n{\"id\":\"42\",\"title\":\"Hades\",\"rating\":90}"
        }
        let (base, _handle) = mock_server(handler).await;
        let config = test_config(&base);
        let candidate = fetch_by_id(&config, "42")
            .await
            .expect("fetch")
            .expect("candidate");
        assert_eq!(candidate.id, "42");
        assert_eq!(candidate.rating, Some(90.0));
    }
}
