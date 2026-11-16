//! VirusTotal hash lookups (#224).
//!
//! Ludex already stores a full archive SHA-256 (`game_versions.content_sha256`,
//! #216). VirusTotal indexes files by SHA-256, so a malware signal costs one
//! `GET /files/{sha256}`: no upload, no extraction, no execution.
//!
//! Design constraints that shape this module:
//!
//! * **Hash only by default.** Nothing leaves the machine unless an
//!   administrator explicitly uploads one file with the public-sharing consent
//!   flag. Uploads are never a side effect of "Scan".
//! * **Honest verdicts.** A 404 from VirusTotal is `not_found` ("never seen"),
//!   never `clean`. An outage, a missing key and an exhausted quota are
//!   `unknown`, `disabled` and `quota_exceeded`; none of them maps to `clean`.
//! * **Public quota.** 4 requests/minute and 500/day. The daily counter is
//!   persisted in `virustotal_budget` so a restart does not reset it, and the
//!   per-minute window lives in memory.
//! * **Cache.** `clean`/`not_found` are cached 7 days, `suspicious` 1 day and
//!   `malicious` for an effectively indefinite period (see
//!   [`MALICIOUS_TTL_SECONDS`]). Concurrent scans of the same hash share one
//!   request (single-flight).
//!
//! The API key is encrypted with [`crate::secrets`] under
//! `LUDEX_VIRUSTOTAL_API_KEY`'s camera; the environment variable wins over the
//! database row and cannot be overwritten or deleted through the API. No
//! endpoint, log line or audit event ever contains the key: only a
//! `...last4` fingerprint.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use axum::http::StatusCode;
use chrono::{Duration as ChronoDuration, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::db;
use crate::error::ApiError;
use crate::state::AppState;

/// Public VirusTotal API base (`LUDEX_VIRUSTOTAL_API_BASE` overrides it).
pub const API_BASE: &str = "https://www.virustotal.com/api/v3";

/// Requests the public API accepts per minute.
pub const PER_MINUTE_LIMIT: u32 = 4;

/// Requests the public API accepts per UTC day.
pub const DAILY_LIMIT: i64 = 500;

/// Cache lifetime for a known-clean file.
pub const CLEAN_TTL_SECONDS: i64 = 7 * 24 * 3_600;

/// Cache lifetime for a file VirusTotal has never seen.
pub const NOT_FOUND_TTL_SECONDS: i64 = 7 * 24 * 3_600;

/// Cache lifetime for a file with suspicious detections but no malicious one.
pub const SUSPICIOUS_TTL_SECONDS: i64 = 24 * 3_600;

/// Cache lifetime for a malicious file. A hundred years is "indefinite" for a
/// library server while staying far from any integer overflow; `force` still
/// re-checks.
pub const MALICIOUS_TTL_SECONDS: i64 = 100 * 365 * 24 * 3_600;

/// Largest file accepted by the direct `POST /files` endpoint.
pub const DIRECT_UPLOAD_MAX_BYTES: u64 = 32 * 1024 * 1024;

/// Largest file accepted through `GET /files/upload_url`.
pub const UPLOAD_MAX_BYTES: u64 = 650 * 1024 * 1024;

/// Executables/installers hashed per version in `hash_and_binaries` mode.
pub const MAX_BINARY_CANDIDATES: usize = 5;

/// Yes/no string required in a request to upload a file (the UI warning alone
/// is not consent).
pub const PUBLIC_SHARING_CONSENT: &str = "public-sharing";

/// File extensions treated as executables/installers next to an archive.
const BINARY_EXTENSIONS: &[&str] = &["exe", "msi", "dmg", "pkg", "appimage"];

/// Timeout for one upload request (a 650 MB body needs room).
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(600);

const KEY_HKDF_SALT: &[u8] = b"ludex-virustotal-key";
const KEY_HKDF_INFO: &[u8] = b"virustotal-api-key-v1";
const KEY_AAD: &[u8] = b"virustotal_integration.api_key_enc";

const INTEGRATION_COLUMNS: &str = "api_key_enc, fingerprint, added_at, last_used_at, last_error";
const CACHE_COLUMNS: &str = "verdict, stats_json, checked_at, ttl_seconds, permalink";

/// Verdict stored for one SHA-256.
///
/// The wire values are stable: the UI and the audit trail share them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Known file with no malicious or suspicious detection.
    Clean,
    /// Suspicious detections and no malicious one.
    Suspicious,
    /// At least one malicious detection.
    Malicious,
    /// VirusTotal has never seen the hash (HTTP 404). Never "clean".
    NotFound,
    /// The lookup could not be completed (outage, timeout, unhashed archive).
    Unknown,
    /// No API key is configured, or the integration is switched off.
    Disabled,
    /// Rate limit or daily budget exhausted (locally or at VirusTotal).
    QuotaExceeded,
    /// VirusTotal rejected the API key, or the request was invalid.
    Error,
}

impl Verdict {
    /// Stable lowercase wire value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Clean => "clean",
            Verdict::Suspicious => "suspicious",
            Verdict::Malicious => "malicious",
            Verdict::NotFound => "not_found",
            Verdict::Unknown => "unknown",
            Verdict::Disabled => "disabled",
            Verdict::QuotaExceeded => "quota_exceeded",
            Verdict::Error => "error",
        }
    }

    /// Parses a stored verdict, falling back to `unknown` for stale values.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "clean" => Verdict::Clean,
            "suspicious" => Verdict::Suspicious,
            "malicious" => Verdict::Malicious,
            "not_found" => Verdict::NotFound,
            "disabled" => Verdict::Disabled,
            "quota_exceeded" => Verdict::QuotaExceeded,
            "error" => Verdict::Error,
            _ => Verdict::Unknown,
        }
    }

    /// Cache lifetime for a verdict; `None` means "do not cache".
    #[must_use]
    pub fn ttl_seconds(self) -> Option<i64> {
        match self {
            Verdict::Clean | Verdict::NotFound => Some(NOT_FOUND_TTL_SECONDS),
            Verdict::Suspicious => Some(SUSPICIOUS_TTL_SECONDS),
            Verdict::Malicious => Some(MALICIOUS_TTL_SECONDS),
            Verdict::Unknown | Verdict::Disabled | Verdict::QuotaExceeded | Verdict::Error => None,
        }
    }
}

/// Where a report came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScanSource {
    /// Served from `virustotal_cache`; no API request.
    Cache,
    /// Produced by a VirusTotal request (or by a request that was refused by
    /// the local quota before leaving the machine).
    Api,
}

impl ScanSource {
    /// Stable lowercase wire value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ScanSource::Cache => "cache",
            ScanSource::Api => "api",
        }
    }
}

/// Scan mode requested by an administrator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScanMode {
    /// Look up the stored archive SHA-256 only (default).
    Hash,
    /// Look up the archive, then the executables/installers beside it.
    HashAndBinaries,
    /// Explicitly upload one archive for analysis (opt-in, consent-gated).
    Upload,
}

impl ScanMode {
    /// Parses a request value.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "hash" => Some(ScanMode::Hash),
            "hash_and_binaries" | "hash-and-binaries" => Some(ScanMode::HashAndBinaries),
            "upload" => Some(ScanMode::Upload),
            _ => None,
        }
    }

    /// Stable lowercase wire value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ScanMode::Hash => "hash",
            ScanMode::HashAndBinaries => "hash_and_binaries",
            ScanMode::Upload => "upload",
        }
    }
}

/// Engine counters returned by VirusTotal, stored raw-ish so a later threshold
/// change does not require a re-scan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VtStats {
    /// Engines that flagged the file as malicious.
    pub malicious: i64,
    /// Engines that flagged the file as suspicious.
    pub suspicious: i64,
    /// Engines that found nothing.
    pub undetected: i64,
    /// Engines that classified the file as harmless.
    pub harmless: i64,
    /// Engines that timed out.
    pub timeout: i64,
    /// Engines that failed to analyse the file.
    pub failure: i64,
    /// Sum of every engine bucket above (the "N/72" denominator).
    pub total_engines: i64,
}

/// One engine detection kept for the detail popover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EngineHit {
    /// Engine display name.
    pub engine: String,
    /// `malicious` or `suspicious`.
    pub category: String,
    /// Signature name reported by the engine, when any.
    pub result: Option<String>,
}

/// Normalised `GET /files/{sha256}` answer.
#[derive(Debug, Clone)]
pub struct FileReport {
    /// Hash the report describes.
    pub sha256: String,
    /// File size reported by VirusTotal, when known.
    pub size_bytes: Option<i64>,
    /// Engine counters.
    pub stats: VtStats,
    /// Detections with their engine and signature.
    pub engine_hits: Vec<EngineHit>,
    /// RFC 3339 analysis date, when reported.
    pub last_analysis_date: Option<String>,
    /// Human report link.
    pub permalink: Option<String>,
}

impl FileReport {
    /// Verdict rule: `malicious > 0` wins, then `suspicious > 0`, then clean.
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        if self.stats.malicious > 0 {
            Verdict::Malicious
        } else if self.stats.suspicious > 0 {
            Verdict::Suspicious
        } else {
            Verdict::Clean
        }
    }
}

/// One scan result attached to a version.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    /// Version the report belongs to (`0` when the target vanished).
    pub version_id: i64,
    /// Full SHA-256 that was looked up.
    pub sha256: String,
    /// Verdict (see [`Verdict`]).
    pub verdict: Verdict,
    /// Engine counters.
    pub stats: VtStats,
    /// When the lookup (or the cache entry) was produced, RFC 3339.
    pub checked_at: String,
    /// `cache` or `api`.
    pub source: ScanSource,
    /// VirusTotal report link, when a hash is known.
    pub permalink: Option<String>,
}

/// A scan result plus the metadata the route needs for audits.
#[derive(Debug, Clone)]
pub struct ScanEntry {
    /// Report returned to the caller.
    pub report: ScanReport,
    /// Owning game, for the `game.scanned` audit event.
    pub game_id: i64,
    /// Mode that produced the report.
    pub mode: ScanMode,
}

/// `GET /api/games/{id}/virustotal` payload (cached reports only).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GameScanReport {
    /// Game id.
    pub game_id: i64,
    /// One cached report per version that has been checked.
    pub versions: Vec<ScanReport>,
}

/// Where the active API key comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum KeySource {
    /// `LUDEX_VIRUSTOTAL_API_KEY`; the UI cannot change it.
    Env,
    /// Encrypted row in `virustotal_integration`.
    Database,
    /// No key configured.
    #[serde(rename = "none")]
    Missing,
}

impl KeySource {
    /// Stable lowercase wire value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            KeySource::Env => "env",
            KeySource::Database => "database",
            KeySource::Missing => "none",
        }
    }
}

/// Today's request budget.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuotaStatus {
    /// Daily request ceiling (500 for the public API).
    pub daily_limit: i64,
    /// Requests already made today.
    pub used_today: i64,
    /// Requests left before the UTC reset.
    pub remaining: i64,
    /// RFC 3339 timestamp of the next UTC midnight.
    pub reset_at: String,
}

/// `GET/PUT/DELETE /api/admin/integrations/virustotal` payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationStatus {
    /// A usable key is configured (env or database).
    pub configured: bool,
    /// Which source wins.
    pub source: KeySource,
    /// `...last4` mask; never the key.
    pub fingerprint: Option<String>,
    /// When the database key was saved.
    pub added_at: Option<String>,
    /// Last successful VirusTotal request.
    pub last_used_at: Option<String>,
    /// Last failure, with its category (`invalid_key:`, `quota_exceeded:`,
    /// `outage:`).
    pub last_error: Option<String>,
    /// Today's quota.
    pub quota: QuotaStatus,
}

/// `POST /api/admin/integrations/virustotal/test` payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestResponse {
    /// Whether VirusTotal accepted the key.
    pub ok: bool,
    /// Human explanation, safe to display.
    pub message: String,
}

/// `POST /api/admin/virustotal/scan` body.
#[derive(Debug, Clone, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScanRequest {
    /// Game to scan (every available version).
    #[serde(default)]
    pub game_id: Option<i64>,
    /// Single version to scan.
    #[serde(default)]
    pub version_id: Option<i64>,
    /// `hash` (default), `hash_and_binaries` or `upload`.
    #[serde(default)]
    pub mode: Option<String>,
    /// Ignore a fresh cache entry.
    #[serde(default)]
    pub force: bool,
    /// Required with `mode=upload`: explicit acknowledgement.
    #[serde(default)]
    pub confirm_upload: bool,
    /// Required with `mode=upload`: must be `public-sharing`. The web client
    /// sends it as `iUnderstand`; `consent` is accepted too.
    #[serde(default)]
    pub i_understand: Option<String>,
    /// Legacy name for [`ScanRequest::i_understand`].
    #[serde(default)]
    pub consent: Option<String>,
}

/// Transport failure, classified so the caller can pick a verdict and a reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VtError {
    /// HTTP 404: VirusTotal has never seen the hash.
    NotFound,
    /// HTTP 401/403: the key is invalid or revoked.
    InvalidKey,
    /// HTTP 429 with an optional `Retry-After`.
    RateLimited {
        /// Seconds the server asked us to wait.
        retry_after: Option<u64>,
    },
    /// HTTP 5xx or an unparseable success body.
    Upstream {
        /// HTTP status, when there was one.
        status: u16,
        /// Safe description (never contains the key or URL).
        message: String,
    },
    /// The request timed out.
    Timeout,
    /// Network or local I/O failure.
    Transport(String),
    /// The file is larger than the upload endpoints accept.
    TooLarge {
        /// Actual size in bytes.
        size: u64,
        /// Endpoint limit in bytes.
        limit: u64,
    },
}

impl VtError {
    /// Verdict exposed for this failure.
    ///
    /// An outage is `unknown` (retry later), a rejected key is `error` (fix the
    /// key), a 429 is `quota_exceeded` and a 404 is `not_found`. None of these
    /// is ever mapped to `clean`.
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        match self {
            VtError::NotFound => Verdict::NotFound,
            VtError::InvalidKey => Verdict::Error,
            VtError::RateLimited { .. } => Verdict::QuotaExceeded,
            VtError::Upstream { .. } | VtError::Timeout | VtError::Transport(_) => Verdict::Unknown,
            VtError::TooLarge { .. } => Verdict::Error,
        }
    }

    /// Category-prefixed explanation persisted as `lastError`.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            VtError::NotFound => "not_found: VirusTotal has never seen this file".to_string(),
            VtError::InvalidKey => {
                "invalid_key: VirusTotal rejected the API key".to_string()
            }
            VtError::RateLimited { retry_after } => match retry_after {
                Some(seconds) => format!(
                    "quota_exceeded: VirusTotal rate limit reached; retry after {seconds}s"
                ),
                None => "quota_exceeded: VirusTotal rate limit reached".to_string(),
            },
            VtError::Upstream { status, message } => {
                format!("outage: VirusTotal returned HTTP {status} ({message})")
            }
            VtError::Timeout => "outage: the VirusTotal request timed out".to_string(),
            VtError::Transport(detail) => format!("outage: VirusTotal is unreachable ({detail})"),
            VtError::TooLarge { size, limit } => format!(
                "upload_too_large: the file is {size} bytes; the VirusTotal public API accepts at most {limit} bytes"
            ),
        }
    }
}

/// Answer of an upload submission.
#[derive(Debug, Clone, Default)]
pub struct UploadOutcome {
    /// Analysis id returned by VirusTotal, when parsed.
    pub analysis_id: Option<String>,
}

/// One HTTP transport used by the scanner. Implemented by [`HttpTransport`] in
/// production and by an in-memory mock in tests.
pub trait VtTransport: Send + Sync {
    /// Looks one hash up (`GET /files/{sha256}`).
    fn lookup_file<'a>(
        &'a self,
        api_key: &'a str,
        sha256: &'a str,
    ) -> TransportFuture<'a, Result<FileReport, VtError>>;

    /// Validates a key (`GET /users/{key}`).
    fn validate_key<'a>(&'a self, api_key: &'a str) -> TransportFuture<'a, Result<(), VtError>>;

    /// Uploads one file, either directly (≤ 32 MB) or through
    /// `GET /files/upload_url` (≤ 650 MB).
    fn upload_file<'a>(
        &'a self,
        api_key: &'a str,
        path: &'a Path,
    ) -> TransportFuture<'a, Result<UploadOutcome, VtError>>;
}

/// Boxed future returned by [`VtTransport`] without an extra dependency.
pub type TransportFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Real VirusTotal HTTP client.
pub struct HttpTransport {
    client: reqwest::Client,
    base: String,
    timeout: Duration,
}

/// Returns `true` for an `http://` URL pointing at a loopback address.
///
/// That is the only plaintext base accepted: the integration tests run a mock
/// VirusTotal on `127.0.0.1`, and loopback traffic never leaves the machine.
/// Any other `http://` base is refused, because `GET /users/{key}` puts the API
/// key in the URL path.
fn is_loopback_http(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "http" {
        return false;
    }
    match parsed.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// `true` when the transport may send credentials to this base URL.
fn is_secure_base(url: &str) -> bool {
    url.to_ascii_lowercase().starts_with("https://") || is_loopback_http(url)
}

impl HttpTransport {
    /// Builds the transport from the shared server client.
    ///
    /// `GET /users/{key}` carries the API key in the path, so the base URL must
    /// be HTTPS. A plaintext value (an `http://` override in the environment)
    /// is replaced by the public endpoint instead of sending the key over the
    /// wire in clear text. Loopback is the one exception: the integration tests
    /// run a mock on `127.0.0.1` (#266).
    #[must_use]
    pub fn new(client: reqwest::Client, base: impl Into<String>, timeout: Duration) -> Self {
        let requested = base.into();
        let trimmed = requested.trim();
        let base = if is_secure_base(trimmed) {
            trimmed.trim_end_matches('/').to_string()
        } else {
            if !trimmed.is_empty() && trimmed.trim_end_matches('/') != API_BASE {
                tracing::warn!(
                    "LUDEX_VIRUSTOTAL_API_BASE is not HTTPS; using the public VirusTotal endpoint"
                );
            }
            API_BASE.to_string()
        };
        Self {
            client,
            base,
            timeout,
        }
    }

    /// Base URL actually used, exposed for the configuration tests.
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }
}

/// Maps a reqwest failure without leaking the request URL (it can contain the
/// API key on the `/users/{key}` route).
fn map_reqwest(error: reqwest::Error) -> VtError {
    if error.is_timeout() {
        VtError::Timeout
    } else {
        VtError::Transport(error.without_url().to_string())
    }
}

/// Reads `Retry-After` as seconds, ignoring HTTP-date values we cannot use.
fn retry_after(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

impl VtTransport for HttpTransport {
    fn lookup_file<'a>(
        &'a self,
        api_key: &'a str,
        sha256: &'a str,
    ) -> TransportFuture<'a, Result<FileReport, VtError>> {
        Box::pin(async move {
            let url = format!("{}/files/{}", self.base, sha256);
            let response = self
                .client
                .get(&url)
                .header("x-apikey", api_key)
                .timeout(self.timeout)
                .send()
                .await
                .map_err(map_reqwest)?;
            match response.status().as_u16() {
                200 => {
                    let payload: Value = response.json().await.map_err(map_reqwest)?;
                    parse_file_payload(sha256, &payload)
                }
                404 => Err(VtError::NotFound),
                401 | 403 => Err(VtError::InvalidKey),
                429 => Err(VtError::RateLimited {
                    retry_after: retry_after(&response),
                }),
                status => Err(VtError::Upstream {
                    status,
                    message: "unexpected status".to_string(),
                }),
            }
        })
    }

    fn validate_key<'a>(&'a self, api_key: &'a str) -> TransportFuture<'a, Result<(), VtError>> {
        Box::pin(async move {
            let url = format!("{}/users/{}", self.base, api_key);
            // Belt and braces: `new` already refuses a non-HTTPS base (loopback
            // excepted for the mock), and this keeps a future refactor from
            // sending the key in clear text.
            if !is_secure_base(&url) {
                return Err(VtError::Transport(
                    "refusing to send the API key over plaintext".to_string(),
                ));
            }
            // HTTPS is enforced above and in `HttpTransport::new`; see
            // `plaintext_api_base_is_refused`.
            // codeql[rust/cleartext-transmission]
            let request = self.client.get(&url);
            let response = request
                .header("x-apikey", api_key)
                .timeout(self.timeout)
                .send()
                .await
                .map_err(map_reqwest)?;
            match response.status().as_u16() {
                200 => Ok(()),
                // VirusTotal answers 404 for an unknown user id.
                401 | 403 | 404 => Err(VtError::InvalidKey),
                429 => Err(VtError::RateLimited {
                    retry_after: retry_after(&response),
                }),
                status => Err(VtError::Upstream {
                    status,
                    message: "unexpected status".to_string(),
                }),
            }
        })
    }

    fn upload_file<'a>(
        &'a self,
        api_key: &'a str,
        path: &'a Path,
    ) -> TransportFuture<'a, Result<UploadOutcome, VtError>> {
        Box::pin(async move {
            let metadata = tokio::fs::metadata(path).await.map_err(|error| {
                VtError::Transport(format!("could not read the file to upload: {error}"))
            })?;
            let size = metadata.len();
            if size > UPLOAD_MAX_BYTES {
                return Err(VtError::TooLarge {
                    size,
                    limit: UPLOAD_MAX_BYTES,
                });
            }
            let url = if size <= DIRECT_UPLOAD_MAX_BYTES {
                format!("{}/files", self.base)
            } else {
                let response = self
                    .client
                    .get(format!("{}/files/upload_url", self.base))
                    .header("x-apikey", api_key)
                    .timeout(self.timeout)
                    .send()
                    .await
                    .map_err(map_reqwest)?;
                match response.status().as_u16() {
                    200 => {
                        let payload: Value = response.json().await.map_err(map_reqwest)?;
                        payload
                            .get("data")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .ok_or_else(|| VtError::Upstream {
                                status: 200,
                                message: "the upload URL response had no data field".to_string(),
                            })?
                    }
                    401 | 403 => return Err(VtError::InvalidKey),
                    429 => {
                        return Err(VtError::RateLimited {
                            retry_after: retry_after(&response),
                        })
                    }
                    status => {
                        return Err(VtError::Upstream {
                            status,
                            message: "could not obtain an upload URL".to_string(),
                        })
                    }
                }
            };
            let part = reqwest::multipart::Part::file(path)
                .await
                .map_err(|error| {
                    VtError::Transport(format!("could not open the file to upload: {error}"))
                })?;
            let form = reqwest::multipart::Form::new().part("file", part);
            let response = self
                .client
                .post(&url)
                .header("x-apikey", api_key)
                .timeout(UPLOAD_TIMEOUT)
                .multipart(form)
                .send()
                .await
                .map_err(map_reqwest)?;
            match response.status().as_u16() {
                200 | 201 => {
                    let payload: Value = response.json().await.unwrap_or(Value::Null);
                    Ok(UploadOutcome {
                        analysis_id: payload
                            .get("data")
                            .and_then(|data| data.get("id"))
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                }
                401 | 403 => Err(VtError::InvalidKey),
                413 => Err(VtError::TooLarge {
                    size,
                    limit: UPLOAD_MAX_BYTES,
                }),
                429 => Err(VtError::RateLimited {
                    retry_after: retry_after(&response),
                }),
                status => Err(VtError::Upstream {
                    status,
                    message: "the upload was refused".to_string(),
                }),
            }
        })
    }
}

/// Parses a `GET /files/{sha256}` payload into a [`FileReport`].
fn parse_file_payload(sha256: &str, payload: &Value) -> Result<FileReport, VtError> {
    let attributes = payload
        .get("data")
        .and_then(|data| data.get("attributes"))
        .ok_or_else(|| VtError::Upstream {
            status: 200,
            message: "the response had no data.attributes".to_string(),
        })?;
    let stats_value = attributes
        .get("last_analysis_stats")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let stat = |name: &str| stats_value.get(name).and_then(Value::as_i64).unwrap_or(0);
    let stats = VtStats {
        malicious: stat("malicious"),
        suspicious: stat("suspicious"),
        undetected: stat("undetected"),
        harmless: stat("harmless"),
        timeout: stat("timeout"),
        failure: stat("failure"),
        total_engines: stat("malicious")
            + stat("suspicious")
            + stat("undetected")
            + stat("harmless")
            + stat("timeout")
            + stat("failure"),
    };
    let engine_hits = attributes
        .get("last_analysis_results")
        .and_then(Value::as_object)
        .map(|results| {
            results
                .iter()
                .filter_map(|(engine, entry)| {
                    let category = entry.get("category").and_then(Value::as_str)?;
                    if category != "malicious" && category != "suspicious" {
                        return None;
                    }
                    Some(EngineHit {
                        engine: engine.clone(),
                        category: category.to_string(),
                        result: entry
                            .get("result")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let last_analysis_date = attributes
        .get("last_analysis_date")
        .and_then(Value::as_i64)
        .and_then(|seconds| Utc.timestamp_opt(seconds, 0).single())
        .map(db::db_time);
    Ok(FileReport {
        sha256: attributes
            .get("sha256")
            .and_then(Value::as_str)
            .unwrap_or(sha256)
            .to_ascii_lowercase(),
        size_bytes: attributes.get("size").and_then(Value::as_i64),
        stats,
        engine_hits,
        last_analysis_date,
        permalink: Some(gui_permalink(sha256)),
    })
}

/// Human VirusTotal report link for a hash.
#[must_use]
pub fn gui_permalink(sha256: &str) -> String {
    format!("https://www.virustotal.com/gui/file/{sha256}")
}

/// Today's budget row (UTC).
#[derive(Debug, Clone, sqlx::FromRow)]
struct BudgetRow {
    used: i64,
}

/// One `virustotal_integration` row.
#[derive(Debug, Clone, sqlx::FromRow)]
struct IntegrationRow {
    api_key_enc: String,
    fingerprint: String,
    added_at: String,
    last_used_at: Option<String>,
    last_error: Option<String>,
}

/// One `virustotal_cache` row.
#[derive(Debug, Clone, sqlx::FromRow)]
struct CacheRow {
    verdict: String,
    stats_json: String,
    checked_at: String,
    ttl_seconds: i64,
    permalink: Option<String>,
}

/// One `game_versions` row needed by a scan.
#[derive(Debug, Clone, sqlx::FromRow)]
struct VersionRow {
    id: i64,
    game_id: i64,
    file_path: String,
    root_id: Option<i64>,
    storage_backend: Option<String>,
    content_sha256: Option<String>,
}

impl VersionRow {
    fn is_remote(&self) -> bool {
        self.storage_backend.as_deref() == Some("s3")
    }
}

/// A resolved API key. [`std::fmt::Debug`] redacts the secret.
#[derive(Clone)]
pub struct ResolvedKey {
    /// Plaintext key, only held in memory for the request.
    pub key: String,
    /// Where it came from.
    pub source: KeySource,
    /// `...last4` mask safe for responses.
    pub fingerprint: String,
    /// Database row creation time (env keys have none).
    pub added_at: Option<String>,
    /// Last successful request.
    pub last_used_at: Option<String>,
    /// Last failure.
    pub last_error: Option<String>,
}

impl std::fmt::Debug for ResolvedKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedKey")
            .field("key", &"<redacted>")
            .field("source", &self.source)
            .field("fingerprint", &self.fingerprint)
            .field("added_at", &self.added_at)
            .field("last_used_at", &self.last_used_at)
            .field("last_error", &self.last_error)
            .finish()
    }
}

/// State of the key lookup, including an unreadable database row.
#[derive(Debug, Clone)]
pub enum KeyState {
    /// A usable key.
    Key(ResolvedKey),
    /// Nothing configured.
    Missing,
    /// A row exists but cannot be decrypted (server secret rotated).
    Unreadable(String),
}

/// Runtime counters that are not worth a database round trip.
#[derive(Debug, Default)]
struct RuntimeStatus {
    last_used_at: Option<String>,
    last_error: Option<String>,
}

/// The scanner shared through [`AppState`].
///
/// Holds the injectable transport, the single-flight table and the in-memory
/// per-minute window. The daily counter lives in the database.
pub struct Scanner {
    transport: Arc<dyn VtTransport>,
    flight: StdMutex<HashMap<String, Arc<Mutex<()>>>>,
    minute: StdMutex<VecDeque<i64>>,
    runtime: StdMutex<RuntimeStatus>,
    rescan_busy: std::sync::atomic::AtomicBool,
}

impl Scanner {
    /// Builds the production scanner from the shared HTTP client.
    #[must_use]
    pub fn new(config: &crate::config::Config, client: reqwest::Client) -> Self {
        let transport = Arc::new(HttpTransport::new(
            client,
            config.virustotal_api_base.clone(),
            config.virustotal_request_timeout,
        ));
        Self::with_transport(transport)
    }

    /// Builds a scanner with an explicit transport (tests, future backends).
    #[must_use]
    pub fn with_transport(transport: Arc<dyn VtTransport>) -> Self {
        Self {
            transport,
            flight: StdMutex::new(HashMap::new()),
            minute: StdMutex::new(VecDeque::new()),
            runtime: StdMutex::new(RuntimeStatus::default()),
            rescan_busy: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Claims the single periodic re-check slot (#239).
    ///
    /// Returns `false` when a pass is already running, so the worker and the
    /// admin "run now" action never walk the same queue twice.
    pub(crate) fn claim_rescan(&self) -> bool {
        self.rescan_busy
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
    }

    /// Releases the periodic re-check slot.
    pub(crate) fn release_rescan(&self) {
        self.rescan_busy
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Transport used for lookups.
    #[must_use]
    pub fn transport(&self) -> &Arc<dyn VtTransport> {
        &self.transport
    }

    fn shared_flight(&self, sha256: &str) -> Arc<Mutex<()>> {
        let mut flights = self
            .flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        flights
            .entry(sha256.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn release_flight(&self, sha256: &str) {
        let mut flights = self
            .flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = flights.get(sha256) {
            // Only the map owns the lock now: nobody is waiting on it.
            if Arc::strong_count(entry) == 1 {
                flights.remove(sha256);
            }
        }
    }

    /// Reserves one request in the trailing sixty-second window.
    ///
    /// Returns `Some(retry_after_seconds)` when the window is full.
    fn take_minute_slot(&self, limit: u32) -> Option<u64> {
        if limit == 0 {
            return None;
        }
        let now = Utc::now().timestamp();
        let mut window = self
            .minute
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while window.front().is_some_and(|start| *start <= now - 60) {
            window.pop_front();
        }
        if window.len() >= limit as usize {
            let retry = window
                .front()
                .map(|start| (start + 60 - now).max(1) as u64)
                .unwrap_or(60);
            return Some(retry);
        }
        window.push_back(now);
        None
    }

    fn snapshot_runtime(&self) -> RuntimeStatus {
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        RuntimeStatus {
            last_used_at: runtime.last_used_at.clone(),
            last_error: runtime.last_error.clone(),
        }
    }

    fn update_runtime(&self, used_at: Option<&str>, error: Option<&str>, clear_error: bool) {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(used_at) = used_at {
            runtime.last_used_at = Some(used_at.to_string());
        }
        if clear_error {
            runtime.last_error = None;
        } else if let Some(error) = error {
            runtime.last_error = Some(error.to_string());
        }
    }
}

/// UTC date used as the budget key.
fn today() -> String {
    db::now().format("%Y-%m-%d").to_string()
}

/// Seconds until the next UTC midnight.
fn seconds_until_reset() -> u64 {
    let now = db::now();
    let tomorrow = (now + ChronoDuration::days(1))
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap_or_else(|| now.naive_utc());
    let reset = Utc.from_utc_datetime(&tomorrow);
    u64::try_from((reset - now).num_seconds().max(1)).unwrap_or(86_400)
}

/// RFC 3339 timestamp of the next UTC midnight.
fn next_reset_at() -> String {
    let now = db::now();
    let tomorrow = (now + ChronoDuration::days(1))
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap_or_else(|| now.naive_utc());
    db::db_time(Utc.from_utc_datetime(&tomorrow))
}

/// Fingerprint shown to administrators: the last four characters only.
///
/// A key too short to hide its tail is fully masked, so a malformed value can
/// never be reconstructed from the fingerprint.
#[must_use]
pub fn fingerprint(api_key: &str) -> String {
    if api_key.chars().count() < 8 {
        return "...".to_string();
    }
    let tail: String = api_key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<char>>()
        .into_iter()
        .rev()
        .collect();
    format!("...{tail}")
}

async fn load_integration_row(state: &AppState) -> anyhow::Result<Option<IntegrationRow>> {
    db::query_as::<IntegrationRow>(&format!(
        "SELECT {INTEGRATION_COLUMNS} FROM virustotal_integration WHERE id = 1"
    ))
    .fetch_optional(&state.pool)
    .await
    .map_err(Into::into)
}

/// Resolves the active key: environment first, then the encrypted row.
pub async fn key_state(state: &AppState) -> KeyState {
    if let Some(key) = state
        .config
        .virustotal_api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
    {
        let runtime = state.virustotal.snapshot_runtime();
        return KeyState::Key(ResolvedKey {
            key: key.to_string(),
            source: KeySource::Env,
            fingerprint: fingerprint(key),
            added_at: None,
            last_used_at: runtime.last_used_at,
            last_error: runtime.last_error,
        });
    }
    let row = match load_integration_row(state).await {
        Ok(row) => row,
        Err(error) => {
            tracing::warn!(%error, "could not read the VirusTotal integration row");
            return KeyState::Missing;
        }
    };
    let Some(row) = row else {
        return KeyState::Missing;
    };
    match crate::secrets::decrypt(
        &state.jwt_secret,
        KEY_HKDF_SALT,
        KEY_HKDF_INFO,
        KEY_AAD,
        &row.api_key_enc,
    ) {
        Ok(key) => KeyState::Key(ResolvedKey {
            key,
            source: KeySource::Database,
            fingerprint: row.fingerprint,
            added_at: Some(row.added_at),
            last_used_at: row.last_used_at,
            last_error: row.last_error,
        }),
        Err(error) => {
            tracing::warn!(
                %error,
                "the stored VirusTotal API key cannot be decrypted; save it again"
            );
            KeyState::Unreadable(
                "the stored VirusTotal API key cannot be decrypted (the server secret changed); save it again"
                    .to_string(),
            )
        }
    }
}

/// Today's request usage.
pub async fn used_today(state: &AppState) -> i64 {
    let day = today();
    db::query_as::<BudgetRow>("SELECT used FROM virustotal_budget WHERE day = ?")
        .bind(&day)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten()
        .map_or(0, |row| row.used.max(0))
}

/// Today's quota, for the integrations screen.
pub async fn quota_status(state: &AppState) -> QuotaStatus {
    let limit = state.config.virustotal_daily_limit;
    let used = used_today(state).await;
    QuotaStatus {
        daily_limit: limit,
        used_today: used,
        remaining: (limit - used).max(0),
        reset_at: next_reset_at(),
    }
}

/// Current integration status.
pub async fn integration_status(state: &AppState) -> IntegrationStatus {
    let quota = quota_status(state).await;
    match key_state(state).await {
        KeyState::Key(key) => IntegrationStatus {
            configured: true,
            source: key.source,
            fingerprint: Some(key.fingerprint),
            added_at: key.added_at,
            last_used_at: key.last_used_at,
            last_error: key.last_error,
            quota,
        },
        KeyState::Missing => IntegrationStatus {
            configured: false,
            source: KeySource::Missing,
            fingerprint: None,
            added_at: None,
            last_used_at: None,
            last_error: None,
            quota,
        },
        KeyState::Unreadable(error) => IntegrationStatus {
            configured: false,
            source: KeySource::Database,
            fingerprint: None,
            added_at: None,
            last_used_at: None,
            last_error: Some(error),
            quota,
        },
    }
}

/// Validates key material before it is stored or used.
fn validate_key_material(api_key: &str) -> Result<(), ApiError> {
    if api_key.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_api_key",
            "The API key is required",
        ));
    }
    if api_key.chars().count() < 8 || api_key.len() > 256 || api_key.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            "invalid_api_key",
            "The API key must be 8 to 256 printable characters",
        ));
    }
    Ok(())
}

/// Stores an encrypted API key (admin only). Refuses when the environment
/// pins one.
pub async fn save_key(state: &AppState, api_key: &str) -> Result<IntegrationStatus, ApiError> {
    if state
        .config
        .virustotal_api_key
        .as_deref()
        .is_some_and(|key| !key.trim().is_empty())
    {
        return Err(ApiError::conflict(
            "env_managed",
            "LUDEX_VIRUSTOTAL_API_KEY is set; the key cannot be changed from the API",
        ));
    }
    let key = api_key.trim();
    validate_key_material(key)?;
    let encrypted = crate::secrets::encrypt(
        &state.jwt_secret,
        KEY_HKDF_SALT,
        KEY_HKDF_INFO,
        KEY_AAD,
        key,
    )?;
    let now = db::db_time(db::now());
    db::query(
        "INSERT INTO virustotal_integration (id, api_key_enc, fingerprint, added_at)
         VALUES (1, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET
            api_key_enc = excluded.api_key_enc,
            fingerprint = excluded.fingerprint,
            added_at = excluded.added_at,
            last_used_at = NULL,
            last_error = NULL",
    )
    .bind(&encrypted)
    .bind(fingerprint(key))
    .bind(&now)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    state.virustotal.update_runtime(None, None, true);
    Ok(integration_status(state).await)
}

/// Deletes the stored key. Refuses when the environment pins one.
pub async fn delete_key(state: &AppState) -> Result<IntegrationStatus, ApiError> {
    if state
        .config
        .virustotal_api_key
        .as_deref()
        .is_some_and(|key| !key.trim().is_empty())
    {
        return Err(ApiError::conflict(
            "env_managed",
            "LUDEX_VIRUSTOTAL_API_KEY is set; the key cannot be deleted from the API",
        ));
    }
    db::query("DELETE FROM virustotal_integration WHERE id = 1")
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    state.virustotal.update_runtime(None, None, true);
    Ok(integration_status(state).await)
}

/// Records the last request outcome. The database row is only updated for a
/// database-stored key; an environment key keeps its counters in memory.
async fn record_use(state: &AppState, error: Option<&str>) {
    let now = db::db_time(db::now());
    state
        .virustotal
        .update_runtime(Some(&now), error, error.is_none());
    if state
        .config
        .virustotal_api_key
        .as_deref()
        .is_some_and(|key| !key.trim().is_empty())
    {
        return;
    }
    let update = if let Some(error) = error {
        db::query("UPDATE virustotal_integration SET last_used_at = ?, last_error = ? WHERE id = 1")
            .bind(&now)
            .bind(error)
    } else {
        db::query(
            "UPDATE virustotal_integration SET last_used_at = ?, last_error = NULL WHERE id = 1",
        )
        .bind(&now)
    };
    if let Err(failure) = update.execute(&state.pool).await {
        tracing::warn!(%failure, "could not record the VirusTotal request outcome");
    }
}

/// Validates an API key against `GET /users/{key}`.
///
/// `provided` is the key from the request body: when present it is tested as
/// given (never stored, never recorded as the integration's last error), which
/// lets the admin screen validate a key before saving it. Without it the
/// stored or environment key is used and its last-used/last-error state is
/// updated as usual. Either way the call consumes one quota slot.
pub async fn test_key(state: &AppState, provided: Option<&str>) -> Result<TestResponse, ApiError> {
    let transient = provided.map(str::trim).filter(|key| !key.is_empty());
    let (api_key, stored) = if let Some(key) = transient {
        validate_key_material(key)?;
        (key.to_string(), false)
    } else {
        match key_state(state).await {
            KeyState::Key(key) => (key.key, true),
            KeyState::Missing => {
                return Ok(TestResponse {
                    ok: false,
                    message: "No VirusTotal API key is configured.".to_string(),
                })
            }
            KeyState::Unreadable(error) => {
                return Ok(TestResponse {
                    ok: false,
                    message: error,
                })
            }
        }
    };
    if let Err(rejection) = acquire_slot(state).await {
        let message = rejection.message();
        if stored {
            record_use(state, Some(&message)).await;
        }
        return Ok(TestResponse { ok: false, message });
    }
    match state.virustotal.transport().validate_key(&api_key).await {
        Ok(()) => {
            if stored {
                record_use(state, None).await;
            }
            Ok(TestResponse {
                ok: true,
                message: "VirusTotal accepted the API key.".to_string(),
            })
        }
        Err(error) => {
            let message = error.message();
            if stored {
                record_use(state, Some(&message)).await;
            }
            Ok(TestResponse { ok: false, message })
        }
    }
}

/// Why a request was refused before leaving the machine.
struct SlotRejection {
    retry_after: u64,
    daily: bool,
}

impl SlotRejection {
    fn message(&self) -> String {
        if self.daily {
            "quota_exceeded: the daily VirusTotal request budget is exhausted".to_string()
        } else {
            format!(
                "quota_exceeded: the VirusTotal rate limit is reached; retry after {}s",
                self.retry_after
            )
        }
    }
}

/// Consumes one request slot (per-minute window + persisted daily budget).
async fn acquire_slot(state: &AppState) -> Result<(), SlotRejection> {
    let limit = state.config.virustotal_daily_limit;
    if limit <= 0 {
        return Err(SlotRejection {
            retry_after: seconds_until_reset(),
            daily: true,
        });
    }
    if used_today(state).await >= limit {
        return Err(SlotRejection {
            retry_after: seconds_until_reset(),
            daily: true,
        });
    }
    if let Some(retry_after) = state
        .virustotal
        .take_minute_slot(state.config.virustotal_per_minute)
    {
        return Err(SlotRejection {
            retry_after,
            daily: false,
        });
    }
    let day = today();
    let insert = db::query(
        "INSERT INTO virustotal_budget (day, used) VALUES (?, 0) ON CONFLICT (day) DO NOTHING",
    )
    .bind(&day)
    .execute(&state.pool)
    .await;
    if let Err(error) = insert {
        tracing::warn!(%error, "could not create the VirusTotal daily budget row");
        return Err(SlotRejection {
            retry_after: seconds_until_reset(),
            daily: true,
        });
    }
    let update =
        db::query("UPDATE virustotal_budget SET used = used + 1 WHERE day = ? AND used < ?")
            .bind(&day)
            .bind(limit)
            .execute(&state.pool)
            .await;
    match update {
        Ok(result) if result.rows_affected() == 1 => Ok(()),
        Ok(_) => Err(SlotRejection {
            retry_after: seconds_until_reset(),
            daily: true,
        }),
        Err(error) => {
            tracing::warn!(%error, "could not consume the VirusTotal daily budget");
            Err(SlotRejection {
                retry_after: seconds_until_reset(),
                daily: true,
            })
        }
    }
}

async fn load_cache(state: &AppState, sha256: &str) -> Option<CacheRow> {
    match db::query_as::<CacheRow>(&format!(
        "SELECT {CACHE_COLUMNS} FROM virustotal_cache WHERE sha256 = ?"
    ))
    .bind(sha256)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(row) => row,
        Err(error) => {
            tracing::warn!(%error, "could not read the VirusTotal cache");
            None
        }
    }
}

fn cache_is_fresh(row: &CacheRow) -> bool {
    let Some(checked_at) = db::parse_time(&row.checked_at) else {
        return false;
    };
    let expires = checked_at + ChronoDuration::seconds(row.ttl_seconds.max(0));
    Utc::now() < expires
}

fn cache_report(version_id: i64, sha256: &str, row: &CacheRow) -> ScanReport {
    ScanReport {
        version_id,
        sha256: sha256.to_string(),
        verdict: Verdict::parse(&row.verdict),
        stats: serde_json::from_str(&row.stats_json).unwrap_or_default(),
        checked_at: row.checked_at.clone(),
        source: ScanSource::Cache,
        permalink: row
            .permalink
            .clone()
            .or_else(|| Some(gui_permalink(sha256))),
    }
}

async fn store_cache(state: &AppState, sha256: &str, report: &FileReport, verdict: Verdict) {
    let Some(ttl) = verdict.ttl_seconds() else {
        return;
    };
    let now = db::db_time(db::now());
    let stats = serde_json::to_string(&report.stats).unwrap_or_else(|_| "{}".to_string());
    let hits = serde_json::to_string(&report.engine_hits).unwrap_or_else(|_| "[]".to_string());
    let write = db::query(
        "INSERT INTO virustotal_cache
            (sha256, size_bytes, verdict, stats_json, engine_hits_json,
             last_analysis_date, checked_at, ttl_seconds, permalink)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (sha256) DO UPDATE SET
            size_bytes = excluded.size_bytes,
            verdict = excluded.verdict,
            stats_json = excluded.stats_json,
            engine_hits_json = excluded.engine_hits_json,
            last_analysis_date = excluded.last_analysis_date,
            checked_at = excluded.checked_at,
            ttl_seconds = excluded.ttl_seconds,
            permalink = excluded.permalink",
    )
    .bind(sha256)
    .bind(report.size_bytes)
    .bind(verdict.as_str())
    .bind(stats)
    .bind(hits)
    .bind(report.last_analysis_date.as_deref())
    .bind(&now)
    .bind(ttl)
    .bind(report.permalink.as_deref())
    .execute(&state.pool)
    .await;
    if let Err(error) = write {
        tracing::warn!(%error, "could not cache the VirusTotal report");
    }
}

/// Caches a `not_found` answer without pretending it is a clean file.
async fn store_not_found(state: &AppState, sha256: &str) {
    let report = FileReport {
        sha256: sha256.to_string(),
        size_bytes: None,
        stats: VtStats::default(),
        engine_hits: Vec::new(),
        last_analysis_date: None,
        permalink: None,
    };
    store_cache(state, sha256, &report, Verdict::NotFound).await;
}

/// Verdict report that carried no API answer.
fn plain_report(version_id: i64, sha256: &str, verdict: Verdict, checked_at: &str) -> ScanReport {
    ScanReport {
        version_id,
        sha256: sha256.to_string(),
        verdict,
        stats: VtStats::default(),
        checked_at: checked_at.to_string(),
        source: ScanSource::Api,
        permalink: if sha256.is_empty() {
            None
        } else {
            Some(gui_permalink(sha256))
        },
    }
}

pub(crate) async fn store_link(
    state: &AppState,
    game_id: i64,
    version_id: Option<i64>,
    sha256: &str,
    verdict: Verdict,
    mode: ScanMode,
    source: ScanSource,
) {
    let write = db::query(
        "INSERT INTO virustotal_links
            (game_id, version_id, sha256, verdict, mode, source, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(game_id)
    .bind(version_id)
    .bind(sha256)
    .bind(verdict.as_str())
    .bind(mode.as_str())
    .bind(source.as_str())
    .bind(db::db_time(db::now()))
    .execute(&state.pool)
    .await;
    if let Err(error) = write {
        tracing::warn!(%error, "could not record the VirusTotal association");
    }
}

/// Runs a scan and returns one report per looked-up hash.
pub async fn scan(state: &AppState, request: ScanRequest) -> Result<Vec<ScanEntry>, ApiError> {
    let mode = match request.mode.as_deref() {
        None | Some("") => ScanMode::Hash,
        Some(value) => ScanMode::parse(value).ok_or_else(|| {
            ApiError::bad_request(
                "invalid_mode",
                "mode must be 'hash', 'hash_and_binaries' or 'upload'",
            )
        })?,
    };
    if request.game_id.is_none() && request.version_id.is_none() {
        return Err(ApiError::bad_request(
            "missing_target",
            "gameId or versionId is required",
        ));
    }
    let versions = load_versions(state, request.game_id, request.version_id).await?;
    if versions.is_empty() {
        return Err(ApiError::not_found(
            "target_not_found",
            "No available version matches this request",
        ));
    }
    if mode == ScanMode::Upload {
        if versions.len() != 1 {
            return Err(ApiError::unprocessable(
                "upload_requires_single_version",
                "Upload mode accepts exactly one version; pass versionId",
            ));
        }
        if !request.confirm_upload
            || request
                .i_understand
                .as_deref()
                .or(request.consent.as_deref())
                != Some(PUBLIC_SHARING_CONSENT)
        {
            return Err(ApiError::unprocessable(
                "upload_consent_required",
                "Uploading shares the file with VirusTotal's community and partners; pass confirmUpload=true and iUnderstand=\"public-sharing\"",
            ));
        }
    }

    let key = match key_state(state).await {
        KeyState::Key(key) => Some(key),
        KeyState::Missing => None,
        KeyState::Unreadable(error) => {
            tracing::warn!(%error, "VirusTotal key is unreadable; scan reports disabled");
            None
        }
    };
    if mode == ScanMode::Upload && key.is_none() {
        return Err(ApiError::conflict(
            "virustotal_disabled",
            "No VirusTotal API key is configured",
        ));
    }

    let mut entries = Vec::new();
    for version in &versions {
        if mode == ScanMode::Upload {
            let Some(key) = key.as_ref() else {
                return Err(ApiError::conflict(
                    "virustotal_disabled",
                    "No VirusTotal API key is configured",
                ));
            };
            entries.push(upload_version(state, version, key).await?);
            continue;
        }

        let sha256 = crate::content_hash::version_sha256(state, version.id)
            .await
            .map_err(ApiError::from)?;
        let Some(sha256) = sha256 else {
            let checked_at = db::db_time(db::now());
            let report = plain_report(version.id, "", Verdict::Unknown, &checked_at);
            store_link(
                state,
                version.game_id,
                Some(version.id),
                "",
                report.verdict,
                mode,
                report.source,
            )
            .await;
            entries.push(ScanEntry {
                report,
                game_id: version.game_id,
                mode,
            });
            continue;
        };

        let report = lookup_hash(state, key.as_ref(), version.id, &sha256, request.force).await;
        store_link(
            state,
            version.game_id,
            Some(version.id),
            &sha256,
            report.verdict,
            mode,
            report.source,
        )
        .await;
        entries.push(ScanEntry {
            report,
            game_id: version.game_id,
            mode,
        });

        if mode == ScanMode::HashAndBinaries {
            for binary in binary_candidates(state, version).await {
                let digest = hash_binary(&binary).await;
                let Some(digest) = digest else {
                    continue;
                };
                let report =
                    lookup_hash(state, key.as_ref(), version.id, &digest, request.force).await;
                store_link(
                    state,
                    version.game_id,
                    Some(version.id),
                    &digest,
                    report.verdict,
                    mode,
                    report.source,
                )
                .await;
                entries.push(ScanEntry {
                    report,
                    game_id: version.game_id,
                    mode,
                });
            }
        }
    }
    Ok(entries)
}

/// Looks one hash up, honouring the cache, the quota and single-flight.
///
/// Shared with the periodic re-check worker (#239), which passes `force: true`
/// so a stale-but-cached report is actually refreshed.
pub(crate) async fn lookup_hash(
    state: &AppState,
    key: Option<&ResolvedKey>,
    version_id: i64,
    sha256: &str,
    force: bool,
) -> ScanReport {
    let sha256 = sha256.to_ascii_lowercase();
    let cached = if force {
        None
    } else {
        load_cache(state, &sha256)
            .await
            .filter(cache_is_fresh)
            .map(|row| cache_report(version_id, &sha256, &row))
    };
    if let Some(report) = cached {
        return report;
    }
    let Some(key) = key else {
        let checked_at = db::db_time(db::now());
        return plain_report(version_id, &sha256, Verdict::Disabled, &checked_at);
    };

    let flight = state.virustotal.shared_flight(&sha256);
    let report = {
        let _guard = flight.lock().await;
        // A concurrent scan of the same hash may have filled the cache while
        // this one waited for the flight lock.
        let cached = if force {
            None
        } else {
            load_cache(state, &sha256)
                .await
                .filter(cache_is_fresh)
                .map(|row| cache_report(version_id, &sha256, &row))
        };
        if let Some(report) = cached {
            report
        } else if let Err(rejection) = acquire_slot(state).await {
            let message = rejection.message();
            record_use(state, Some(&message)).await;
            let checked_at = db::db_time(db::now());
            plain_report(version_id, &sha256, Verdict::QuotaExceeded, &checked_at)
        } else {
            match state
                .virustotal
                .transport()
                .lookup_file(&key.key, &sha256)
                .await
            {
                Ok(file) => {
                    let verdict = file.verdict();
                    store_cache(state, &sha256, &file, verdict).await;
                    record_use(state, None).await;
                    ScanReport {
                        version_id,
                        sha256: sha256.clone(),
                        verdict,
                        stats: file.stats,
                        checked_at: db::db_time(db::now()),
                        source: ScanSource::Api,
                        permalink: file.permalink.or_else(|| Some(gui_permalink(&sha256))),
                    }
                }
                Err(VtError::NotFound) => {
                    store_not_found(state, &sha256).await;
                    record_use(state, None).await;
                    let checked_at = db::db_time(db::now());
                    plain_report(version_id, &sha256, Verdict::NotFound, &checked_at)
                }
                Err(error) => {
                    let message = error.message();
                    record_use(state, Some(&message)).await;
                    let checked_at = db::db_time(db::now());
                    plain_report(version_id, &sha256, error.verdict(), &checked_at)
                }
            }
        }
    };
    drop(flight);
    state.virustotal.release_flight(&sha256);
    report
}

/// Explicit, consent-gated upload of one archive.
async fn upload_version(
    state: &AppState,
    version: &VersionRow,
    key: &ResolvedKey,
) -> Result<ScanEntry, ApiError> {
    if version.is_remote() {
        return Err(ApiError::unprocessable(
            "upload_requires_local_archive",
            "Only an archive stored on a local library root can be uploaded",
        ));
    }
    let Some(path) = local_path(state, version).await else {
        return Err(ApiError::not_found(
            "archive_not_found",
            "The archive file is not readable on disk",
        ));
    };
    let metadata = tokio::fs::metadata(&path).await.map_err(|error| {
        tracing::warn!(version = version.id, %error, "could not stat the archive to upload");
        ApiError::new(
            StatusCode::NOT_FOUND,
            "archive_not_found",
            "The archive file is not readable on disk",
        )
    })?;
    let size = metadata.len();
    if size > UPLOAD_MAX_BYTES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "upload_too_large",
            format!(
                "The archive is {size} bytes; the VirusTotal public API accepts at most {UPLOAD_MAX_BYTES} bytes (650 MB), so this file can only be checked by hash"
            ),
        ));
    }
    let sha256 = crate::content_hash::version_sha256(state, version.id)
        .await
        .map_err(ApiError::from)?
        .unwrap_or_default();
    if let Err(rejection) = acquire_slot(state).await {
        let message = rejection.message();
        record_use(state, Some(&message)).await;
        return Err(ApiError::too_many_requests("quota_exceeded", message)
            .with_retry_after(rejection.retry_after));
    }
    let outcome = state
        .virustotal
        .transport()
        .upload_file(&key.key, &path)
        .await
        .map_err(|error| {
            let message = error.message();
            vt_error_to_api(&error, &message)
        })?;
    let _ = outcome.analysis_id;
    record_use(state, None).await;
    let permalink = if sha256.is_empty() {
        None
    } else {
        Some(gui_permalink(&sha256))
    };
    Ok(ScanEntry {
        report: ScanReport {
            version_id: version.id,
            sha256,
            verdict: Verdict::Unknown,
            stats: VtStats::default(),
            checked_at: db::db_time(db::now()),
            source: ScanSource::Api,
            permalink,
        },
        game_id: version.game_id,
        mode: ScanMode::Upload,
    })
}

fn vt_error_to_api(error: &VtError, message: &str) -> ApiError {
    match error {
        VtError::InvalidKey => ApiError::conflict("invalid_key", message),
        VtError::RateLimited { retry_after } => {
            let api = ApiError::too_many_requests("quota_exceeded", message);
            match retry_after {
                Some(seconds) => api.with_retry_after(*seconds),
                None => api,
            }
        }
        VtError::TooLarge { .. } => {
            ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "upload_too_large", message)
        }
        VtError::NotFound => ApiError::not_found("not_found", message),
        VtError::Upstream { .. } | VtError::Timeout | VtError::Transport(_) => {
            ApiError::new(StatusCode::BAD_GATEWAY, "virustotal_unavailable", message)
        }
    }
}

/// Lists the versions a scan targets.
async fn load_versions(
    state: &AppState,
    game_id: Option<i64>,
    version_id: Option<i64>,
) -> Result<Vec<VersionRow>, ApiError> {
    let mut filters = vec!["v.missing = 0", "v.root_unavailable = 0"];
    if game_id.is_some() {
        filters.push("v.game_id = ?");
    }
    if version_id.is_some() {
        filters.push("v.id = ?");
    }
    let sql = format!(
        "SELECT v.id, v.game_id, v.file_path, v.root_id,
                v.storage_backend, v.content_sha256
         FROM game_versions v
         WHERE {}
         ORDER BY v.id",
        filters.join(" AND ")
    );
    let mut query = db::query_as::<VersionRow>(&sql);
    if let Some(game_id) = game_id {
        query = query.bind(game_id);
    }
    if let Some(version_id) = version_id {
        query = query.bind(version_id);
    }
    query.fetch_all(&state.pool).await.map_err(ApiError::from)
}

/// Resolves the archive path under its library root. Never accepts a
/// caller-supplied path.
async fn local_path(state: &AppState, version: &VersionRow) -> Option<PathBuf> {
    if version.is_remote() {
        return None;
    }
    crate::roots::resolve_version_path(
        &state.pool,
        &state.config,
        version.root_id,
        &version.file_path,
    )
    .await
}

/// Executables/installers beside the archive, ranked and bounded.
///
/// The server does not extract archives itself; when the desktop already
/// extracted a game into the same folder, those binaries are what this hashes.
async fn binary_candidates(state: &AppState, version: &VersionRow) -> Vec<PathBuf> {
    let Some(archive) = local_path(state, version).await else {
        return Vec::new();
    };
    let Some(directory) = archive.parent().map(Path::to_path_buf) else {
        return Vec::new();
    };
    let mut entries = match tokio::fs::read_dir(&directory).await {
        Ok(entries) => entries,
        Err(error) => {
            tracing::debug!(version = version.id, %error, "could not list the archive directory");
            return Vec::new();
        }
    };
    let mut candidates: Vec<(bool, u64, PathBuf)> = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path == archive {
            continue;
        }
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !BINARY_EXTENSIONS.contains(&extension.as_str()) {
            continue;
        }
        let Ok(metadata) = entry.metadata().await else {
            continue;
        };
        if !metadata.is_file() || metadata.len() == 0 {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        candidates.push((ludex_core::is_installer(&name), metadata.len(), path));
    }
    // Installers first (the risky ones), then larger files.
    candidates.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.cmp(&left.1))
            .then_with(|| left.2.cmp(&right.2))
    });
    candidates.truncate(MAX_BINARY_CANDIDATES);
    candidates.into_iter().map(|(_, _, path)| path).collect()
}

async fn hash_binary(path: &Path) -> Option<String> {
    let owned = path.to_path_buf();
    let label = owned.display().to_string();
    match tokio::task::spawn_blocking(move || crate::content_hash::hash_file(&owned)).await {
        Ok(Ok((_, digest))) => Some(digest),
        Ok(Err(error)) => {
            tracing::debug!(path = %label, %error, "could not hash an executable");
            None
        }
        Err(error) => {
            tracing::debug!(path = %label, %error, "hashing an executable panicked");
            None
        }
    }
}

/// Cached reports for one game. Never calls VirusTotal: a player read must not
/// consume the admin quota.
pub async fn game_reports(state: &AppState, game_id: i64) -> Result<GameScanReport, ApiError> {
    let exists = db::query_scalar::<i64>("SELECT COUNT(*) FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)?;
    if exists == 0 {
        return Err(ApiError::not_found("game_not_found", "Game not found"));
    }
    let versions = db::query_as::<VersionRow>(
        "SELECT v.id, v.game_id, v.file_path, v.root_id,
                v.storage_backend, v.content_sha256
         FROM game_versions v
         WHERE v.game_id = ? AND v.missing = 0 AND v.root_unavailable = 0
         ORDER BY v.id",
    )
    .bind(game_id)
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let mut reports = Vec::new();
    for version in versions {
        let sha256 = match version.content_sha256.clone() {
            Some(sha256) if !sha256.is_empty() => Some(sha256),
            _ => db::query_scalar::<String>(
                "SELECT sha256 FROM virustotal_links
                 WHERE version_id = ? AND sha256 <> ''
                 ORDER BY id DESC LIMIT 1",
            )
            .bind(version.id)
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten(),
        };
        let Some(sha256) = sha256 else {
            continue;
        };
        if let Some(row) = load_cache(state, &sha256).await {
            reports.push(cache_report(version.id, &sha256, &row));
        }
    }
    // One report per version, in version order (the payload mirrors the
    // version list; the UI renders the rest as "not checked").
    reports.sort_by_key(|report| report.version_id);
    Ok(GameScanReport {
        game_id,
        versions: reports,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::mpsc;

    /// Evidence for `rust/cleartext-transmission` (#266): the production
    /// transport never keeps a non-loopback `http://` base, so the API key in
    /// `/users/{key}` cannot travel in clear text. Plaintext is allowed only for
    /// the loopback mock the integration tests run.
    #[test]
    fn plaintext_api_base_is_refused() {
        let transport = HttpTransport::new(
            reqwest::Client::new(),
            "http://virustotal.example.test/api/v3",
            Duration::from_secs(1),
        );
        assert_eq!(transport.base(), API_BASE);

        for plaintext in [
            "http://10.0.0.5/api/v3",
            "http://192.168.1.20:8080/api/v3",
            "http://[fe80::1]/api/v3",
        ] {
            let transport =
                HttpTransport::new(reqwest::Client::new(), plaintext, Duration::from_secs(1));
            assert_eq!(transport.base(), API_BASE, "{plaintext}");
        }

        let secure = HttpTransport::new(
            reqwest::Client::new(),
            "https://vt.example.test/api/v3/",
            Duration::from_secs(1),
        );
        assert_eq!(secure.base(), "https://vt.example.test/api/v3");

        // The mock runs on loopback and stays plaintext.
        let mock = HttpTransport::new(
            reqwest::Client::new(),
            "http://127.0.0.1:8123",
            Duration::from_secs(1),
        );
        assert_eq!(mock.base(), "http://127.0.0.1:8123");
    }

    /// In-memory transport recording calls and returning canned answers.
    struct MockTransport {
        lookups: StdMutex<Vec<String>>,
        uploads: AtomicUsize,
        validations: AtomicUsize,
        answer: StdMutex<MockAnswer>,
        validation: StdMutex<Result<(), VtError>>,
        delay_ms: std::sync::atomic::AtomicU64,
    }

    type MockAnswer = Box<dyn Fn(&str) -> Result<FileReport, VtError> + Send + Sync>;

    impl MockTransport {
        fn new(
            answer: impl Fn(&str) -> Result<FileReport, VtError> + Send + Sync + 'static,
        ) -> Self {
            Self {
                lookups: StdMutex::new(Vec::new()),
                uploads: AtomicUsize::new(0),
                validations: AtomicUsize::new(0),
                answer: StdMutex::new(Box::new(answer)),
                validation: StdMutex::new(Ok(())),
                delay_ms: std::sync::atomic::AtomicU64::new(0),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.lookups
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn set_delay(&self, delay: Duration) {
            self.delay_ms
                .store(delay.as_millis() as u64, Ordering::SeqCst);
        }
    }

    impl VtTransport for MockTransport {
        fn lookup_file<'a>(
            &'a self,
            _api_key: &'a str,
            sha256: &'a str,
        ) -> TransportFuture<'a, Result<FileReport, VtError>> {
            Box::pin(async move {
                self.lookups
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(sha256.to_string());
                if self.delay_ms.load(Ordering::SeqCst) != 0 {
                    tokio::time::sleep(Duration::from_millis(self.delay_ms.load(Ordering::SeqCst)))
                        .await;
                }
                let answer = self
                    .answer
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                answer(sha256)
            })
        }

        fn validate_key<'a>(
            &'a self,
            _api_key: &'a str,
        ) -> TransportFuture<'a, Result<(), VtError>> {
            Box::pin(async move {
                self.validations.fetch_add(1, Ordering::SeqCst);
                self.validation
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
            })
        }

        fn upload_file<'a>(
            &'a self,
            _api_key: &'a str,
            _path: &'a Path,
        ) -> TransportFuture<'a, Result<UploadOutcome, VtError>> {
            Box::pin(async move {
                self.uploads.fetch_add(1, Ordering::SeqCst);
                Ok(UploadOutcome {
                    analysis_id: Some("analysis-mock".to_string()),
                })
            })
        }
    }

    fn report(sha256: &str, malicious: i64, suspicious: i64) -> FileReport {
        let undetected = 70;
        FileReport {
            sha256: sha256.to_string(),
            size_bytes: Some(1_024),
            stats: VtStats {
                malicious,
                suspicious,
                undetected,
                harmless: 0,
                timeout: 0,
                failure: 0,
                total_engines: malicious + suspicious + undetected,
            },
            engine_hits: Vec::new(),
            last_analysis_date: Some("2026-01-01T00:00:00Z".to_string()),
            permalink: Some(gui_permalink(sha256)),
        }
    }

    async fn fixture(
        answer: impl Fn(&str) -> Result<FileReport, VtError> + Send + Sync + 'static,
    ) -> (AppState, Arc<MockTransport>) {
        let temp = tempfile::tempdir().expect("tempdir");
        let games = temp.path().join("games");
        let data = temp.path().join("data");
        let media = temp.path().join("media");
        for directory in [&games, &data, &media] {
            std::fs::create_dir_all(directory).expect("mkdir");
        }
        let mut config = crate::config::Config::for_local(games.clone(), data, media);
        config.virustotal_api_key = Some("test-key-abcdef".to_string());
        config.virustotal_per_minute = 60;
        let (tx, _rx) = mpsc::channel(8);
        let mut state = AppState::for_tests(config, tx).await;
        let transport = Arc::new(MockTransport::new(answer));
        state.virustotal = Arc::new(Scanner::with_transport(transport.clone()));
        // Keep the temp directory alive for the test's lifetime.
        std::mem::forget(temp);
        (state, transport)
    }

    async fn add_game(state: &AppState, sha256: Option<&str>) -> (i64, i64) {
        let now = db::db_time(db::now());
        let game_id = db::query(
            "INSERT INTO games (title, sort_title, created_at, updated_at) VALUES (?, ?, ?, ?)",
        )
        .bind("Celeste")
        .bind("celeste")
        .bind(&now)
        .bind(&now)
        .returning_id(&state.pool)
        .await
        .expect("game");
        let version_id = db::query(
            "INSERT INTO game_versions
                (game_id, file_path, file_name, size, missing, indexed_at, content_sha256)
             VALUES (?, ?, 'Celeste.zip', 4, 0, ?, ?)",
        )
        .bind(game_id)
        .bind("Celeste.zip")
        .bind(&now)
        .bind(sha256)
        .returning_id(&state.pool)
        .await
        .expect("version");
        (game_id, version_id)
    }

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn scan_request(version_id: i64) -> ScanRequest {
        ScanRequest {
            version_id: Some(version_id),
            ..ScanRequest::default()
        }
    }

    #[tokio::test]
    async fn verdict_mapping_never_turns_unknown_into_clean() {
        let (state, transport) = fixture(|sha| Ok(report(sha, 0, 0))).await;
        let (_, version) = add_game(&state, Some(SHA_A)).await;
        let entries = scan(&state, scan_request(version)).await.expect("scan");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].report.verdict, Verdict::Clean);
        assert_eq!(entries[0].report.source, ScanSource::Api);
        assert_eq!(transport.calls(), vec![SHA_A.to_string()]);

        // A 404 is not_found, never clean.
        let (state, _) = fixture(|_| Err(VtError::NotFound)).await;
        let (_, version) = add_game(&state, Some(SHA_B)).await;
        let entries = scan(&state, scan_request(version)).await.expect("scan");
        assert_eq!(entries[0].report.verdict, Verdict::NotFound);

        // An outage is unknown, never clean.
        let (state, _) = fixture(|_| {
            Err(VtError::Upstream {
                status: 503,
                message: "maintenance".to_string(),
            })
        })
        .await;
        let (_, version) = add_game(&state, Some(SHA_B)).await;
        let entries = scan(&state, scan_request(version)).await.expect("scan");
        assert_eq!(entries[0].report.verdict, Verdict::Unknown);

        // An invalid key is an explicit error, never clean.
        let (state, _) = fixture(|_| Err(VtError::InvalidKey)).await;
        let (_, version) = add_game(&state, Some(SHA_B)).await;
        let entries = scan(&state, scan_request(version)).await.expect("scan");
        assert_eq!(entries[0].report.verdict, Verdict::Error);
    }

    #[tokio::test]
    async fn malicious_wins_over_suspicious() {
        let (state, _) = fixture(|sha| Ok(report(sha, 3, 7))).await;
        let (_, version) = add_game(&state, Some(SHA_A)).await;
        let entries = scan(&state, scan_request(version)).await.expect("scan");
        assert_eq!(entries[0].report.verdict, Verdict::Malicious);
        assert_eq!(entries[0].report.stats.malicious, 3);
        assert_eq!(entries[0].report.stats.total_engines, 80);

        let (state, _) = fixture(|sha| Ok(report(sha, 0, 2))).await;
        let (_, version) = add_game(&state, Some(SHA_A)).await;
        let entries = scan(&state, scan_request(version)).await.expect("scan");
        assert_eq!(entries[0].report.verdict, Verdict::Suspicious);
    }

    #[tokio::test]
    async fn cache_hit_skips_the_api_and_expires() {
        let (state, transport) = fixture(|sha| Ok(report(sha, 0, 0))).await;
        let (_, version) = add_game(&state, Some(SHA_A)).await;
        scan(&state, scan_request(version))
            .await
            .expect("first scan");
        scan(&state, scan_request(version))
            .await
            .expect("second scan");
        assert_eq!(transport.calls().len(), 1, "second scan must hit the cache");

        // Expire the row and scan again: the API is called once more.
        db::query("UPDATE virustotal_cache SET checked_at = ?, ttl_seconds = 0 WHERE sha256 = ?")
            .bind(db::db_time(db::now() - ChronoDuration::days(8)))
            .bind(SHA_A)
            .execute(&state.pool)
            .await
            .expect("expire cache");
        scan(&state, scan_request(version))
            .await
            .expect("third scan");
        assert_eq!(transport.calls().len(), 2, "expired cache re-scans");

        // force bypasses a fresh cache.
        scan(
            &state,
            ScanRequest {
                force: true,
                ..scan_request(version)
            },
        )
        .await
        .expect("forced scan");
        assert_eq!(transport.calls().len(), 3);
    }

    #[tokio::test]
    async fn concurrent_scans_deduplicate_one_hash() {
        let (state, transport) = {
            let (state, transport) = fixture(|sha| Ok(report(sha, 0, 0))).await;
            transport.set_delay(Duration::from_millis(80));
            (state, transport)
        };
        let (_, version) = add_game(&state, Some(SHA_A)).await;
        let first = tokio::spawn({
            let state = state.clone();
            async move { scan(&state, scan_request(version)).await }
        });
        let second = tokio::spawn({
            let state = state.clone();
            async move { scan(&state, scan_request(version)).await }
        });
        first.await.expect("join").expect("first scan");
        second.await.expect("join").expect("second scan");
        assert_eq!(transport.calls(), vec![SHA_A.to_string()]);
    }

    #[tokio::test]
    async fn daily_budget_persists_across_a_restart() {
        let (state, _) = fixture(|sha| Ok(report(sha, 0, 0))).await;
        // The config is shared behind an Arc; rebuild one with a tiny budget.
        let mut config = (*state.config).clone();
        config.virustotal_daily_limit = 1;
        let state = AppState {
            config: Arc::new(config),
            ..state
        };
        let (_, version) = add_game(&state, Some(SHA_A)).await;
        scan(&state, scan_request(version)).await.expect("scan");
        assert_eq!(used_today(&state).await, 1);

        // A second server process (fresh scanner) reads the same database.
        let restarted = AppState {
            virustotal: Arc::new(Scanner::with_transport(Arc::new(MockTransport::new(
                |sha| Ok(report(sha, 0, 0)),
            )))),
            ..state
        };
        let (_, other) = add_game(&restarted, Some(SHA_B)).await;
        let entries = scan(&restarted, scan_request(other))
            .await
            .expect("scan after restart");
        assert_eq!(entries[0].report.verdict, Verdict::QuotaExceeded);
    }

    #[tokio::test]
    async fn upload_is_never_automatic_and_requires_consent() {
        let (state, transport) = fixture(|sha| Ok(report(sha, 0, 0))).await;
        let (_, version) = add_game(&state, Some(SHA_A)).await;
        scan(
            &state,
            ScanRequest {
                mode: Some("hash_and_binaries".to_string()),
                ..scan_request(version)
            },
        )
        .await
        .expect("hash scan");
        assert_eq!(
            transport.uploads.load(Ordering::SeqCst),
            0,
            "no auto-upload"
        );

        // Consent is mandatory.
        let refused = scan(
            &state,
            ScanRequest {
                mode: Some("upload".to_string()),
                ..scan_request(version)
            },
        )
        .await;
        assert!(refused.is_err());
        assert_eq!(transport.uploads.load(Ordering::SeqCst), 0);

        // The archive must exist on disk for an upload.
        let archive = state.config.games_dir.join("Celeste.zip");
        std::fs::write(&archive, b"ludex").expect("write archive");
        let entries = scan(
            &state,
            ScanRequest {
                mode: Some("upload".to_string()),
                confirm_upload: true,
                consent: Some(PUBLIC_SHARING_CONSENT.to_string()),
                ..scan_request(version)
            },
        )
        .await
        .expect("upload");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].report.verdict, Verdict::Unknown);
        assert_eq!(transport.uploads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn invalid_key_and_quota_are_distinguishable() {
        let (state, transport) = fixture(|sha| Ok(report(sha, 0, 0))).await;
        assert!(test_key(&state, None).await.expect("test").ok);
        assert_eq!(transport.validations.load(Ordering::SeqCst), 1);

        *transport
            .validation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Err(VtError::InvalidKey);
        let response = test_key(&state, None).await.expect("test");
        assert!(!response.ok);
        assert!(
            response.message.starts_with("invalid_key:"),
            "{}",
            response.message
        );
        let status = integration_status(&state).await;
        assert!(status
            .last_error
            .as_deref()
            .is_some_and(|error| error.starts_with("invalid_key:")));
    }

    #[tokio::test]
    async fn unsaved_key_can_be_tested_without_touching_the_stored_one() {
        let (state, transport) = fixture(|sha| Ok(report(sha, 0, 0))).await;
        let response = test_key(&state, Some("transient-key-9999"))
            .await
            .expect("transient test");
        assert!(response.ok);
        assert_eq!(transport.validations.load(Ordering::SeqCst), 1);

        // A rejected transient key is reported but never becomes lastError.
        *transport
            .validation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Err(VtError::InvalidKey);
        let response = test_key(&state, Some("transient-key-9999"))
            .await
            .expect("transient test");
        assert!(!response.ok);
        assert!(response.message.starts_with("invalid_key:"));
        let status = integration_status(&state).await;
        assert_eq!(status.source, KeySource::Env);
        assert_eq!(status.last_error, None);

        // Short keys are rejected before any request.
        let error = test_key(&state, Some("short"))
            .await
            .expect_err("short key");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(transport.validations.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cache_ttl_matrix_matches_the_contract() {
        assert_eq!(Verdict::Clean.ttl_seconds(), Some(7 * 24 * 3_600));
        assert_eq!(Verdict::NotFound.ttl_seconds(), Some(7 * 24 * 3_600));
        assert_eq!(Verdict::Suspicious.ttl_seconds(), Some(24 * 3_600));
        assert_eq!(
            Verdict::Malicious.ttl_seconds(),
            Some(MALICIOUS_TTL_SECONDS)
        );
        assert_eq!(Verdict::Unknown.ttl_seconds(), None);
        assert_eq!(Verdict::QuotaExceeded.ttl_seconds(), None);
    }

    #[tokio::test]
    async fn env_key_wins_and_is_masked() {
        let (state, _) = fixture(|sha| Ok(report(sha, 0, 0))).await;
        let status = integration_status(&state).await;
        assert_eq!(status.source, KeySource::Env);
        assert_eq!(status.fingerprint.as_deref(), Some("...cdef"));
        assert!(save_key(&state, "another-key").await.is_err());
        assert!(delete_key(&state).await.is_err());
    }

    #[tokio::test]
    async fn database_key_round_trips_encrypted() {
        let (mut state, _) = fixture(|sha| Ok(report(sha, 0, 0))).await;
        let mut config = (*state.config).clone();
        config.virustotal_api_key = None;
        state.config = Arc::new(config);
        let status = save_key(&state, "vt-secret-1234").await.expect("save");
        assert!(status.configured);
        assert_eq!(status.source, KeySource::Database);
        assert_eq!(status.fingerprint.as_deref(), Some("...1234"));

        let row: (String,) = db::query_as::<(String,)>(
            "SELECT api_key_enc FROM virustotal_integration WHERE id = 1",
        )
        .fetch_one(&state.pool)
        .await
        .expect("row");
        assert!(!row.0.contains("vt-secret-1234"), "key must be encrypted");
        assert!(row.0.starts_with("v1."));

        let resolved = key_state(&state).await;
        match resolved {
            KeyState::Key(key) => {
                assert_eq!(key.key, "vt-secret-1234");
                assert!(!format!("{key:?}").contains("vt-secret-1234"));
            }
            other => panic!("unexpected key state: {other:?}"),
        }

        let status = delete_key(&state).await.expect("delete");
        assert!(!status.configured);
        assert_eq!(status.source, KeySource::Missing);
    }
}
