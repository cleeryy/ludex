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

use std::path::{Path, PathBuf};
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
    // folder is still on disk.
    let (mut completed, staging, adopted, stale_staging) = {
        let connection = state.db.lock().map_err(|_| {
            Failure::Failed("The local database is unavailable; restart Ludex.".to_string())
        })?;
        let previous =
            db::latest_stream_manifest_for_version(&connection, params.game_id, params.version_id)
                .ok()
                .flatten();
        if let Some(record) = previous {
            let stored_staging = PathBuf::from(&record.staging);
            if record.fingerprint == fingerprint && stored_staging.is_dir() {
                let bits = db::get_install_job(&connection, &record.job_id)
                    .ok()
                    .flatten()
                    .and_then(|job| job.stream_completed)
                    .map(|value| CompletedEntries::from_hex(&value))
                    .unwrap_or_default();
                (bits, stored_staging, true, None)
            } else {
                // The archive changed under the same version row (or its
                // staging folder is gone): the old files cannot be trusted.
                (
                    CompletedEntries::new(manifest.entry_count),
                    default_staging.to_path_buf(),
                    false,
                    stored_staging.is_dir().then_some(stored_staging),
                )
            }
        } else {
            (
                CompletedEntries::new(manifest.entry_count),
                default_staging.to_path_buf(),
                false,
                None,
            )
        }
    };
    completed.resize(manifest.entry_count);
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
        job.message = Some(if resumed > 0 {
            format!("Resuming: {resumed}/{files_total} files already installed")
        } else {
            format!("Downloading {files_total} files")
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

    let downloaded = download_entries(
        &state.http,
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
    client: &reqwest::Client,
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
    mut on_complete: F,
) -> Result<(), Failure>
where
    F: FnMut(&StreamEntry, &str),
{
    let throttle = Arc::new(Mutex::new(Throttle::new(bandwidth_kib)));
    let downloads = futures_util::stream::iter(pending)
        .map(|item| {
            let client = client.clone();
            let token = token.map(str::to_string);
            let staging = staging.to_path_buf();
            let partial_root = partial_root.to_path_buf();
            let throttle = Arc::clone(&throttle);
            let cancel = cancel.clone();
            let pause = pause.clone();
            let progress = Arc::clone(progress);
            async move {
                download_entry(
                    &client,
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
#[allow(clippy::too_many_arguments)]
async fn download_entry(
    client: &reqwest::Client,
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

    let mut last_error = String::new();
    for attempt in 0..2 {
        if attempt > 0 {
            let _ = tokio::fs::remove_file(&partial).await;
        }
        match download_entry_once(
            client, &url, token, item, &partial, throttle, cancel, pause, progress,
        )
        .await
        {
            Ok(()) => {
                finalize_entry(&partial, &target, item).await?;
                return Ok(());
            }
            Err(Failure::Failed(message)) => {
                // Only a corrupted/truncated entry is worth a second pass; a
                // network error must reach the caller so the job can resume.
                let retryable = message.contains("checksum") || message.contains("size mismatch");
                if !retryable {
                    return Err(Failure::Failed(message));
                }
                last_error = message;
            }
            Err(other) => return Err(other),
        }
    }
    Err(Failure::Failed(last_error))
}

/// One HTTP pass for an entry, starting at the current `.part` size.
#[allow(clippy::too_many_arguments)]
async fn download_entry_once(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
    item: &StreamEntry,
    partial: &Path,
    throttle: &Arc<Mutex<Option<Throttle>>>,
    cancel: &CancellationToken,
    pause: &CancellationToken,
    progress: &ProgressState,
) -> Result<(), Failure> {
    if item.size == 0 {
        let _ = tokio::fs::write(partial, []).await;
        return Ok(());
    }

    let mut offset = tokio::fs::metadata(partial)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if offset > item.size {
        progress.subtract_bytes(offset);
        let _ = tokio::fs::remove_file(partial).await;
        offset = 0;
    }
    progress.set_current(&item.path);

    let mut restarted = false;
    loop {
        let mut request = client
            .get(url)
            .header(reqwest::header::ACCEPT_ENCODING, "identity");
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if offset > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }

        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(Failure::Cancelled),
            _ = pause.cancelled() => return Err(Failure::Paused),
            response = request.send() => response.map_err(|e| {
                Failure::Failed(format!(
                    "Could not download {}: {e}. Retry the install to resume.",
                    item.path
                ))
            })?,
        };

        let status = response.status();
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
            let start = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split_once('-'))
                .and_then(|(start, _)| start.split_whitespace().last())
                .and_then(|start| start.parse::<u64>().ok());
            if start != Some(offset) {
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
        let mut stream = response.bytes_stream();
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
                chunk = stream.next() => chunk,
            };
            match chunk {
                Some(Ok(bytes)) => {
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
                Some(Err(error)) => {
                    let _ = writer.flush().await;
                    return Err(Failure::Failed(format!(
                        "The download of {} was interrupted after {written} bytes: {error}. Retry the install to resume.",
                        item.path
                    )));
                }
                None => break,
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

        let size = tokio::fs::metadata(partial)
            .await
            .map(|metadata| metadata.len())
            .unwrap_or(0);
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
        let pending: Vec<StreamEntry> = files(manifest)
            .into_iter()
            .filter(|item| !completed.contains(item.index))
            .collect();
        let _ = contents;
        download_entries(
            &reqwest::Client::new(),
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
        let result = run(
            &failing,
            &manifest,
            &contents,
            &mut completed,
            &staging,
            &partial_root,
            &progress,
            &CancellationToken::new(),
            &CancellationToken::new(),
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

        let result = run(
            &server,
            &manifest,
            &[],
            &mut completed,
            &staging,
            &partial_root,
            &progress,
            &CancellationToken::new(),
            &CancellationToken::new(),
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
}
