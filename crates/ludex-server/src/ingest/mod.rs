//! Ingest manager: download game archives from URLs into the library.
//!
//! Port of the standalone `gv-downloader` engine: `.part` files with HTTP Range
//! resume, magic-byte format detection, unlimited retries with backoff, jobs
//! persisted across restarts and a library/stats view. The HTTP surface is
//! mounted by the server lane through [`router`]; [`resume_pending`] must be
//! called once at startup so interrupted downloads continue automatically.

mod manager;
pub mod types;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::{rejection::JsonRejection, Path as AxumPath, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::fs;
use tracing::info;

use self::manager::Inner;
use self::types::{
    infer_title, is_archive_name, parse_http_url, sanitize_title, CreateDownloadRequest,
    UpdateDownloadRequest, PART_SUFFIX,
};

pub use self::types::{IngestFile, IngestJob, IngestStats, IngestStatus, VerifyResponse};

/// Directories and channels the ingest manager needs from the server.
#[derive(Clone)]
pub struct IngestContext {
    /// GameVault-compatible library volume; finished archives land here.
    pub games_dir: PathBuf,
    /// Persistent state (`ingest-jobs.json`, `ingest-parts.json`).
    pub data_dir: PathBuf,
    /// Triggered (non-blocking) after every successful download.
    pub scan_tx: tokio::sync::mpsc::Sender<crate::scanner::ScanRequest>,
    /// Allow loopback/private targets (tests, LAN mirrors). Mirrors
    /// `LUDEX_INGEST_ALLOW_PRIVATE`.
    pub allow_private: bool,
    /// Event bus, used for audit and webhook events. `None` in unit tests.
    pub events: Option<crate::events::EventSink>,
}

/// Error payload: `{ "error": { "code": ..., "message": ... } }`.
#[derive(Debug, Clone, Serialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiErrorEnvelope {
    pub error: ApiError,
}

type ApiErrorResponse = (StatusCode, Json<ApiErrorEnvelope>);
type ApiResult<T> = Result<T, ApiErrorResponse>;

fn api_error(status: StatusCode, code: &str, message: impl Into<String>) -> ApiErrorResponse {
    (
        status,
        Json(ApiErrorEnvelope {
            error: ApiError {
                code: code.to_string(),
                message: message.into(),
            },
        }),
    )
}

fn not_found(message: &str) -> ApiErrorResponse {
    api_error(StatusCode::NOT_FOUND, "not_found", message)
}

fn json_rejection(rejection: JsonRejection) -> ApiErrorResponse {
    api_error(
        StatusCode::BAD_REQUEST,
        "invalid_body",
        rejection.body_text(),
    )
}

/// Counts of ingest jobs by lifecycle bucket.
#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestCounts {
    /// Jobs waiting for a download slot.
    pub queued: i64,
    /// Jobs downloading or verifying.
    pub running: i64,
    /// Jobs stopped by an error.
    pub failed: i64,
    /// Jobs completed successfully.
    pub done: i64,
    /// Jobs cancelled by an operator.
    pub cancelled: i64,
    /// All known jobs.
    pub total: i64,
}

/// Cloneable handle to the ingest manager, used by health and metrics.
#[derive(Clone)]
pub struct IngestHandle {
    inner: Arc<manager::Inner>,
}

impl IngestHandle {
    /// Job counts by lifecycle bucket.
    #[must_use]
    pub fn counts(&self) -> IngestCounts {
        self.inner.counts()
    }
}

/// Returns a handle to the ingest manager registered for `ctx.data_dir`.
#[must_use]
pub fn handle(ctx: &IngestContext) -> IngestHandle {
    IngestHandle {
        inner: manager::shared_state(ctx),
    }
}

/// Build the ingest routes (full paths under `/api/ingest`).
pub fn router(ctx: IngestContext) -> Router {
    let state = manager::shared_state(&ctx);
    Router::new()
        .route(
            "/api/ingest/downloads",
            get(list_downloads).post(create_download),
        )
        .route(
            "/api/ingest/downloads/{id}",
            get(get_download)
                .delete(cancel_download)
                .put(update_download),
        )
        .route("/api/ingest/downloads/{id}/retry", post(retry_download))
        .route("/api/ingest/files", get(list_files))
        .route("/api/ingest/files/{name}", delete(delete_file))
        .route("/api/ingest/files/{name}/verify", post(verify_file))
        .route("/api/ingest/stats", get(stats))
        .with_state(state)
}

/// Resume jobs that were queued or downloading when the server stopped.
///
/// Call once during startup, before accepting traffic. Safe to call twice.
pub async fn resume_pending(ctx: IngestContext) {
    let state = manager::shared_state(&ctx);
    manager::resume_pending(&state).await;
}

// ---------------------------------------------------------------------------
// Downloads
// ---------------------------------------------------------------------------

async fn list_downloads(State(state): State<Arc<Inner>>) -> Json<Vec<IngestJob>> {
    Json(state.jobs_sorted())
}

async fn create_download(
    State(state): State<Arc<Inner>>,
    payload: Result<Json<CreateDownloadRequest>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<IngestJob>)> {
    let Json(payload) = payload.map_err(json_rejection)?;
    let url = parse_http_url(&payload.url)
        .map_err(|message| api_error(StatusCode::BAD_REQUEST, "invalid_url", message))?;
    manager::reject_private_literal(&state, &url)
        .map_err(|message| api_error(StatusCode::BAD_REQUEST, "private_address", message))?;

    let title = payload
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(sanitize_title)
        .unwrap_or_else(|| infer_title(&url));

    let now = Utc::now();
    let job = IngestJob {
        id: uuid::Uuid::new_v4().to_string(),
        title,
        url: Some(url.to_string()),
        status: IngestStatus::Queued,
        file_name: None,
        downloaded_bytes: 0,
        total_bytes: None,
        speed_bps: 0,
        attempt: 0,
        error: None,
        created_at: now,
        updated_at: now,
    };
    state.insert_job(job.clone()).await;
    manager::ensure_dispatcher(&state);
    state.notify_queue();
    info!(job = %job.id, title = %job.title, "ingest download queued");
    Ok((StatusCode::CREATED, Json(job)))
}

async fn get_download(
    State(state): State<Arc<Inner>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<IngestJob>> {
    state
        .get_job(&id)
        .map(Json)
        .ok_or_else(|| not_found("Download not found"))
}

async fn cancel_download(
    State(state): State<Arc<Inner>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<StatusCode> {
    let Some(job) = state.get_job(&id) else {
        return Err(not_found("Download not found"));
    };
    if job.status == IngestStatus::Done {
        return Ok(StatusCode::NO_CONTENT);
    }

    let flag = state.cancel_flag(&id);
    flag.store(true, Ordering::SeqCst);
    state.wake_cancelled();
    if let Some(handle) = state.take_task(&id) {
        handle.abort();
        let _ = handle.await;
    }
    // An aborted task never reaches its own cleanup, so clear its slot here or
    // a later retry would never be dispatched.
    state.release_active(&id);
    manager::cancel_cleanup(&state, &id).await;
    info!(job = %id, "ingest download cancelled");
    Ok(StatusCode::NO_CONTENT)
}

async fn retry_download(
    State(state): State<Arc<Inner>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<IngestJob>> {
    let Some(job) = state.get_job(&id) else {
        return Err(not_found("Download not found"));
    };
    if !job.status.is_stopped() {
        return Err(api_error(
            StatusCode::CONFLICT,
            "conflict",
            "Only failed or cancelled downloads can be retried",
        ));
    }
    state.reset_cancel(&id);
    let _ = state.update_job(&id, |job| {
        job.status = IngestStatus::Queued;
        job.error = None;
        job.attempt = 0;
        job.speed_bps = 0;
        job.file_name = None;
    });
    state.persist().await;
    manager::ensure_dispatcher(&state);
    state.notify_queue();
    info!(job = %id, "ingest download requeued");
    let job = state
        .get_job(&id)
        .ok_or_else(|| not_found("Download not found"))?;
    Ok(Json(job))
}

async fn update_download(
    State(state): State<Arc<Inner>>,
    AxumPath(id): AxumPath<String>,
    payload: Result<Json<UpdateDownloadRequest>, JsonRejection>,
) -> ApiResult<Json<IngestJob>> {
    let Json(payload) = payload.map_err(json_rejection)?;
    let url = parse_http_url(&payload.url)
        .map_err(|message| api_error(StatusCode::BAD_REQUEST, "invalid_url", message))?;
    manager::reject_private_literal(&state, &url)
        .map_err(|message| api_error(StatusCode::BAD_REQUEST, "private_address", message))?;

    let Some(job) = state.get_job(&id) else {
        return Err(not_found("Download not found"));
    };
    let normalized = url.to_string();
    match job.status {
        IngestStatus::Error | IngestStatus::Cancelled => {
            state.reset_cancel(&id);
            let _ = state.update_job(&id, |job| {
                job.url = Some(normalized);
                job.status = IngestStatus::Queued;
                job.error = None;
                job.attempt = 0;
                job.speed_bps = 0;
                job.file_name = None;
            });
            state.persist().await;
            manager::ensure_dispatcher(&state);
            state.notify_queue();
            info!(job = %id, "download URL replaced; job requeued");
        }
        IngestStatus::Done => {
            let _ = state.update_job(&id, |job| {
                job.url = Some(normalized);
            });
            state.persist().await;
        }
        IngestStatus::Queued | IngestStatus::Downloading | IngestStatus::Verifying => {
            // The driver re-reads the URL before every attempt.
            let _ = state.update_job(&id, |job| {
                job.url = Some(normalized);
                job.error = Some("URL updated; it will be used on the next attempt".to_string());
            });
            state.persist().await;
        }
    }

    let job = state
        .get_job(&id)
        .ok_or_else(|| not_found("Download not found"))?;
    Ok(Json(job))
}

// ---------------------------------------------------------------------------
// Library files
// ---------------------------------------------------------------------------

async fn list_files(State(state): State<Arc<Inner>>) -> ApiResult<Json<Vec<IngestFile>>> {
    let mut entries = fs::read_dir(&state.ctx.games_dir).await.map_err(|error| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "io_error",
            format!("Could not list the games directory: {error}"),
        )
    })?;
    let mut files = Vec::new();
    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(error) => {
                return Err(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "io_error",
                    format!("Could not list the games directory: {error}"),
                ))
            }
        };
        let metadata = match entry.metadata().await {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if !metadata.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let partial = name.starts_with('.') && name.ends_with(PART_SUFFIX);
        if !partial && name.starts_with('.') {
            continue;
        }
        files.push(IngestFile {
            name,
            size: metadata.len(),
            partial,
            modified_at: metadata.modified().ok().map(DateTime::<Utc>::from),
        });
    }
    files.sort_by_key(|file| file.name.to_lowercase());
    Ok(Json(files))
}

async fn delete_file(
    State(state): State<Arc<Inner>>,
    AxumPath(name): AxumPath<String>,
) -> ApiResult<StatusCode> {
    let name = safe_file_name(&name)?;
    if state.is_part_in_use(name) {
        return Err(api_error(
            StatusCode::CONFLICT,
            "file_busy",
            "This file is currently being downloaded",
        ));
    }
    let path = state.ctx.games_dir.join(name);
    match fs::remove_file(&path).await {
        Ok(()) => {
            if let Some(events) = &state.ctx.events {
                events.emit(
                    crate::events::Event::new(
                        crate::audit::actions::GAME_DELETED,
                        crate::events::Actor::System,
                    )
                    .target("game_file", name)
                    .metadata(serde_json::json!({ "source": "ingest" })),
                );
            }
            info!(file = name, "ingest file deleted");
            Ok(StatusCode::NO_CONTENT)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(not_found("File not found"))
        }
        Err(error) => Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "io_error",
            format!("Could not delete the file: {error}"),
        )),
    }
}

async fn verify_file(
    State(state): State<Arc<Inner>>,
    AxumPath(name): AxumPath<String>,
) -> ApiResult<Json<VerifyResponse>> {
    let name = safe_file_name(&name)?;
    let path = state.ctx.games_dir.join(name);
    match fs::metadata(&path).await {
        Ok(metadata) if metadata.is_file() => {}
        _ => return Err(not_found("File not found")),
    }
    let label = name.to_string();
    // A full archive test can take minutes on a large game: never block the
    // async runtime on it.
    let result = tokio::task::spawn_blocking(move || ludex_core::verify_archive(&path, None)).await;
    match result {
        Ok(Ok(_)) => Ok(Json(VerifyResponse {
            ok: true,
            log: format!("{label}: archive verified successfully"),
        })),
        Ok(Err(error)) => Ok(Json(VerifyResponse {
            ok: false,
            log: truncate_log(format!("{label}: {error}")),
        })),
        Err(error) => Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "verify_failed",
            format!("Verification task failed: {error}"),
        )),
    }
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

async fn stats(State(state): State<Arc<Inner>>) -> Json<IngestStats> {
    let games_dir = state.ctx.games_dir.clone();
    let walk_dir = games_dir.clone();
    let (library_bytes, game_count) = tokio::task::spawn_blocking(move || walk_library(&walk_dir))
        .await
        .unwrap_or((0, 0));
    let disk_total = fs2::total_space(&games_dir).unwrap_or(0);
    let disk_free = fs2::available_space(&games_dir).unwrap_or(0);
    Json(IngestStats {
        games_dir: games_dir.display().to_string(),
        disk_total,
        disk_free,
        library_bytes,
        game_count,
        active_jobs: state.active_count(),
    })
}

fn walk_library(root: &Path) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut count = 0u64;
    for entry in walkdir::WalkDir::new(root)
        .max_depth(4)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if !is_archive_name(&name) {
            continue;
        }
        if let Ok(metadata) = entry.metadata() {
            bytes = bytes.saturating_add(metadata.len());
            count = count.saturating_add(1);
        }
    }
    (bytes, count)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Reject anything that is not a single, plain file name inside the games dir.
fn safe_file_name(name: &str) -> Result<&str, ApiErrorResponse> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.contains('\0')
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "invalid_file_name",
            "Invalid file name",
        ));
    }
    Ok(name)
}

fn truncate_log(message: String) -> String {
    const MAX_CHARS: usize = 4000;
    if message.chars().count() <= MAX_CHARS {
        return message;
    }
    let mut truncated: String = message.chars().take(MAX_CHARS).collect();
    truncated.push('…');
    truncated
}
