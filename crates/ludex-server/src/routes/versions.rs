//! Server-side extracted cache and file-manifest transport (#231).
//!
//! The endpoints below are the transport half of the prototype described in
//! GitHub issue #231. The server extracts an archive once (see
//! [`crate::extracted_cache`]), then serves a manifest and the individual files
//! so a client can install without downloading or extracting the archive.
//!
//! They are **off by default**: `LUDEX_EXTRACTED_CACHE=1` turns them on. While
//! disabled the extract route answers `unavailable` and the public routes answer
//! `409 extracted_cache_unavailable`, which is the client's signal to fall back
//! to the classic archive download. Nothing is written to disk while disabled.
//!
//! | Route | Auth |
//! |---|---|
//! | `POST /api/admin/versions/{versionId}/extract` | administrator |
//! | `GET /api/versions/{versionId}/manifest?token=` | download ticket or Bearer |
//! | `GET /api/versions/{versionId}/files/{*path}?token=` | download ticket or Bearer |
//! | `GET /api/admin/extracted-cache` | administrator |
//! | `DELETE /api/admin/extracted-cache/{sha256}` | administrator |
//!
//! The public routes take the same download ticket as `/download` and
//! `/stream/manifest` (#115, #216): `POST /api/games/{id}/download-ticket`
//! returns a 15-minute JWT bound to the game and version, so a desktop client
//! can hand the URLs to a parallel worker without sharing the session token.
//! A Bearer token is accepted as well and goes through the same age gate as a
//! download.

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
use utoipa::ToSchema;

use crate::auth::{AuthUser, DownloadTicket};
use crate::db;
use crate::download_limits::{self, DownloadAdmission};
use crate::error::ApiError;
use crate::extract::{ApiPath, ApiQuery};
use crate::extracted_cache::{
    self, EntryLease, ExtractState, ExtractStatus, ExtractedCache, ExtractedFile,
    ExtractedManifest, Submission,
};
use crate::models::VersionRow;
use crate::routes::games::{age_blocked, age_not_found, deny_age_restricted, game_age_rating};
use crate::state::AppState;
use crate::transfer::zip::{resolve_entry_range, EntryRange};

/// Response header carrying the served file's SHA-256.
pub const FILE_SHA256_HEADER: &str = "x-ludex-file-sha256";
/// Chunk size of the file response body.
const FILE_CHUNK_BYTES: usize = 64 * 1024;

/// Extracted cache and file-manifest routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/versions/{version_id}/manifest", get(manifest))
        .route("/versions/{version_id}/files/{*path}", get(file))
        .route("/admin/versions/{version_id}/extract", post(extract))
        .route("/admin/extracted-cache", get(cache_overview))
        .route("/admin/extracted-cache/{sha256}", delete(remove))
}

/// `GET /api/versions/{versionId}/manifest?token=`
///
/// The manifest is derived from the archive digest, so it never depends on the
/// client. `409 extracted_cache_unavailable` means "not ready, or disabled";
/// the client falls back to the classic download rather than looping.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ManifestResponse {
    /// Version the manifest belongs to.
    pub version_id: i64,
    /// Archive content SHA-256 the cache entry is keyed by.
    pub archive_sha256: String,
    /// Always `ready`; the endpoint never returns a partial manifest.
    pub status: String,
    /// Extraction timestamp.
    pub generated_at: DateTime<Utc>,
    /// Sum of the file sizes.
    pub total_bytes: u64,
    /// Number of files.
    pub file_count: usize,
    /// Files sorted by path, with per-file SHA-256 and mode.
    pub files: Vec<ExtractedFile>,
    /// Directories that hold no file (empty directories to recreate).
    pub directories: Vec<String>,
}

async fn manifest(
    State(state): State<AppState>,
    ApiPath(version_id): ApiPath<i64>,
    ApiQuery(query): ApiQuery<VersionQuery>,
    headers: HeaderMap,
) -> Result<Json<ManifestResponse>, ApiError> {
    let access = authorize(&state, &headers, version_id, query.token.as_deref()).await?;
    let sha256 = ready_sha(&state, &access.version)?;
    let Some(lease) = state.transfer.extracted.acquire(sha256) else {
        return Err(extracted_unavailable());
    };
    lease.touch();
    let manifest = lease.manifest();
    Ok(Json(ManifestResponse {
        version_id: access.version.id,
        archive_sha256: sha256.to_string(),
        status: ExtractState::Ready.as_str().to_string(),
        generated_at: manifest.generated_at,
        total_bytes: manifest.total_bytes,
        file_count: manifest.file_count,
        files: manifest.files.clone(),
        directories: manifest.directories.clone(),
    }))
}

/// `GET /api/versions/{versionId}/files/{*path}?token=`
///
/// One file of the extracted tree. Single `Range` support with the same
/// `If-Range` rule as the archive download (a stale validator restarts at
/// byte 0), an exact `Content-Length` and `X-Ludex-File-Sha256`. The path is
/// resolved against the entry manifest and canonicalised under the cache root:
/// `..`, empty segments, backslashes, absolute paths and symlinks are refused
/// with `404 file_not_found`, so a cache entry can never serve outside its own
/// tree.
async fn file(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    ApiPath((version_id, requested)): ApiPath<(i64, String)>,
    ApiQuery(query): ApiQuery<FileQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _download = state.ops.metrics.download_started();
    let access = authorize(&state, &headers, version_id, query.token.as_deref()).await?;
    let sha256 = ready_sha(&state, &access.version)?.to_string();
    let Some(lease) = state.transfer.extracted.acquire(&sha256) else {
        return Err(extracted_unavailable());
    };
    lease.touch();
    let _lease = state.transfer.transfers.begin(access.version.id);

    let job_id = job_id(&headers, &query)?;
    let admission = state
        .transfer
        .download_limits
        .admit_stream(peer.ip(), access.version.id, job_id.as_deref())
        .map_err(limited)?;

    let manifest = lease.manifest();
    let (entry, absolute) =
        resolve_file(&state.transfer.extracted, &sha256, &manifest, &requested)?;
    let size = entry.size;
    let etag = format!("\"{}\"", entry.sha256);

    let requested_range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .filter(|_| if_range_matches(&headers, &etag));
    let range = match requested_range {
        Some(raw) => match resolve_entry_range(Some(raw), size) {
            Ok(range) => range,
            Err(()) => return Ok(range_not_satisfiable(size)),
        },
        None => EntryRange {
            start: 0,
            length: size,
            partial: false,
        },
    };

    let mut reader = tokio::fs::File::open(&absolute).await.map_err(|error| {
        tracing::warn!(%error, path = %entry.path, "cached file disappeared");
        ApiError::not_found("file_not_found", "This file is not in the extracted cache")
    })?;
    if range.start > 0 {
        reader
            .seek(std::io::SeekFrom::Start(range.start))
            .await
            .map_err(|_| ApiError::internal())?;
    }
    let body = ReaderStream::with_capacity(reader.take(range.length), FILE_CHUNK_BYTES);

    let mut builder = Response::builder()
        .status(if range.partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, content_type(&entry.path))
        .header(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"))
        .header(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        )
        .header(header::CONTENT_LENGTH, range.length)
        .header(header::ETAG, etag)
        .header(
            HeaderName::from_static(FILE_SHA256_HEADER),
            entry.sha256.clone(),
        )
        .header(
            HeaderName::from_static(crate::content_hash::ARCHIVE_SHA256_HEADER),
            sha256,
        );
    if range.partial {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!(
                "bytes {}-{}/{}",
                range.start,
                range.start + range.length - 1,
                size
            ),
        );
    }
    if let Some(disposition) = content_disposition(&entry.path) {
        builder = builder.header(header::CONTENT_DISPOSITION, disposition);
    }
    let stream = HeldStream {
        inner: Box::pin(body),
        _lease: lease,
        _admission: admission,
    };
    builder
        .body(Body::from_stream(stream))
        .map_err(|_| ApiError::internal())
}

/// `POST /api/admin/versions/{versionId}/extract`
///
/// Starts (or joins) the extraction of one archive into the server cache and
/// reports its state. Concurrent calls for the same archive content share one
/// job, so a retry loop cannot extract the same 40 GB twice.
async fn extract(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    user: AuthUser,
    ApiPath(version_id): ApiPath<i64>,
) -> Result<Json<ExtractStatus>, ApiError> {
    user.require_admin()?;
    let version = fetch_version(&state, version_id).await?;
    if !state.transfer.extracted.enabled() {
        return Ok(Json(ExtractStatus {
            version_id,
            archive_sha256: version.content_sha256.clone().unwrap_or_default(),
            status: ExtractState::Unavailable,
            files: 0,
            bytes: 0,
            error: Some("extracted cache is disabled (LUDEX_EXTRACTED_CACHE=0)".to_string()),
        }));
    }
    if version.missing != 0 {
        return Err(ApiError::not_found(
            "file_not_found",
            "This version is no longer available",
        ));
    }
    let sha256 = version
        .content_sha256
        .as_deref()
        .filter(|value| extracted_cache::is_sha256_hex(value))
        .ok_or_else(|| {
            ApiError::conflict(
                "content_hash_unavailable",
                "Run the archive hash job first: no content SHA-256 is stored for this version",
            )
        })?
        .to_string();

    match state.transfer.extracted.submit(&sha256, version_id) {
        Submission::Ready(status) | Submission::Running(status) => Ok(Json(status)),
        Submission::Started(job) => {
            let status = ExtractStatus {
                version_id,
                archive_sha256: sha256.clone(),
                status: ExtractState::Queued,
                files: 0,
                bytes: 0,
                error: None,
            };
            extracted_cache::spawn_extract(
                state.clone(),
                version,
                sha256,
                job,
                user.actor(),
                Some(peer.ip()),
            );
            Ok(Json(status))
        }
    }
}

/// `GET /api/admin/extracted-cache`
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CacheOverview {
    /// Whether the feature is on.
    pub enabled: bool,
    /// Cache root on the server filesystem.
    pub root: String,
    /// Configured byte budget.
    pub budget_bytes: u64,
    /// Bytes used by ready entries.
    pub used_bytes: u64,
    /// Ready entries, oldest first.
    pub entries: Vec<CacheEntryView>,
}

/// One entry of the admin cache listing.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CacheEntryView {
    /// Archive content SHA-256.
    pub sha256: String,
    /// Version that first extracted this archive.
    pub version_id: i64,
    /// Extracted size in bytes.
    pub bytes: u64,
    /// Number of files.
    pub files: usize,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last use.
    pub last_used_at: DateTime<Utc>,
}

async fn cache_overview(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<CacheOverview>, ApiError> {
    user.require_admin()?;
    let entries = state
        .transfer
        .extracted
        .entries()
        .into_iter()
        .map(|entry| CacheEntryView {
            sha256: entry.sha256,
            version_id: entry.version_id,
            bytes: entry.bytes,
            files: entry.files,
            created_at: entry.created_at,
            last_used_at: entry.last_used_at,
        })
        .collect();
    Ok(Json(CacheOverview {
        enabled: state.transfer.extracted.enabled(),
        root: state.transfer.extracted.root().display().to_string(),
        budget_bytes: state.transfer.extracted.budget_bytes(),
        used_bytes: state.transfer.extracted.used_bytes(),
        entries,
    }))
}

/// `DELETE /api/admin/extracted-cache/{sha256}`
///
/// Removing an entry that is currently being served is refused with
/// `409 cache_entry_in_use`; the same entry is deleted by the LRU eviction once
/// the last response ends.
async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(sha256): ApiPath<String>,
) -> Result<StatusCode, ApiError> {
    user.require_admin()?;
    let sha256 = sha256.to_ascii_lowercase();
    if !extracted_cache::is_sha256_hex(&sha256) {
        return Err(cache_entry_not_found());
    }
    let cache = state.transfer.extracted.clone();
    let key = sha256.clone();
    let removed = tokio::task::spawn_blocking(move || cache.remove(&key))
        .await
        .map_err(|_| ApiError::internal())?;
    if removed {
        Ok(StatusCode::NO_CONTENT)
    } else if state.transfer.extracted.has_entry(&sha256)
        || state.transfer.extracted.has_job(&sha256)
    {
        Err(ApiError::conflict(
            "cache_entry_in_use",
            "The entry is being served or extracted; retry once the transfer ends",
        ))
    } else {
        Err(cache_entry_not_found())
    }
}

fn cache_entry_not_found() -> ApiError {
    ApiError::not_found(
        "cache_entry_not_found",
        "No extracted cache entry has this digest",
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionQuery {
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileQuery {
    token: Option<String>,
    job_id: Option<String>,
}

struct VersionAccess {
    version: VersionRow,
}

/// Ticket-or-Bearer authentication for the version-scoped routes.
async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    version_id: i64,
    ticket: Option<&str>,
) -> Result<VersionAccess, ApiError> {
    let version = fetch_version(state, version_id).await?;
    if version.missing != 0 {
        return Err(ApiError::not_found(
            "file_not_found",
            "This version is no longer available",
        ));
    }
    let game_id = version_game(state, version.id).await?;
    let Some(ticket) = ticket else {
        let token = crate::auth::bearer_token(headers).ok_or_else(ApiError::unauthorized)?;
        let user = crate::auth::authenticate_token(state, &token).await?;
        if age_blocked(user.age_limit, game_age_rating(state, game_id).await?) {
            deny_age_restricted(state, &user, game_id);
            return Err(age_not_found());
        }
        return Ok(VersionAccess { version });
    };
    let decoded: DownloadTicket =
        crate::auth::decode_download_ticket(&state.auth.jwt_secret, ticket)
            .ok_or_else(ApiError::unauthorized)?;
    if decoded.version_id != version.id || decoded.game_id != game_id {
        return Err(ApiError::forbidden(
            "ticket_mismatch",
            "Ticket does not match this version",
        ));
    }
    Ok(VersionAccess { version })
}

/// Cache key of a version, or `409 extracted_cache_unavailable`.
fn ready_sha<'a>(state: &AppState, version: &'a VersionRow) -> Result<&'a str, ApiError> {
    if !state.transfer.extracted.enabled() {
        return Err(extracted_unavailable());
    }
    version
        .content_sha256
        .as_deref()
        .filter(|value| extracted_cache::is_sha256_hex(value))
        .ok_or_else(extracted_unavailable)
}

fn extracted_unavailable() -> ApiError {
    ApiError::conflict(
        "extracted_cache_unavailable",
        "The extracted cache is disabled or this archive has not been extracted yet; use the classic download",
    )
}

fn limited(rejection: download_limits::LimitRejection) -> ApiError {
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "too_many_download_attempts",
        rejection.message(),
    )
    .with_retry_after(rejection.retry_after)
}

fn job_id(headers: &HeaderMap, query: &FileQuery) -> Result<Option<String>, ApiError> {
    let raw = match headers.get(download_limits::JOB_ID_HEADER) {
        Some(value) => Some(value.to_str().map_err(|_| invalid_job_id())?),
        None => query.job_id.as_deref(),
    };
    download_limits::parse_job_id(raw).map_err(|_| invalid_job_id())
}

fn invalid_job_id() -> ApiError {
    ApiError::bad_request(
        "invalid_job_id",
        "Job id must be 1-64 characters of A-Z, a-z, 0-9, '-' or '_'",
    )
}

/// `416` with the contract's `Content-Range: bytes */<size>`.
fn range_not_satisfiable(size: u64) -> Response {
    let mut response = ApiError::new(
        StatusCode::RANGE_NOT_SATISFIABLE,
        "invalid_range",
        "The requested byte range cannot be satisfied",
    )
    .into_response();
    if let Ok(value) = HeaderValue::from_str(&format!("bytes */{size}")) {
        response.headers_mut().insert(header::CONTENT_RANGE, value);
    }
    response
        .headers_mut()
        .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response
}

/// Whether a client `Range` may be honoured under `If-Range`.
///
/// Only a strong entity-tag equal to the file digest matches. A missing
/// `If-Range` keeps the plain `Range` behaviour; a stale date or digest means
/// the file changed and the whole file is re-sent instead of splicing two
/// different contents.
fn if_range_matches(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_RANGE)
        .and_then(|value| value.to_str().ok())
        .is_none_or(|value| value.trim() == etag)
}

/// Resolves a requested path to a manifest entry and a canonical file.
fn resolve_file(
    cache: &ExtractedCache,
    sha256: &str,
    manifest: &ExtractedManifest,
    requested: &str,
) -> Result<(ExtractedFile, PathBuf), ApiError> {
    let requested = requested.trim_start_matches('/');
    if requested.is_empty() || !safe_relative(requested) {
        return Err(file_not_found());
    }
    let index = manifest
        .files
        .binary_search_by(|candidate| candidate.path.as_str().cmp(requested))
        .map_err(|_| file_not_found())?;
    let entry = manifest.files[index].clone();

    let root = cache.files_dir(sha256);
    let candidate = root.join(&entry.path);
    let canonical = std::fs::canonicalize(&candidate).map_err(|_| file_not_found())?;
    let canonical_root = std::fs::canonicalize(&root).map_err(|_| file_not_found())?;
    if canonical != candidate || !canonical.starts_with(&canonical_root) {
        // A symlink anywhere in the path, or an escape from the cache root.
        return Err(file_not_found());
    }
    let metadata = std::fs::symlink_metadata(&canonical).map_err(|_| file_not_found())?;
    if !metadata.is_file() {
        return Err(file_not_found());
    }
    Ok((entry, canonical))
}

/// Whether a relative path has no `..`, empty, backslash or NUL segment.
fn safe_relative(path: &str) -> bool {
    !path.contains('\\')
        && !path.contains('\0')
        && path.split('/').all(|segment| {
            !segment.is_empty() && segment != "." && segment != ".." && !segment.contains(':')
        })
}

fn file_not_found() -> ApiError {
    ApiError::not_found("file_not_found", "This file is not in the extracted cache")
}

fn content_type(path: &str) -> HeaderValue {
    mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string()
        .parse()
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"))
}

fn content_disposition(path: &str) -> Option<HeaderValue> {
    let name = path.rsplit('/').next()?.replace('"', "_");
    HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")).ok()
}

/// Response body that keeps the cache lease and the #217 part permit alive
/// until the last byte was sent.
struct HeldStream {
    inner: Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send>>,
    _lease: EntryLease,
    _admission: DownloadAdmission,
}

impl Stream for HeldStream {
    type Item = io::Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(context)
    }
}

/// Reads one version row by id.
pub(crate) async fn fetch_version(
    state: &AppState,
    version_id: i64,
) -> Result<VersionRow, ApiError> {
    db::query_as::<VersionRow>(
        "SELECT id, root_id, file_path, file_name, size, version, release_date, early_access, \
         missing, root_unavailable, archive_hash, content_sha256, indexed_at, updated_at, \
         storage_backend, object_key, etag FROM game_versions WHERE id = ?",
    )
    .bind(version_id)
    .fetch_optional(&state.core.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("version_not_found", "Game version not found"))
}

async fn version_game(state: &AppState, version_id: i64) -> Result<i64, ApiError> {
    db::query_scalar::<i64>("SELECT game_id FROM game_versions WHERE id = ?")
        .bind(version_id)
        .fetch_optional(&state.core.pool)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("version_not_found", "Game version not found"))
}

/// Deletes the cache entry of a version that is being removed.
///
/// Called by `DELETE /api/games/{id}/versions/{versionId}` after the row is
/// gone: an archive digest that another version still references is kept.
pub(crate) async fn forget_version_cache(
    state: &AppState,
    sha256: &str,
    archive_hash: Option<&str>,
) {
    let others = db::query_scalar::<i64>(
        "SELECT COUNT(*) FROM game_versions WHERE content_sha256 = ?          OR (content_sha256 IS NULL AND archive_hash = ?)",
    )
    .bind(sha256)
    .bind(archive_hash)
    .fetch_one(&state.core.pool)
    .await;
    match others {
        // Another version still needs the same bytes, or the count is
        // unknown: keep the entry (the LRU/TTL sweep owns it then). A row
        // whose full hash is still pending is covered by the cheap sample.
        Ok(0) => {}
        _ => return,
    }
    let cache = state.transfer.extracted.clone();
    let key = sha256.to_string();
    let removed = tokio::task::spawn_blocking(move || cache.forget(&key))
        .await
        .unwrap_or(false);
    if removed {
        tracing::info!(
            sha256,
            "deleted the extracted cache entry of a removed version"
        );
    }
}
