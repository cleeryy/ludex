//! Resumable per-entry stream installs (#115).
//!
//! The server exposes the ZIP central directory as a manifest
//! (`/stream/manifest`) plus one ranged endpoint per entry
//! (`/stream/entries/{index}`). This module downloads entries two at a time,
//! writes them straight into the staging folder (no archive ever reaches the
//! disk), verifies sizes and CRC-32, and records completed indexes in the
//! install job. A dropped connection, a pause or an app restart resumes at
//! entry granularity and a partial entry resumes with HTTP Range.
//!
//! Formats without random access (7z, RAR) stay on the sequential tar stream
//! and the normal resumable download.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use ludex_core::archive::{StreamEntry, StreamManifest};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::db;
use crate::install::{
    persist, safe_tar_path, Failure, InstallParams, JobPaths, StreamOutcome, Throttle,
};
use crate::state::{AppState, JobEntry};
use crate::util::now_rfc3339;

/// Number of entries downloaded concurrently. Two keeps a single HTTP request
/// from wasting the link on a slow start without turning many small files into
/// a thousand-connection storm.
const ENTRY_CONCURRENCY: usize = 2;

/// Folder inside staging that holds in-flight `.part` entry files. Removed
/// before the staging folder is moved into place.
const PARTIAL_DIR: &str = ".ludex-partial";

/// How long an entry response may go without a single chunk before the client
/// considers the connection dead (#204). A long decompress-and-skip can look
/// like a network drop: the request is retried with a `Range` header, so the
/// retry resumes at the exact byte instead of restarting the job.
const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(45);
/// Bounded attempts per entry. Five covers a proxy 502/504 burst without
/// looping forever; the job stays resumable when they are exhausted.
const DEFAULT_ENTRY_ATTEMPTS: u32 = 5;
/// First backoff; doubles per attempt up to eight seconds.
const RETRY_BACKOFF: Duration = Duration::from_millis(500);

/// Retry policy of the per-entry stream (#204).
#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamTuning {
    pub stall_timeout: Duration,
    pub attempts: u32,
    pub backoff: Duration,
}

impl Default for StreamTuning {
    fn default() -> Self {
        Self {
            stall_timeout: DEFAULT_STALL_TIMEOUT,
            attempts: DEFAULT_ENTRY_ATTEMPTS,
            backoff: RETRY_BACKOFF,
        }
    }
}

impl StreamTuning {
    /// Delay before attempt `failed + 1`, capped at eight times the base.
    fn backoff(&self, failed: u32) -> Duration {
        let factor = 1_u32 << failed.saturating_sub(1).min(3);
        self.backoff.saturating_mul(factor)
    }
}

/// Body chunks of one entry response, already mapped to owned bytes so a test
/// transport can inject stalls and mid-body disconnects (#204).
type EntryChunks = Pin<Box<dyn futures_util::Stream<Item = Result<Vec<u8>, String>> + Send>>;

/// One entry response as the download loop sees it.
pub(crate) struct EntryResponse {
    pub status: reqwest::StatusCode,
    /// Start offset of a `206` response, parsed from `Content-Range`.
    pub content_range_start: Option<u64>,
    pub body: EntryChunks,
}

/// Injectable HTTP transport for the per-entry stream.
///
/// Production uses [`HttpTransport`] (reqwest). Tests implement this trait to
/// close a body mid-entry, stall without sending a chunk, or answer 502/504
/// without a socket, and then assert the next request resumes at the exact
/// offset.
pub(crate) trait EntryTransport: Send + Sync {
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        token: Option<&'a str>,
        offset: u64,
    ) -> Pin<Box<dyn Future<Output = Result<EntryResponse, String>> + Send + 'a>>;
}

pub(crate) struct HttpTransport<'a> {
    pub client: &'a reqwest::Client,
}

impl EntryTransport for HttpTransport<'_> {
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        token: Option<&'a str>,
        offset: u64,
    ) -> Pin<Box<dyn Future<Output = Result<EntryResponse, String>> + Send + 'a>> {
        Box::pin(async move {
            let mut request = self
                .client
                .get(url)
                .header(reqwest::header::ACCEPT_ENCODING, "identity");
            if let Some(token) = token {
                request = request.bearer_auth(token);
            }
            if offset > 0 {
                request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
            }
            let response = request.send().await.map_err(|e| e.to_string())?;
            let status = response.status();
            let content_range_start = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_content_range_start);
            let body = response
                .bytes_stream()
                .map(|item| item.map(|bytes| bytes.to_vec()).map_err(|e| e.to_string()));
            Ok(EntryResponse {
                status,
                content_range_start,
                body: Box::pin(body),
            })
        })
    }
}

/// `bytes 100-199/200` → `Some(100)`.
fn parse_content_range_start(value: &str) -> Option<u64> {
    let rest = value.trim().strip_prefix("bytes")?.trim_start();
    let (start, _) = rest.split_once('-')?;
    start.trim().parse().ok()
}

/// How a stream job treats a previous attempt (#203).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckpointDecision {
    /// No previous attempt, or its staging folder is gone.
    Fresh,
    /// Same archive and the staging folder is on disk: adopt the checkpoint.
    Adopted,
    /// A previous attempt exists but the served archive changed: restart
    /// explicitly, never mix old and new files.
    ArchiveChanged,
}

pub(crate) struct CheckpointPlan {
    pub decision: CheckpointDecision,
    pub staging: PathBuf,
    pub stale_staging: Option<PathBuf>,
}

/// Decides whether a stream job adopts its previous checkpoint.
///
/// The manifest fingerprint is a SHA-256 of the entry list, so a rebuilt or
/// replaced archive under the same version row restarts from zero instead of
/// trusting files that no longer match the server.
pub(crate) fn plan_checkpoint(
    previous: Option<(&str, &Path)>,
    fingerprint: &str,
    staging_exists: bool,
    default_staging: &Path,
) -> CheckpointPlan {
    match previous {
        Some((stored_fingerprint, stored_staging))
            if stored_fingerprint == fingerprint && staging_exists =>
        {
            CheckpointPlan {
                decision: CheckpointDecision::Adopted,
                staging: stored_staging.to_path_buf(),
                stale_staging: None,
            }
        }
        Some((_, stored_staging)) if staging_exists => CheckpointPlan {
            decision: CheckpointDecision::ArchiveChanged,
            staging: default_staging.to_path_buf(),
            stale_staging: Some(stored_staging.to_path_buf()),
        },
        _ => CheckpointPlan {
            decision: CheckpointDecision::Fresh,
            staging: default_staging.to_path_buf(),
            stale_staging: None,
        },
    }
}

/// Hex-encoded bitset of completed entry indexes.
///
/// One bit per manifest entry keeps the per-entry checkpoint a few kilobytes
/// even for archives with 100k entries, so rewriting it after each file is
/// cheap.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CompletedEntries {
    bits: Vec<u8>,
}

impl CompletedEntries {
    pub(crate) fn new(count: usize) -> Self {
        Self {
            bits: vec![0; count.div_ceil(8)],
        }
    }

    pub(crate) fn from_hex(value: &str) -> Self {
        Self {
            bits: hex::decode(value.trim()).unwrap_or_default(),
        }
    }

    pub(crate) fn to_hex(&self) -> String {
        hex::encode(&self.bits)
    }

    pub(crate) fn resize(&mut self, count: usize) {
        self.bits.resize(count.div_ceil(8), 0);
    }

    pub(crate) fn contains(&self, index: usize) -> bool {
        self.bits
            .get(index / 8)
            .is_some_and(|byte| byte & (1 << (index % 8)) != 0)
    }

    pub(crate) fn insert(&mut self, index: usize) {
        if let Some(byte) = self.bits.get_mut(index / 8) {
            *byte |= 1 << (index % 8);
        }
    }

    pub(crate) fn remove(&mut self, index: usize) {
        if let Some(byte) = self.bits.get_mut(index / 8) {
            *byte &= !(1 << (index % 8));
        }
    }

    pub(crate) fn count(&self) -> usize {
        self.bits
            .iter()
            .map(|byte| byte.count_ones() as usize)
            .sum()
    }
}

/// Live byte/entry counters shared by the concurrent entry downloads.
struct ProgressState {
    bytes: AtomicU64,
    entries_done: AtomicU64,
    current: Mutex<String>,
}

impl ProgressState {
    fn new(bytes: u64, entries_done: u64) -> Self {
        Self {
            bytes: AtomicU64::new(bytes),
            entries_done: AtomicU64::new(entries_done),
            current: Mutex::new(String::new()),
        }
    }

    fn subtract_bytes(&self, amount: u64) {
        let _ = self
            .bytes
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_sub(amount))
            });
    }

    fn set_current(&self, path: &str) {
        if let Ok(mut current) = self.current.lock() {
            current.clear();
            current.push_str(path);
        }
    }

    fn current(&self) -> String {
        self.current
            .lock()
            .map(|current| current.clone())
            .unwrap_or_default()
    }
}

/// Downloads and installs a ZIP per entry, resuming a persisted checkpoint.
///
/// Returns [`StreamOutcome::Unsupported`] when the server does not expose the
/// per-entry protocol (404/405/415, an old build, or a malformed manifest):
/// the caller then falls back to the sequential tar stream and finally to the
/// normal resumable download.
pub(crate) async fn install(
    app: &AppHandle,
    entry: &Arc<JobEntry>,
    params: &InstallParams,
    default_staging: &Path,
    job_paths: &mut JobPaths,
) -> Result<StreamOutcome, Failure> {
    let state = app.state::<AppState>();
    let base = params.server_url.trim_end_matches('/');
    if base.is_empty() {
        return Ok(StreamOutcome::Unsupported(reqwest::StatusCode::NOT_FOUND));
    }

    let manifest = match fetch_manifest(&state, base, params).await {
        Ok(manifest) => manifest,
        Err(status) => return Ok(StreamOutcome::Unsupported(status)),
    };
    if manifest.format != "zip" || manifest.entry_count == 0 {
        return Ok(StreamOutcome::Unsupported(
            reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ));
    }

    // Validate every path before touching the disk: a malicious or buggy
    // manifest must not escape staging.
    let mut files: Vec<StreamEntry> = Vec::new();
    for item in &manifest.entries {
        let relative = safe_tar_path(Path::new(&item.path)).ok_or_else(|| {
            Failure::Failed(format!(
                "The server sent an unsafe entry path: {}",
                item.path
            ))
        })?;
        if relative
            .components()
            .next()
            .and_then(|component| component.as_os_str().to_str())
            == Some(PARTIAL_DIR)
        {
            return Err(Failure::Failed(format!(
                "The server sent a reserved entry path: {}",
                item.path
            )));
        }
        if !item.is_dir {
            files.push(item.clone());
        }
    }

    let fingerprint = manifest.fingerprint();
    let total_bytes: u64 = manifest
        .entries
        .iter()
        .filter(|item| !item.is_dir)
        .fold(0_u64, |total, item| total.saturating_add(item.size));

    // Adopt the checkpoint of the previous attempt of this version when the
    // archive is byte-identical (same manifest fingerprint) and its staging
    // folder is still on disk (#115/#203). A changed archive restarts
    // explicitly: old and new files are never mixed.
    let (mut completed, staging, adopted, stale_staging, decision, had_previous) = {
        let connection = state.db.lock().map_err(|_| {
            Failure::Failed("The local database is unavailable; restart Ludex.".to_string())
        })?;
        let previous =
            db::latest_stream_manifest_for_version(&connection, params.game_id, params.version_id)
                .ok()
                .flatten();
        let plan = plan_checkpoint(
            previous
                .as_ref()
                .map(|record| (record.fingerprint.as_str(), Path::new(&record.staging))),
            &fingerprint,
            previous
                .as_ref()
                .is_some_and(|record| Path::new(&record.staging).is_dir()),
            default_staging,
        );
        let bits = if plan.decision == CheckpointDecision::Adopted {
            previous
                .as_ref()
                .and_then(|record| {
                    db::get_install_job(&connection, &record.job_id)
                        .ok()
                        .flatten()
                        .and_then(|job| job.stream_completed)
                })
                .map(|value| CompletedEntries::from_hex(&value))
                .unwrap_or_default()
        } else {
            CompletedEntries::new(manifest.entry_count)
        };
        (
            bits,
            plan.staging,
            plan.decision == CheckpointDecision::Adopted,
            plan.stale_staging,
            plan.decision,
            previous.is_some(),
        )
    };
    completed.resize(manifest.entry_count);
    match decision {
        CheckpointDecision::Adopted => tracing::info!(
            game_id = params.game_id,
            version_id = params.version_id,
            "adopting the stream checkpoint of the previous attempt"
        ),
        CheckpointDecision::ArchiveChanged => tracing::warn!(
            game_id = params.game_id,
            version_id = params.version_id,
            "the served archive changed; restarting the stream install from zero"
        ),
        CheckpointDecision::Fresh if had_previous => tracing::info!(
            game_id = params.game_id,
            version_id = params.version_id,
            "the previous stream staging folder is gone; starting from zero"
        ),
        CheckpointDecision::Fresh => {}
    }
    if let Some(stale) = stale_staging {
        // Drop a checkpoint that no longer matches the served archive instead
        // of leaking the previous staging folder.
        let _ = tokio::fs::remove_dir_all(&stale).await;
    }
    tokio::fs::create_dir_all(&staging).await.map_err(|e| {
        Failure::Failed(format!(
            "Could not create the staging folder {}: {e}",
            staging.display()
        ))
    })?;
    let partial_root = staging.join(PARTIAL_DIR);
    if !adopted && tokio::fs::try_exists(&partial_root).await.unwrap_or(false) {
        let _ = tokio::fs::remove_dir_all(&partial_root).await;
    }
    tokio::fs::create_dir_all(&partial_root)
        .await
        .map_err(|e| {
            Failure::Failed(format!(
                "Could not create the staging folder {}: {e}",
                partial_root.display()
            ))
        })?;

    // Drop stale partial files (completed or out-of-range entries) and confirm
    // that files marked complete really are on disk with the right size.
    cleanup_partials(&partial_root, &completed, manifest.entry_count).await;
    for item in &manifest.entries {
        if item.is_dir || !completed.contains(item.index) {
            continue;
        }
        let Some(relative) = safe_tar_path(Path::new(&item.path)) else {
            completed.remove(item.index);
            continue;
        };
        let target = staging.join(relative);
        let ok = tokio::fs::metadata(&target)
            .await
            .map(|metadata| metadata.is_file() && metadata.len() == item.size)
            .unwrap_or(false);
        if !ok {
            completed.remove(item.index);
        }
    }

    // Create the directory entries once so an empty folder is preserved even
    // when no file lives below it.
    for item in &manifest.entries {
        if !item.is_dir {
            continue;
        }
        if let Some(relative) = safe_tar_path(Path::new(&item.path)) {
            let target = staging.join(relative);
            if let Err(error) = tokio::fs::create_dir_all(&target).await {
                tracing::warn!(path = %target.display(), error = %error, "could not create a streamed directory");
            }
        }
    }

    let job_id = entry.snapshot().job_id;
    {
        let connection = state.db.lock().map_err(|_| {
            Failure::Failed("The local database is unavailable; restart Ludex.".to_string())
        })?;
        let entries_json = serde_json::to_string(&manifest.entries)
            .map_err(|e| Failure::Failed(format!("Could not encode the stream manifest: {e}")))?;
        let _ = db::set_install_job_archive_format(&connection, &job_id, &manifest.format);
        db::upsert_stream_manifest(
            &connection,
            &db::StreamManifestRecord {
                job_id: job_id.clone(),
                game_id: params.game_id,
                version_id: params.version_id,
                format: manifest.format.clone(),
                staging: staging.to_string_lossy().to_string(),
                entries: entries_json,
                entry_count: manifest.entry_count as i64,
                total_bytes: total_bytes as i64,
                fingerprint: fingerprint.clone(),
                created_at: now_rfc3339(),
            },
        )
        .map_err(Failure::Failed)?;
        let _ = db::prune_stream_manifests(&connection, params.game_id, params.version_id, &job_id);
    }

    let resumed = completed.count();
    let files_total = files.len();
    let mut initial_bytes: u64 = files
        .iter()
        .filter(|item| completed.contains(item.index))
        .fold(0_u64, |total, item| total.saturating_add(item.size));
    // Count the bytes already on disk for entries that were interrupted.
    for item in files.iter().filter(|item| !completed.contains(item.index)) {
        let partial = partial_root.join(format!("{}.part", item.index));
        if let Ok(metadata) = tokio::fs::metadata(&partial).await {
            initial_bytes = initial_bytes.saturating_add(metadata.len().min(item.size));
        }
    }
    if initial_bytes > total_bytes {
        initial_bytes = total_bytes;
    }

    entry.set_stream_completed(Some(completed.to_hex()));
    entry.set_staging_path(Some(staging.to_string_lossy().to_string()));
    job_paths.staging = Some(staging.clone());
    entry.update(|job| {
        job.stream = true;
        job.resumable = true;
        job.phase = crate::install::InstallPhase::Downloading;
        job.bytes_done = initial_bytes;
        job.bytes_total = Some(total_bytes);
        job.speed_bps = 0;
        job.entries_total = Some(files_total as u32);
        job.entries_done = Some(resumed as u32);
        job.resumed_entries = Some(resumed as u32);
        job.current_entry = None;
        job.message = Some(match decision {
            CheckpointDecision::ArchiveChanged => {
                "The archive changed on the server; restarting the install from zero".to_string()
            }
            _ if resumed > 0 => {
                format!("Resuming: {resumed}/{files_total} files already installed")
            }
            _ => format!("Downloading {files_total} files"),
        });
    });
    persist(app, entry);
    let _ = app.emit("install://progress", entry.snapshot());

    let progress = Arc::new(ProgressState::new(initial_bytes, resumed as u64));
    let progress_task = {
        let app = app.clone();
        let entry = Arc::clone(entry);
        let progress = Arc::clone(&progress);
        tokio::spawn(async move {
            let mut last_bytes = progress.bytes.load(Ordering::Relaxed);
            let mut last_at = Instant::now();
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let done = progress.bytes.load(Ordering::Relaxed).min(total_bytes);
                let entries_done = progress.entries_done.load(Ordering::Relaxed);
                let elapsed = last_at.elapsed().as_secs_f64().max(0.001);
                let speed = ((done.saturating_sub(last_bytes)) as f64 / elapsed) as u64;
                let current = progress.current();
                let message = if current.is_empty() {
                    format!("Downloading {entries_done}/{files_total} files")
                } else {
                    format!("{entries_done}/{files_total} files · {current}")
                };
                entry.update(|job| {
                    job.phase = crate::install::InstallPhase::Downloading;
                    job.bytes_done = done;
                    job.bytes_total = Some(total_bytes);
                    job.speed_bps = speed;
                    job.entries_done = Some(entries_done as u32);
                    job.entries_total = Some(files_total as u32);
                    job.current_entry = (!current.is_empty()).then(|| current.clone());
                    job.message = Some(message);
                });
                persist(&app, &entry);
                let _ = app.emit("install://progress", entry.snapshot());
                last_bytes = done;
                last_at = Instant::now();
            }
        })
    };

    let pending: Vec<StreamEntry> = files
        .into_iter()
        .filter(|item| !completed.contains(item.index))
        .collect();

    let transport = HttpTransport {
        client: &state.http,
    };
    let downloaded = download_entries(
        &transport,
        base,
        params.token.as_deref(),
        params.game_id,
        params.version_id,
        pending,
        &staging,
        &partial_root,
        &mut completed,
        state.settings_snapshot().bandwidth_limit_kib,
        &entry.cancel,
        &entry.pause_token(),
        &progress,
        StreamTuning::default(),
        |item, completed_hex| {
            entry.set_stream_completed(Some(completed_hex.to_string()));
            let entries_done = progress.entries_done.load(Ordering::Relaxed);
            let done_bytes = progress.bytes.load(Ordering::Relaxed).min(total_bytes);
            entry.update(|job| {
                job.bytes_done = done_bytes;
                job.entries_done = Some(entries_done as u32);
                job.current_entry = Some(item.path.clone());
            });
            // The checkpoint must be durable before the next attempt can
            // trust the file: persist after every completed entry.
            persist(app, entry);
            let _ = app.emit("install://progress", entry.snapshot());
            tracing::debug!(index = item.index, path = %item.path, "streamed entry");
        },
    )
    .await;
    progress_task.abort();
    downloaded?;

    let _ = tokio::fs::remove_dir_all(&partial_root).await;
    let files_done = progress.entries_done.load(Ordering::Relaxed);
    entry.set_stream_completed(Some(completed.to_hex()));
    entry.update(|job| {
        job.bytes_done = total_bytes;
        job.bytes_total = Some(total_bytes);
        job.speed_bps = 0;
        job.entries_done = Some(files_done as u32);
        job.entries_total = Some(files_total as u32);
        job.current_entry = None;
        job.message = Some(format!("Stream complete ({files_done} files)"));
    });
    persist(app, entry);
    let _ = app.emit("install://progress", entry.snapshot());

    Ok(StreamOutcome::Done)
}

/// Downloads the pending entries two at a time and records each completion.
///
/// The callback runs after a file is on disk and its checkpoint bit is set, so
/// the caller can persist the job row before the next entry finishes.
#[allow(clippy::too_many_arguments)]
async fn download_entries<F>(
    transport: &dyn EntryTransport,
    base: &str,
    token: Option<&str>,
    game_id: i64,
    version_id: i64,
    pending: Vec<StreamEntry>,
    staging: &Path,
    partial_root: &Path,
    completed: &mut CompletedEntries,
    bandwidth_kib: u64,
    cancel: &CancellationToken,
    pause: &CancellationToken,
    progress: &Arc<ProgressState>,
    tuning: StreamTuning,
    mut on_complete: F,
) -> Result<(), Failure>
where
    F: FnMut(&StreamEntry, &str),
{
    let throttle = Arc::new(Mutex::new(Throttle::new(bandwidth_kib)));
    let downloads = futures_util::stream::iter(pending)
        .map(|item| {
            let token = token.map(str::to_string);
            let staging = staging.to_path_buf();
            let partial_root = partial_root.to_path_buf();
            let throttle = Arc::clone(&throttle);
            let cancel = cancel.clone();
            let pause = pause.clone();
            let progress = Arc::clone(progress);
            async move {
                download_entry(
                    transport,
                    base,
                    token.as_deref(),
                    game_id,
                    version_id,
                    &item,
                    &staging,
                    &partial_root,
                    &throttle,
                    &cancel,
                    &pause,
                    &progress,
                    tuning,
                )
                .await
                .map(|()| item)
            }
        })
        .buffer_unordered(ENTRY_CONCURRENCY);
    tokio::pin!(downloads);

    while let Some(result) = downloads.next().await {
        let item = result?;
        completed.insert(item.index);
        progress.entries_done.fetch_add(1, Ordering::Relaxed);
        on_complete(&item, &completed.to_hex());
    }
    Ok(())
}

/// Downloads one entry into `staging`, resuming a `.part` file with Range.
///
/// Every attempt continues at the current `.part` size. A stalled body, a
/// dropped connection, a proxy 502/504 and a short body are retried with
/// bounded exponential backoff (#204); the checkpoint on disk is what makes
/// each retry cheap. When the whole job has received zero bytes and every
/// attempt fails at the transport level, the stream path is reported as
/// unavailable so the caller can fall back to the classic download once.
#[allow(clippy::too_many_arguments)]
async fn download_entry(
    transport: &dyn EntryTransport,
    base: &str,
    token: Option<&str>,
    game_id: i64,
    version_id: i64,
    item: &StreamEntry,
    staging: &Path,
    partial_root: &Path,
    throttle: &Arc<Mutex<Option<Throttle>>>,
    cancel: &CancellationToken,
    pause: &CancellationToken,
    progress: &ProgressState,
    tuning: StreamTuning,
) -> Result<(), Failure> {
    let relative = safe_tar_path(Path::new(&item.path)).ok_or_else(|| {
        Failure::Failed(format!(
            "The server sent an unsafe entry path: {}",
            item.path
        ))
    })?;
    let target = staging.join(&relative);
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            Failure::Failed(format!(
                "Could not create the folder {}: {e}",
                parent.display()
            ))
        })?;
    }

    // Already on disk from an earlier attempt whose checkpoint was lost.
    if let Ok(metadata) = tokio::fs::metadata(&target).await {
        if metadata.is_file() && metadata.len() == item.size {
            progress.bytes.fetch_add(item.size, Ordering::Relaxed);
            return Ok(());
        }
    }

    let partial = partial_root.join(format!("{}.part", item.index));
    let url = format!(
        "{base}/api/games/{game_id}/versions/{version_id}/stream/entries/{}",
        item.index
    );

    let started = Instant::now();
    let mut last_error = String::new();
    for attempt in 1..=tuning.attempts {
        if attempt > 1 {
            let delay = tuning.backoff(attempt - 1);
            let kept = partial_size(&partial).await;
            tracing::warn!(
                index = item.index,
                path = %item.path,
                attempt,
                max_attempts = tuning.attempts,
                bytes = kept,
                delay_ms = delay.as_millis() as u64,
                error = %last_error,
                "retrying stream entry"
            );
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(Failure::Cancelled),
                _ = pause.cancelled() => return Err(Failure::Paused),
                _ = tokio::time::sleep(delay) => {}
            }
        }
        match download_entry_once(
            transport, &url, token, item, &partial, throttle, cancel, pause, progress, tuning,
        )
        .await
        {
            Ok(()) => {
                finalize_entry(&partial, &target, item).await?;
                tracing::info!(
                    index = item.index,
                    path = %item.path,
                    bytes = item.size,
                    attempts = attempt,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "stream entry installed"
                );
                return Ok(());
            }
            Err(Failure::Cancelled) => return Err(Failure::Cancelled),
            Err(Failure::Paused) => return Err(Failure::Paused),
            Err(Failure::StreamUnavailable(message)) => {
                return Err(Failure::StreamUnavailable(message))
            }
            Err(Failure::Failed(message)) => {
                if is_permanent_http_failure(&message) {
                    // 401/403/404/415: retrying cannot help. Report at once so
                    // the UI can offer a real fix (sign in, another version).
                    return Err(Failure::Failed(message));
                }
                if message.contains("checksum") {
                    // The bytes on disk are wrong: drop them and fetch the
                    // entry again from zero.
                    let size = partial_size(&partial).await;
                    progress.subtract_bytes(size);
                    let _ = tokio::fs::remove_file(&partial).await;
                }
                last_error = message;
            }
        }
    }

    let kept = partial_size(&partial).await;
    tracing::warn!(
        index = item.index,
        path = %item.path,
        attempts = tuning.attempts,
        bytes = kept,
        elapsed_ms = started.elapsed().as_millis() as u64,
        error = %last_error,
        "stream entry failed"
    );
    // Nothing was ever received for this job: a proxy or a broken stream path
    // rejects every attempt before the first byte. Let the caller fall back to
    // the classic resumable download instead of looping (#204).
    if progress.bytes.load(Ordering::Relaxed) == 0 && kept == 0 {
        return Err(Failure::StreamUnavailable(format!(
            "entry {} could not be reached after {} attempts: {last_error}",
            item.path, tuning.attempts
        )));
    }
    Err(Failure::Failed(format!(
        "Entry {} failed after {} attempts ({kept} bytes kept): {last_error}. Resume the install to continue.",
        item.path, tuning.attempts
    )))
}

/// HTTP failures that a retry cannot fix.
fn is_permanent_http_failure(message: &str) -> bool {
    [
        "HTTP 400", "HTTP 401", "HTTP 403", "HTTP 404", "HTTP 405", "HTTP 415", "HTTP 416",
    ]
    .iter()
    .any(|code| message.contains(code))
}

async fn partial_size(partial: &Path) -> u64 {
    tokio::fs::metadata(partial)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

/// One HTTP pass for an entry, starting at the current `.part` size.
///
/// The body is read with a stall timeout: a decompress-and-skip that stops
/// sending chunks, an idle proxy or a half-closed connection is treated as a
/// retryable failure and the next attempt resumes at the exact byte.
#[allow(clippy::too_many_arguments)]
async fn download_entry_once(
    transport: &dyn EntryTransport,
    url: &str,
    token: Option<&str>,
    item: &StreamEntry,
    partial: &Path,
    throttle: &Arc<Mutex<Option<Throttle>>>,
    cancel: &CancellationToken,
    pause: &CancellationToken,
    progress: &ProgressState,
    tuning: StreamTuning,
) -> Result<(), Failure> {
    if item.size == 0 {
        let _ = tokio::fs::write(partial, []).await;
        return Ok(());
    }

    let mut offset = partial_size(partial).await;
    if offset > item.size {
        progress.subtract_bytes(offset);
        let _ = tokio::fs::remove_file(partial).await;
        offset = 0;
    }
    progress.set_current(&item.path);

    let mut restarted = false;
    loop {
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(Failure::Cancelled),
            _ = pause.cancelled() => return Err(Failure::Paused),
            response = transport.fetch(url, token, offset) => response,
        }
        .map_err(|e| {
            Failure::Failed(format!(
                "Could not download {}: {e}. Retrying from the saved offset.",
                item.path
            ))
        })?;

        let status = response.status;
        if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            // The `.part` may already hold the whole entry (completed just
            // before the app stopped) or be bogus.
            if offset == item.size {
                return Ok(());
            }
            if !restarted {
                restarted = true;
                progress.subtract_bytes(offset);
                let _ = tokio::fs::remove_file(partial).await;
                offset = 0;
                continue;
            }
            return Err(Failure::Failed(format!(
                "The server rejected the resume of {} (HTTP 416).",
                item.path
            )));
        }
        if !status.is_success() {
            let code = status.as_u16();
            let hint = match code {
                401 | 403 => " Check that you are signed in to the server.",
                404 => " The game version or entry was not found on the server.",
                415 => " This server cannot stream this archive per entry.",
                _ => "",
            };
            return Err(Failure::Failed(format!(
                "Entry {} failed with HTTP {code}{hint}",
                item.path
            )));
        }

        let append = status == reqwest::StatusCode::PARTIAL_CONTENT && offset > 0;
        if append {
            if response.content_range_start != Some(offset) {
                let _ = tokio::fs::remove_file(partial).await;
                if restarted {
                    return Err(Failure::Failed(format!(
                        "The server resumed {} at an unexpected offset.",
                        item.path
                    )));
                }
                restarted = true;
                progress.subtract_bytes(offset);
                offset = 0;
                continue;
            }
        } else if offset > 0 {
            // The server ignored Range and sent the whole entry: rewrite it.
            progress.subtract_bytes(offset);
            offset = 0;
        }

        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(partial)
            .await
            .map_err(|e| {
                Failure::Failed(format!(
                    "Could not open the partial file {}: {e}",
                    partial.display()
                ))
            })?;

        let mut writer = tokio::io::BufWriter::with_capacity(1024 * 1024, file);
        let mut written = offset;
        let mut stream = response.body;
        loop {
            let chunk = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    let _ = writer.flush().await;
                    return Err(Failure::Cancelled);
                }
                _ = pause.cancelled() => {
                    let _ = writer.flush().await;
                    return Err(Failure::Paused);
                }
                result = tokio::time::timeout(tuning.stall_timeout, stream.next()) => result,
            };
            match chunk {
                Err(_elapsed) => {
                    let _ = writer.flush().await;
                    return Err(Failure::Failed(format!(
                        "Entry {} stalled for {:?} after {written} bytes; retrying from that offset.",
                        item.path, tuning.stall_timeout
                    )));
                }
                Ok(Some(Ok(bytes))) => {
                    writer.write_all(&bytes).await.map_err(|e| {
                        Failure::Failed(format!("Could not write {}: {e}", partial.display()))
                    })?;
                    written = written.saturating_add(bytes.len() as u64);
                    progress
                        .bytes
                        .fetch_add(bytes.len() as u64, Ordering::Relaxed);
                    // The throttle guard must not live across an await (the
                    // install future is Send), so compute the delay in a scope.
                    let delay = {
                        let mut guard = match throttle.lock() {
                            Ok(guard) => guard,
                            Err(poisoned) => poisoned.into_inner(),
                        };
                        guard
                            .as_mut()
                            .and_then(|throttle| throttle.pace(bytes.len()))
                    };
                    if let Some(delay) = delay.filter(|delay| *delay > Duration::from_millis(2)) {
                        tokio::select! {
                            biased;
                            _ = cancel.cancelled() => {
                                let _ = writer.flush().await;
                                return Err(Failure::Cancelled);
                            }
                            _ = pause.cancelled() => {
                                let _ = writer.flush().await;
                                return Err(Failure::Paused);
                            }
                            _ = tokio::time::sleep(delay) => {}
                        }
                    }
                }
                Ok(Some(Err(error))) => {
                    let _ = writer.flush().await;
                    return Err(Failure::Failed(format!(
                        "The download of {} was interrupted after {written} bytes: {error}. Retrying from that offset.",
                        item.path
                    )));
                }
                Ok(None) => break,
            }
        }

        writer
            .flush()
            .await
            .map_err(|e| Failure::Failed(format!("Could not flush {}: {e}", partial.display())))?;
        let file = writer.into_inner();
        file.sync_all().await.map_err(|e| {
            Failure::Failed(format!(
                "Could not flush {} to disk: {e}",
                partial.display()
            ))
        })?;

        let size = partial_size(partial).await;
        if size != item.size {
            return Err(Failure::Failed(format!(
                "Entry {} size mismatch: expected {} bytes, got {size}",
                item.path, item.size
            )));
        }
        return Ok(());
    }
}

/// Verifies the CRC-32, moves the partial file into place and applies the mode.
async fn finalize_entry(partial: &Path, target: &Path, item: &StreamEntry) -> Result<(), Failure> {
    let path = partial.to_path_buf();
    let expected = item.crc32;
    let crc = tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = crc32fast::Hasher::new();
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok::<u32, std::io::Error>(hasher.finalize())
    })
    .await
    .map_err(|e| Failure::Failed(format!("The checksum task failed: {e}")))?
    .map_err(|e| Failure::Failed(format!("Could not checksum {}: {e}", item.path)))?;

    if crc != expected {
        let _ = tokio::fs::remove_file(partial).await;
        return Err(Failure::Failed(format!(
            "Entry {} checksum mismatch: expected {expected:08x}, got {crc:08x}",
            item.path
        )));
    }

    if tokio::fs::metadata(target).await.is_ok() {
        let _ = tokio::fs::remove_file(target).await;
    }
    tokio::fs::rename(partial, target)
        .await
        .map_err(|e| Failure::Failed(format!("Could not move {} into place: {e}", item.path)))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if item.mode & 0o777 != 0 {
            let _ = tokio::fs::set_permissions(
                target,
                std::fs::Permissions::from_mode(item.mode & 0o777),
            )
            .await;
        }
    }
    Ok(())
}

/// Removes partial files whose entry is already complete or out of range.
async fn cleanup_partials(root: &Path, completed: &CompletedEntries, entry_count: usize) {
    let Ok(mut directory) = tokio::fs::read_dir(root).await else {
        return;
    };
    while let Ok(Some(item)) = directory.next_entry().await {
        let name = item.file_name();
        let Some(stem) = name.to_str().and_then(|name| name.strip_suffix(".part")) else {
            continue;
        };
        let Ok(index) = stem.parse::<usize>() else {
            continue;
        };
        if index >= entry_count || completed.contains(index) {
            let _ = tokio::fs::remove_file(item.path()).await;
        }
    }
}

/// Fetches the manifest. Any transport or HTTP failure is reported as a status
/// so the caller can fall back to the sequential stream instead of failing the
/// whole install (an old server simply returns 404 here).
async fn fetch_manifest(
    state: &AppState,
    base: &str,
    params: &InstallParams,
) -> Result<StreamManifest, reqwest::StatusCode> {
    let url = format!(
        "{base}/api/games/{}/versions/{}/stream/manifest",
        params.game_id, params.version_id
    );
    let mut request = state
        .http
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::ACCEPT_ENCODING, "identity");
    if let Some(token) = params
        .token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        request = request.bearer_auth(token);
    }
    match request.send().await {
        Ok(response) if response.status().is_success() => {
            response.json::<StreamManifest>().await.map_err(|error| {
                tracing::warn!(error = %error, "the stream manifest was not valid JSON");
                reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE
            })
        }
        Ok(response) => Err(response.status()),
        Err(error) => {
            tracing::warn!(error = %error, "could not fetch the stream manifest");
            Err(reqwest::StatusCode::NOT_FOUND)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ludex-stream-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// Writes a real ZIP, then derives the manifest and the raw bytes of every
    /// entry exactly like the server would (central directory + entry reads).
    fn zip_fixture(label: &str) -> (StreamManifest, Vec<Vec<u8>>) {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let dir = temp_dir(label);
        let path = dir.join("fixture.zip");
        let file = std::fs::File::create(&path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .add_directory("Game/", SimpleFileOptions::default())
            .unwrap();
        archive
            .start_file(
                "Game/game.exe",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        archive.write_all(b"MZ streamed").unwrap();
        archive
            .start_file(
                "Game/data/big.bin",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        archive
            .write_all(
                &(0..400_000)
                    .map(|index| (index % 241) as u8)
                    .collect::<Vec<u8>>(),
            )
            .unwrap();
        archive.start_file("Game/run.sh", zip_options()).unwrap();
        archive.write_all(b"#!/bin/sh\nexit 0\n").unwrap();
        archive.finish().unwrap();

        let manifest = ludex_core::archive::zip_stream_manifest(&path).expect("manifest");
        let highest = manifest
            .entries
            .iter()
            .map(|item| item.index)
            .max()
            .unwrap_or(0);
        let mut contents = vec![Vec::new(); highest + 1];
        for item in &manifest.entries {
            if item.is_dir {
                continue;
            }
            let mut bytes = Vec::new();
            ludex_core::archive::write_zip_entry_range(&path, item.index, 0, None, &mut bytes)
                .expect("entry bytes");
            assert_eq!(bytes.len() as u64, item.size);
            assert_eq!(crc32fast::hash(&bytes), item.crc32);
            contents[item.index] = bytes;
        }
        let _ = std::fs::remove_dir_all(&dir);
        (manifest, contents)
    }

    fn zip_options() -> zip::write::SimpleFileOptions {
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)
    }

    fn files(manifest: &StreamManifest) -> Vec<StreamEntry> {
        manifest
            .entries
            .iter()
            .filter(|item| !item.is_dir)
            .cloned()
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    async fn run(
        server: &crate::test_support::ZipServer,
        manifest: &StreamManifest,
        contents: &[Vec<u8>],
        completed: &mut CompletedEntries,
        staging: &Path,
        partial_root: &Path,
        progress: &Arc<ProgressState>,
        cancel: &CancellationToken,
        pause: &CancellationToken,
        on_complete: impl FnMut(&StreamEntry, &str),
    ) -> Result<(), Failure> {
        run_with_tuning(
            server,
            manifest,
            contents,
            completed,
            staging,
            partial_root,
            progress,
            cancel,
            pause,
            StreamTuning::default(),
            on_complete,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_with_tuning(
        server: &crate::test_support::ZipServer,
        manifest: &StreamManifest,
        contents: &[Vec<u8>],
        completed: &mut CompletedEntries,
        staging: &Path,
        partial_root: &Path,
        progress: &Arc<ProgressState>,
        cancel: &CancellationToken,
        pause: &CancellationToken,
        tuning: StreamTuning,
        on_complete: impl FnMut(&StreamEntry, &str),
    ) -> Result<(), Failure> {
        let pending: Vec<StreamEntry> = files(manifest)
            .into_iter()
            .filter(|item| !completed.contains(item.index))
            .collect();
        let _ = contents;
        let client = reqwest::Client::new();
        let transport = HttpTransport { client: &client };
        download_entries(
            &transport,
            &server.url,
            None,
            7,
            9,
            pending,
            staging,
            partial_root,
            completed,
            0,
            cancel,
            pause,
            progress,
            tuning,
            on_complete,
        )
        .await
    }

    #[test]
    fn completed_bitset_roundtrips_and_skips() {
        let mut completed = CompletedEntries::new(20);
        assert_eq!(completed.count(), 0);
        completed.insert(0);
        completed.insert(13);
        completed.insert(19);
        assert!(completed.contains(13));
        assert!(!completed.contains(14));
        assert_eq!(completed.count(), 3);
        completed.remove(13);
        assert_eq!(completed.count(), 2);

        let hex = completed.to_hex();
        let loaded = CompletedEntries::from_hex(&hex);
        assert_eq!(loaded, completed);
        assert_eq!(loaded.count(), 2);
        let mut resized = loaded.clone();
        resized.resize(4);
        assert!(resized.contains(0));
        assert!(!resized.contains(19));
        assert_eq!(
            CompletedEntries::from_hex("not hex"),
            CompletedEntries::default()
        );
    }

    #[tokio::test]
    async fn downloads_entries_resumes_a_partial_file_after_a_restart() {
        let (manifest, contents) = zip_fixture("resume");
        let files = files(&manifest);
        let big = files
            .iter()
            .find(|item| item.path.ends_with("big.bin"))
            .expect("big entry")
            .clone();
        let big_index = big.index;
        let big_bytes = contents[big_index].clone();

        let staging = temp_dir("resume-staging");
        let partial_root = staging.join(PARTIAL_DIR);
        std::fs::create_dir_all(&partial_root).unwrap();
        let mut completed = CompletedEntries::new(manifest.entry_count);
        // Pretend a previous attempt already installed the first file.
        let exe = files
            .iter()
            .find(|item| item.path.ends_with("game.exe"))
            .expect("exe entry")
            .clone();
        std::fs::create_dir_all(staging.join("Game")).unwrap();
        std::fs::write(staging.join(&exe.path), &contents[exe.index]).unwrap();
        completed.insert(exe.index);
        let progress = Arc::new(ProgressState::new(exe.size, 1));

        // First attempt: one entry fails, the others land on disk.
        let failing =
            crate::test_support::serve_zip(manifest.clone(), contents.clone(), Some(big_index))
                .await;
        let result = run_with_tuning(
            &failing,
            &manifest,
            &contents,
            &mut completed,
            &staging,
            &partial_root,
            &progress,
            &CancellationToken::new(),
            &CancellationToken::new(),
            StreamTuning {
                attempts: 1,
                ..StreamTuning::default()
            },
            |_, _| {},
        )
        .await;
        assert!(matches!(result, Err(Failure::Failed(_))));
        let checkpoint = completed.to_hex();
        let completed_before = completed.count();
        assert!(
            completed_before >= 1,
            "at least one entry must be complete before the failure"
        );
        assert!(!completed.contains(big_index));
        for item in files.iter().filter(|item| completed.contains(item.index)) {
            assert!(staging.join(&item.path).is_file(), "{} kept", item.path);
        }

        // Simulate the app restart: the bitset is reloaded from SQLite and the
        // interrupted entry left a partial file.
        let mut resumed = CompletedEntries::from_hex(&checkpoint);
        let skip: Vec<usize> = (0..manifest.entry_count)
            .filter(|index| resumed.contains(*index))
            .collect();
        let half = (big_bytes.len() / 2) as u64;
        std::fs::write(
            partial_root.join(format!("{big_index}.part")),
            &big_bytes[..half as usize],
        )
        .unwrap();
        let progress = ProgressState::new(half, 1);
        let progress = Arc::new(progress);

        let server = crate::test_support::serve_zip(manifest.clone(), contents.clone(), None).await;
        run(
            &server,
            &manifest,
            &contents,
            &mut resumed,
            &staging,
            &partial_root,
            &progress,
            &CancellationToken::new(),
            &CancellationToken::new(),
            |_, _| {},
        )
        .await
        .expect("resumed download");

        assert_eq!(resumed.count(), files.len());
        for item in &files {
            let bytes = std::fs::read(staging.join(&item.path)).expect("installed file");
            assert_eq!(bytes, contents[item.index]);
        }
        let ranges = server.ranges.lock().unwrap().clone();
        assert!(
            ranges
                .iter()
                .any(|(index, range)| *index == big_index && range == &format!("bytes={half}-")),
            "the interrupted entry must resume with Range, got {ranges:?}"
        );
        for index in skip {
            assert!(
                !ranges.iter().any(|(requested, _)| *requested == index),
                "completed entry {index} must not be requested again"
            );
        }
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[tokio::test]
    async fn cancel_stops_between_entries_and_keeps_completed_files() {
        let (manifest, contents) = zip_fixture("cancel");
        let staging = temp_dir("cancel-staging");
        let partial_root = staging.join(PARTIAL_DIR);
        std::fs::create_dir_all(&partial_root).unwrap();
        let mut completed = CompletedEntries::new(manifest.entry_count);
        let progress = Arc::new(ProgressState::new(0, 0));
        let server = crate::test_support::serve_zip(manifest.clone(), contents, None).await;

        let cancel = CancellationToken::new();
        let cancel_from_callback = cancel.clone();
        let result = run(
            &server,
            &manifest,
            &[],
            &mut completed,
            &staging,
            &partial_root,
            &progress,
            &cancel,
            &CancellationToken::new(),
            move |_, _| cancel_from_callback.cancel(),
        )
        .await;
        assert!(matches!(result, Err(Failure::Cancelled)));
        assert!(
            completed.count() >= 1,
            "a cancellation between entries keeps the completed ones"
        );
        let kept: Vec<_> = files(&manifest)
            .into_iter()
            .filter(|item| completed.contains(item.index))
            .collect();
        assert!(!kept.is_empty());
        for item in kept {
            assert!(staging.join(&item.path).is_file());
        }
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[tokio::test]
    async fn a_corrupted_entry_is_rejected_by_its_crc() {
        let (manifest, mut contents) = zip_fixture("crc");
        let target = files(&manifest)
            .into_iter()
            .find(|item| item.path.ends_with("game.exe"))
            .expect("entry")
            .index;
        contents[target] = b"MZ tampered".to_vec();
        let staging = temp_dir("crc-staging");
        let partial_root = staging.join(PARTIAL_DIR);
        std::fs::create_dir_all(&partial_root).unwrap();
        let mut completed = CompletedEntries::new(manifest.entry_count);
        let progress = Arc::new(ProgressState::new(0, 0));
        let server = crate::test_support::serve_zip(manifest.clone(), contents, None).await;

        let result = run_with_tuning(
            &server,
            &manifest,
            &[],
            &mut completed,
            &staging,
            &partial_root,
            &progress,
            &CancellationToken::new(),
            &CancellationToken::new(),
            StreamTuning {
                attempts: 1,
                ..StreamTuning::default()
            },
            |_, _| {},
        )
        .await;
        match result {
            Err(Failure::Failed(message)) => {
                assert!(message.contains("checksum"), "unexpected error: {message}");
            }
            other => panic!("expected a checksum failure, got {other:?}"),
        }
        assert!(!completed.contains(target));
        assert!(!staging.join("Game/game.exe").exists());
        let _ = std::fs::remove_dir_all(&staging);
    }
    /* ------------------- injected transport (#204/#203) ------------------- */

    enum ScriptedResult {
        Response {
            status: u16,
            content_range_start: Option<u64>,
            chunks: Vec<Result<Vec<u8>, String>>,
            /// Keep the body open after the chunks so the stall timeout fires.
            stall: bool,
        },
        Error(String),
    }

    type ScriptHandler = Box<dyn Fn(usize, u64, usize, &[Vec<u8>]) -> ScriptedResult + Send + Sync>;

    /// Test transport: answers from a handler keyed by entry index and the
    /// per-entry attempt number. No socket, so a stall or a mid-body drop is
    /// deterministic.
    struct ScriptedTransport {
        contents: Arc<Vec<Vec<u8>>>,
        handler: ScriptHandler,
        requests: Mutex<Vec<(usize, u64)>>,
        counts: Mutex<std::collections::HashMap<usize, usize>>,
    }

    impl ScriptedTransport {
        fn new(
            contents: Vec<Vec<u8>>,
            handler: impl Fn(usize, u64, usize, &[Vec<u8>]) -> ScriptedResult + Send + Sync + 'static,
        ) -> Self {
            Self {
                contents: Arc::new(contents),
                handler: Box::new(handler),
                requests: Mutex::new(Vec::new()),
                counts: Mutex::new(std::collections::HashMap::new()),
            }
        }

        fn requests(&self) -> Vec<(usize, u64)> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl EntryTransport for ScriptedTransport {
        fn fetch<'a>(
            &'a self,
            url: &'a str,
            _token: Option<&'a str>,
            offset: u64,
        ) -> Pin<Box<dyn Future<Output = Result<EntryResponse, String>> + Send + 'a>> {
            Box::pin(async move {
                let index = url
                    .rsplit('/')
                    .next()
                    .and_then(|value| value.parse::<usize>().ok())
                    .ok_or_else(|| "bad test url".to_string())?;
                let attempt = {
                    let mut counts = self.counts.lock().unwrap();
                    let entry = counts.entry(index).or_insert(0);
                    *entry += 1;
                    *entry
                };
                self.requests.lock().unwrap().push((index, offset));
                match (self.handler)(index, offset, attempt, &self.contents) {
                    ScriptedResult::Error(message) => Err(message),
                    ScriptedResult::Response {
                        status,
                        content_range_start,
                        chunks,
                        stall,
                    } => {
                        let mut body: Pin<
                            Box<dyn futures_util::Stream<Item = Result<Vec<u8>, String>> + Send>,
                        > = Box::pin(futures_util::stream::iter(chunks));
                        if stall {
                            body = Box::pin(body.chain(futures_util::stream::pending()));
                        }
                        Ok(EntryResponse {
                            status: reqwest::StatusCode::from_u16(status).unwrap(),
                            content_range_start,
                            body,
                        })
                    }
                }
            })
        }
    }

    fn full_entry(contents: &[Vec<u8>], index: usize) -> ScriptedResult {
        ScriptedResult::Response {
            status: 200,
            content_range_start: None,
            chunks: vec![Ok(contents[index].clone())],
            stall: false,
        }
    }

    fn fast_tuning(attempts: u32) -> StreamTuning {
        StreamTuning {
            stall_timeout: Duration::from_millis(120),
            attempts,
            backoff: Duration::from_millis(1),
        }
    }

    fn scripted_staging(label: &str) -> (PathBuf, PathBuf) {
        let staging = temp_dir(label);
        let partial_root = staging.join(PARTIAL_DIR);
        std::fs::create_dir_all(&partial_root).unwrap();
        (staging, partial_root)
    }

    #[tokio::test]
    async fn a_stalled_entry_resumes_at_the_exact_byte() {
        let (manifest, contents) = zip_fixture("stall");
        let big = files(&manifest)
            .into_iter()
            .find(|item| item.path.ends_with("big.bin"))
            .expect("big entry");
        let big_index = big.index;
        let big_len = contents[big_index].len();
        let half = (big_len / 2) as u64;
        let (staging, partial_root) = scripted_staging("stall-staging");
        let mut completed = CompletedEntries::new(manifest.entry_count);
        let progress = Arc::new(ProgressState::new(0, 0));
        let transport =
            ScriptedTransport::new(contents.clone(), move |index, offset, attempt, contents| {
                if index != big_index {
                    return full_entry(contents, index);
                }
                if attempt == 1 {
                    // Half the entry, then the body never sends another chunk.
                    ScriptedResult::Response {
                        status: 200,
                        content_range_start: None,
                        chunks: vec![Ok(contents[index][..half as usize].to_vec())],
                        stall: true,
                    }
                } else {
                    // The retry must ask for exactly the bytes already on disk.
                    ScriptedResult::Response {
                        status: 206,
                        content_range_start: Some(offset),
                        chunks: vec![Ok(contents[index][offset as usize..].to_vec())],
                        stall: false,
                    }
                }
            });

        let pending: Vec<StreamEntry> = files(&manifest)
            .into_iter()
            .filter(|item| !completed.contains(item.index))
            .collect();
        let downloaded = download_entries(
            &transport,
            "http://test",
            None,
            7,
            9,
            pending,
            &staging,
            &partial_root,
            &mut completed,
            0,
            &CancellationToken::new(),
            &CancellationToken::new(),
            &progress,
            fast_tuning(3),
            |_, _| {},
        )
        .await;
        downloaded.expect("the stalled entry must resume");

        let requests = transport.requests();
        assert!(
            requests.contains(&(big_index, half)),
            "the retry must resume at the half-written byte, got {requests:?}"
        );
        assert_eq!(completed.count(), files(&manifest).len());
        for item in files(&manifest) {
            assert_eq!(
                std::fs::read(staging.join(&item.path)).unwrap(),
                contents[item.index],
                "{} must be byte-identical",
                item.path
            );
        }
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[tokio::test]
    async fn a_mid_body_disconnect_is_retried_from_the_written_offset() {
        let (manifest, contents) = zip_fixture("drop");
        let big = files(&manifest)
            .into_iter()
            .find(|item| item.path.ends_with("big.bin"))
            .expect("big entry");
        let big_index = big.index;
        let half = (contents[big_index].len() / 2) as u64;
        let (staging, partial_root) = scripted_staging("drop-staging");
        let mut completed = CompletedEntries::new(manifest.entry_count);
        let progress = Arc::new(ProgressState::new(0, 0));
        let transport =
            ScriptedTransport::new(contents.clone(), move |index, offset, attempt, contents| {
                if index != big_index {
                    return full_entry(contents, index);
                }
                if attempt == 1 {
                    ScriptedResult::Response {
                        status: 200,
                        content_range_start: None,
                        chunks: vec![
                            Ok(contents[index][..half as usize].to_vec()),
                            Err("connection reset by peer".to_string()),
                        ],
                        stall: false,
                    }
                } else {
                    ScriptedResult::Response {
                        status: 206,
                        content_range_start: Some(offset),
                        chunks: vec![Ok(contents[index][offset as usize..].to_vec())],
                        stall: false,
                    }
                }
            });

        let pending: Vec<StreamEntry> = files(&manifest)
            .into_iter()
            .filter(|item| !completed.contains(item.index))
            .collect();
        download_entries(
            &transport,
            "http://test",
            None,
            7,
            9,
            pending,
            &staging,
            &partial_root,
            &mut completed,
            0,
            &CancellationToken::new(),
            &CancellationToken::new(),
            &progress,
            fast_tuning(3),
            |_, _| {},
        )
        .await
        .expect("the dropped entry must resume");

        assert!(transport.requests().contains(&(big_index, half)));
        assert_eq!(
            std::fs::read(staging.join(&big.path)).unwrap(),
            contents[big_index]
        );
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[tokio::test]
    async fn entry_retries_are_bounded_and_keep_the_checkpoint() {
        let (manifest, contents) = zip_fixture("bounded");
        let big = files(&manifest)
            .into_iter()
            .find(|item| item.path.ends_with("big.bin"))
            .expect("big entry");
        let big_index = big.index;
        let (staging, partial_root) = scripted_staging("bounded-staging");
        let mut completed = CompletedEntries::new(manifest.entry_count);
        let progress = Arc::new(ProgressState::new(0, 0));
        let transport = ScriptedTransport::new(
            contents.clone(),
            move |index, _offset, _attempt, contents| {
                if index == big_index {
                    ScriptedResult::Error("proxy returned 502".to_string())
                } else {
                    full_entry(contents, index)
                }
            },
        );

        let mut pending: Vec<StreamEntry> = files(&manifest)
            .into_iter()
            .filter(|item| !completed.contains(item.index))
            .collect();
        // Schedule the doomed entry last: with two concurrent entries the test
        // then deterministically has finished entries to keep when the failure
        // aborts the others (it used to depend on scheduler timing).
        pending.sort_by_key(|item| item.index == big_index);
        let result = download_entries(
            &transport,
            "http://test",
            None,
            7,
            9,
            pending,
            &staging,
            &partial_root,
            &mut completed,
            0,
            &CancellationToken::new(),
            &CancellationToken::new(),
            &progress,
            fast_tuning(3),
            |_, _| {},
        )
        .await;

        match result {
            Err(Failure::Failed(message)) => {
                assert!(message.contains("after 3 attempts"), "{message}");
            }
            other => panic!("expected a bounded failure, got {other:?}"),
        }
        let big_requests = transport
            .requests()
            .into_iter()
            .filter(|(index, _)| *index == big_index)
            .count();
        assert_eq!(
            big_requests, 3,
            "the entry must be attempted exactly 3 times"
        );
        assert!(!completed.contains(big_index));
        // The entries that did finish are kept for the next attempt.
        assert!(completed.count() >= 1);
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[tokio::test]
    async fn a_paused_entry_keeps_its_partial_and_resumes_with_range() {
        let (manifest, contents) = zip_fixture("pause-entry");
        let big = files(&manifest)
            .into_iter()
            .find(|item| item.path.ends_with("big.bin"))
            .expect("big entry");
        let big_index = big.index;
        let half = (contents[big_index].len() / 2) as u64;
        let (staging, partial_root) = scripted_staging("pause-entry-staging");
        let mut completed = CompletedEntries::new(manifest.entry_count);
        let progress = Arc::new(ProgressState::new(0, 0));

        // First pass: half the entry, then the pause token fires.
        let transport = ScriptedTransport::new(
            contents.clone(),
            move |index, _offset, _attempt, contents| {
                if index != big_index {
                    return full_entry(contents, index);
                }
                ScriptedResult::Response {
                    status: 200,
                    content_range_start: None,
                    chunks: vec![Ok(contents[index][..half as usize].to_vec())],
                    stall: true,
                }
            },
        );
        let pause = CancellationToken::new();
        let cancel_from_test = pause.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            cancel_from_test.cancel();
        });
        let mut pending: Vec<StreamEntry> = files(&manifest)
            .into_iter()
            .filter(|item| !completed.contains(item.index))
            .collect();
        let result = download_entries(
            &transport,
            "http://test",
            None,
            7,
            9,
            pending,
            &staging,
            &partial_root,
            &mut completed,
            0,
            &CancellationToken::new(),
            &pause,
            &progress,
            StreamTuning {
                stall_timeout: Duration::from_secs(5),
                attempts: 2,
                backoff: Duration::from_millis(1),
            },
            |_, _| {},
        )
        .await;
        assert!(matches!(result, Err(Failure::Paused)), "{result:?}");
        let kept = std::fs::metadata(partial_root.join(format!("{big_index}.part")))
            .expect("partial entry kept")
            .len();
        assert_eq!(kept, half, "the pause must flush the exact byte offset");

        // Second pass: a fresh token resumes at the kept offset.
        let transport = ScriptedTransport::new(
            contents.clone(),
            move |index, offset, _attempt, contents| {
                if index != big_index {
                    return full_entry(contents, index);
                }
                ScriptedResult::Response {
                    status: 206,
                    content_range_start: Some(offset),
                    chunks: vec![Ok(contents[index][offset as usize..].to_vec())],
                    stall: false,
                }
            },
        );
        let pending: Vec<StreamEntry> = files(&manifest)
            .into_iter()
            .filter(|item| !completed.contains(item.index))
            .collect();
        download_entries(
            &transport,
            "http://test",
            None,
            7,
            9,
            pending,
            &staging,
            &partial_root,
            &mut completed,
            0,
            &CancellationToken::new(),
            &CancellationToken::new(),
            &progress,
            fast_tuning(2),
            |_, _| {},
        )
        .await
        .expect("the paused entry must resume");
        assert!(transport.requests().contains(&(big_index, half)));
        assert_eq!(
            std::fs::read(staging.join(&big.path)).unwrap(),
            contents[big_index]
        );
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[test]
    fn checkpoint_plan_adopts_or_restarts_on_a_changed_archive() {
        let default = Path::new("/games/.game.staging-new");
        let previous = Path::new("/games/.game.staging-old");

        let adopted = plan_checkpoint(
            Some(("fingerprint-a", previous)),
            "fingerprint-a",
            true,
            default,
        );
        assert_eq!(adopted.decision, CheckpointDecision::Adopted);
        assert_eq!(adopted.staging, previous);
        assert!(adopted.stale_staging.is_none());

        let changed = plan_checkpoint(
            Some(("fingerprint-a", previous)),
            "fingerprint-b",
            true,
            default,
        );
        assert_eq!(changed.decision, CheckpointDecision::ArchiveChanged);
        assert_eq!(changed.staging, default);
        assert_eq!(changed.stale_staging.as_deref(), Some(previous));

        let gone = plan_checkpoint(
            Some(("fingerprint-a", previous)),
            "fingerprint-a",
            false,
            default,
        );
        assert_eq!(gone.decision, CheckpointDecision::Fresh);
        assert_eq!(gone.staging, default);
        assert!(gone.stale_staging.is_none());

        assert_eq!(
            plan_checkpoint(None, "fingerprint-a", false, default).decision,
            CheckpointDecision::Fresh
        );
    }

    #[test]
    fn content_range_start_is_parsed() {
        assert_eq!(parse_content_range_start("bytes 100-199/200"), Some(100));
        assert_eq!(parse_content_range_start("bytes 0-9/10"), Some(0));
        assert_eq!(parse_content_range_start("bytes */200"), None);
        assert_eq!(parse_content_range_start(""), None);
    }
}
