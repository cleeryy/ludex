//! File-manifest transport (#231): the server extracts an archive once into a
//! cache, publishes a per-file manifest, and the client downloads the files in
//! parallel, verifies each SHA-256, and never stores or extracts an archive.
//!
//! The classic archive+extract path is always available as the fallback
//! ([`crate::settings::DownloadPolicy::Classic`]) and every failure here falls
//! back to it once with a visible message.
//!
//! The module owns the transport-independent engine so it can be exercised with
//! injected manifest and file transports:
//!
//! * [`plan`] validates a manifest and turns it into a download plan (always
//!   before touching the disk, so a malicious path never escapes staging);
//! * [`prepare`] fetches the manifest, optionally asks the server to build the
//!   cache, and reports "unavailable" so the caller can fall back;
//! * [`install`] downloads the plan with a bounded worker pool, per-file
//!   `Range` resume, per-file SHA-256 and a temp-file + fsync + rename commit.
//!
//! No archive is written anywhere: the only files this module creates are the
//! game files themselves, their `.part` temporaries under
//! [`PARTIAL_DIR`], and the empty directories the manifest lists.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

/// Internal folder that holds the in-flight `.part` files of one file-manifest
/// install. A manifest entry may not use it as its first component.
pub(crate) const PARTIAL_DIR: &str = ".ludex-parts";
/// Largest worker pool the engine will ever open for one job.
pub(crate) const MAX_WORKERS: usize = 8;
/// Smallest pool: a single stream still works, it is only slower.
pub(crate) const MIN_WORKERS: usize = 1;
/// Default pool, used when the server advertises nothing.
pub(crate) const DEFAULT_WORKERS: usize = 4;
/// Sanity caps. The server enforces its own; these only stop a malformed
/// manifest from exhausting the client.
pub(crate) const MAX_FILES: usize = 250_000;
pub(crate) const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024 * 1024;

/// Read chunk when re-hashing the prefix of a resumed `.part`.
const REWIND_CHUNK: usize = 1024 * 1024;

/// `GET /api/versions/{id}/manifest` payload (#231).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Manifest {
    pub version_id: i64,
    /// Identity of the archive the file list was derived from. A different
    /// value means a different install: the checkpoint is discarded, never
    /// mixed with the new files.
    pub archive_sha256: String,
    pub status: String,
    pub generated_at: String,
    pub total_bytes: u64,
    pub file_count: u64,
    pub files: Vec<ManifestFile>,
    /// Empty directories the archive carried, as relative paths.
    #[serde(default)]
    pub directories: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ManifestFile {
    /// Relative path, `/` separated.
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// Unix mode bits, when the archive recorded them.
    #[serde(default)]
    pub mode: Option<u32>,
    #[serde(default)]
    pub executable: bool,
}

impl Manifest {
    /// True when the manifest describes a servable cache.
    pub(crate) fn is_ready(&self) -> bool {
        self.status.eq_ignore_ascii_case("ready")
    }
}

/// One validated file of a [`Plan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedFile {
    /// Position in the manifest. The `.part` name derives from it, so a retry
    /// finds the exact same temporary file.
    pub index: usize,
    /// Relative path with `/` separators, as sent to `GET .../files/{path}`.
    pub relative: String,
    /// Normalized relative path, safe to join under the staging folder.
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
    pub mode: Option<u32>,
    pub executable: bool,
}

/// A manifest that passed every validation, ready to be written to disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    pub files: Vec<PlannedFile>,
    pub directories: Vec<PathBuf>,
    pub total_bytes: u64,
}

/// Why a manifest cannot be installed. Every variant is a fallback reason: the
/// caller downloads the classic archive instead of failing the install (#231).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlanError {
    /// The server has no ready extracted cache for this version.
    NotReady(String),
    /// An entry would escape the staging folder or collide with another.
    Unsafe(String),
    /// `fileCount`/`totalBytes` disagree with the entries: the manifest is
    /// inconsistent, so no file can be trusted.
    Inconsistent(String),
    /// The body was not a manifest at all.
    Malformed(String),
}

impl PlanError {
    /// Message shown to the user before falling back to the classic path.
    pub(crate) fn message(&self) -> String {
        match self {
            PlanError::NotReady(reason) => format!("the file manifest is not ready ({reason})"),
            PlanError::Unsafe(reason) => format!("the file manifest is unsafe ({reason})"),
            PlanError::Inconsistent(reason) => {
                format!("the file manifest is inconsistent ({reason})")
            }
            PlanError::Malformed(reason) => format!("the file manifest is invalid ({reason})"),
        }
    }
}

/// Parse a manifest body and reject anything that is not a servable list.
pub(crate) fn manifest_from_json(body: &str) -> Result<Manifest, PlanError> {
    let manifest: Manifest =
        serde_json::from_str(body).map_err(|error| PlanError::Malformed(error.to_string()))?;
    if !manifest.is_ready() {
        return Err(PlanError::NotReady(format!(
            "status \"{}\"",
            manifest.status
        )));
    }
    Ok(manifest)
}

/// Normalize one manifest path, rejecting anything that could escape staging.
///
/// Accepts `/` and `\` separators (a Windows-built manifest), drops `.`
/// components, and refuses empty, absolute, drive-prefixed and `..` paths.
pub(crate) fn safe_relative(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut path = PathBuf::new();
    for part in trimmed.split(['/', '\\']) {
        match part {
            "" | "." => continue,
            ".." => return None,
            // A drive letter or a UNC prefix arrives as `C:` in the first
            // component; a colon is never valid in a relative game path.
            other if other.contains(':') => return None,
            other => path.push(other),
        }
    }
    if path.as_os_str().is_empty() {
        return None;
    }
    // Belt and braces: the built path must only contain plain components.
    if path.components().any(|component| {
        !matches!(component, Component::Normal(_)) || component.as_os_str() == PARTIAL_DIR
    }) {
        return None;
    }
    Some(path)
}

/// Validate a ready manifest and build its plan without touching the disk.
pub(crate) fn plan(manifest: &Manifest) -> Result<Plan, PlanError> {
    if manifest.files.len() > MAX_FILES {
        return Err(PlanError::Inconsistent(format!(
            "{} files is above the {MAX_FILES} file cap",
            manifest.files.len()
        )));
    }
    if manifest.file_count != manifest.files.len() as u64 {
        return Err(PlanError::Inconsistent(format!(
            "fileCount is {} but the manifest lists {} files",
            manifest.file_count,
            manifest.files.len()
        )));
    }

    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut files: Vec<PlannedFile> = Vec::with_capacity(manifest.files.len());
    let mut total_bytes: u64 = 0;
    for (index, file) in manifest.files.iter().enumerate() {
        let Some(path) = safe_relative(&file.path) else {
            return Err(PlanError::Unsafe(format!("path \"{}\"", file.path)));
        };
        if !seen.insert(path.clone()) {
            return Err(PlanError::Unsafe(format!(
                "duplicate path \"{}\"",
                file.path
            )));
        }
        if file.size > MAX_FILE_BYTES {
            return Err(PlanError::Inconsistent(format!(
                "\"{}\" is larger than the {MAX_FILE_BYTES} byte cap",
                file.path
            )));
        }
        if !is_sha256(&file.sha256) {
            return Err(PlanError::Inconsistent(format!(
                "\"{}\" has no valid SHA-256",
                file.path
            )));
        }
        total_bytes = total_bytes.saturating_add(file.size);
        files.push(PlannedFile {
            index,
            relative: path_to_relative(&path),
            path,
            size: file.size,
            sha256: file.sha256.to_ascii_lowercase(),
            mode: file.mode.map(|mode| mode & 0o777),
            executable: file.executable,
        });
    }
    if total_bytes != manifest.total_bytes {
        return Err(PlanError::Inconsistent(format!(
            "totalBytes is {} but the files add up to {total_bytes}",
            manifest.total_bytes
        )));
    }

    let mut directories: Vec<PathBuf> = Vec::with_capacity(manifest.directories.len());
    for raw in &manifest.directories {
        let Some(path) = safe_relative(raw) else {
            return Err(PlanError::Unsafe(format!("directory \"{raw}\"")));
        };
        if seen.contains(&path) {
            return Err(PlanError::Unsafe(format!(
                "directory \"{raw}\" collides with a file"
            )));
        }
        directories.push(path);
    }

    Ok(Plan {
        files,
        directories,
        total_bytes,
    })
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn path_to_relative(path: &Path) -> String {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

/// Transport failure of one manifest or file request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransportError {
    pub status: Option<u16>,
    pub message: String,
}

impl TransportError {
    pub(crate) fn new(status: Option<u16>, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    /// A transport that could not answer at all (connection refused, TLS).
    pub(crate) fn offline(message: impl Into<String>) -> Self {
        Self::new(None, message)
    }

    /// True when the server has no extracted cache to serve: the documented
    /// `409 extracted_cache_unavailable`, or an older server without the route.
    pub(crate) fn is_cache_miss(&self) -> bool {
        matches!(self.status, Some(404 | 405 | 409 | 410 | 501 | 503))
    }
}

/// One response body of a file download.
#[allow(async_fn_in_trait)]
pub(crate) trait FileBody {
    fn status(&self) -> u16;
    fn content_length(&self) -> Option<u64>;
    /// First byte of a `206` response; `None` when the header is absent.
    fn content_range_start(&self) -> Option<u64> {
        None
    }
    /// `X-Ludex-File-Sha256` echoed by the server, when present.
    fn file_sha256(&self) -> Option<String> {
        None
    }
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, String>;
}

/// Opens one manifest or one file (`range_start` = resume offset).
#[allow(async_fn_in_trait)]
pub(crate) trait FileTransport: Send + Sync {
    type Body: FileBody;

    async fn open(
        &self,
        path: &str,
        range_start: Option<u64>,
    ) -> Result<Self::Body, TransportError>;
}

/// Fetches the manifest and can warm the server-side cache (#231).
#[allow(async_fn_in_trait)]
pub(crate) trait ManifestTransport: Send + Sync {
    /// `GET /api/versions/{id}/manifest?token=`.
    async fn fetch(&self) -> Result<String, TransportError>;

    /// `POST /api/admin/versions/{id}/extract`, used only when the user chose
    /// the file transport explicitly. The default reports "unsupported".
    async fn request_extract(&self) -> Result<String, TransportError> {
        Err(TransportError::new(None, "extract is not supported"))
    }
}

/// Tuning of one file-manifest run.
#[derive(Debug, Clone)]
pub(crate) struct Tuning {
    /// Worker pool size, clamped to [`MIN_WORKERS`]..=[`MAX_WORKERS`].
    pub parallel: usize,
    /// Attempts per file before the job fails. A checksum mismatch and a
    /// truncated body each consume one attempt.
    pub attempts_per_file: u32,
    /// Abort a silent body read after this long and resume with `Range`.
    pub idle_timeout: Duration,
    /// Minimum interval between two progress emissions of the same worker.
    pub progress_interval: Duration,
    /// Ask the server to build the cache first (explicit `files` mode only).
    pub warm_cache: bool,
    /// How long to wait for the server cache to become ready.
    pub cache_wait: Duration,
    /// Poll interval while the cache extracts.
    pub cache_poll: Duration,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            parallel: DEFAULT_WORKERS,
            attempts_per_file: 3,
            idle_timeout: Duration::from_secs(30),
            progress_interval: Duration::from_millis(250),
            warm_cache: false,
            cache_wait: Duration::from_secs(120),
            cache_poll: Duration::from_secs(2),
        }
    }
}

impl Tuning {
    pub(crate) fn workers(&self) -> usize {
        self.parallel.clamp(MIN_WORKERS, MAX_WORKERS)
    }
}

/// Fetch a ready manifest, optionally warming the server cache first.
///
/// `409 extracted_cache_unavailable` (or a server without the route) is not an
/// error the install should die on: it is a fallback reason. Only an explicit
/// `files` choice asks the server to extract, and only that path waits.
pub(crate) async fn prepare<T: ManifestTransport>(
    transport: &T,
    tuning: &Tuning,
) -> Result<Manifest, PlanError> {
    let first = transport.fetch().await;
    let mut body = match first {
        Ok(body) => Some(body),
        Err(error) => {
            if !tuning.warm_cache {
                return Err(PlanError::NotReady(error.message));
            }
            tracing::debug!(error = %error.message, "the manifest route is not ready yet");
            None
        }
    };
    // A ready manifest is the common case: only ask for an extraction when the
    // first fetch did not produce one.
    let ready = body
        .as_deref()
        .and_then(|body| manifest_from_json(body).ok());
    if tuning.warm_cache && ready.is_none() {
        if let Err(error) = transport.request_extract().await {
            // The cache cannot be warmed (disabled, forbidden, offline), so
            // polling for a manifest that will never appear would only stall
            // the install for the whole cache timeout (#235).
            return Err(PlanError::NotReady(error.message));
        }
        let deadline = Instant::now() + tuning.cache_wait;
        loop {
            match transport.fetch().await {
                Ok(candidate) => {
                    let ready_now = manifest_from_json(&candidate).is_ok();
                    body = Some(candidate);
                    if ready_now {
                        break;
                    }
                }
                Err(error) => {
                    if Instant::now() >= deadline {
                        return Err(PlanError::NotReady(error.message));
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(PlanError::NotReady(
                    "the server did not finish extracting before the timeout".to_string(),
                ));
            }
            tokio::time::sleep(tuning.cache_poll).await;
        }
    }
    let body = body.ok_or_else(|| {
        PlanError::NotReady("the server did not return a file manifest".to_string())
    })?;
    let manifest = manifest_from_json(&body)?;
    plan(&manifest)?;
    Ok(manifest)
}

/// Why a file-manifest install stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManifestFailure {
    Cancelled,
    Paused,
    /// The path cannot work (cache gone, file 404, unsafe manifest): fall back
    /// to the classic archive once instead of failing the job.
    Fallback(String),
    /// A real failure after the files started (checksum, disk full): the job
    /// fails and keeps its checkpoint for a later resume.
    Failed(String),
}

/// Per-run checkpoint so a restart continues the same job (#231).
///
/// Implemented over SQLite by the desktop ([`crate::db`]); tests use an
/// in-memory map.
pub(crate) trait FileCheckpoints: Send + Sync {
    /// Adopt, reject or create the checkpoint for this archive.
    fn attest(
        &self,
        archive_sha256: &str,
        total_bytes: u64,
        file_count: u64,
    ) -> Result<Resume, String>;
    /// Completed relative paths with the size recorded at commit time.
    fn completed(&self) -> Result<Vec<(String, u64)>, String>;
    /// Record one committed file. Called once per file, after its rename.
    fn record(&self, relative: &str, size: u64, sha256: &str) -> Result<(), String>;
    /// Drop one entry whose file is no longer on disk.
    fn forget(&self, relative: &str) -> Result<(), String> {
        let _ = relative;
        Ok(())
    }
    /// Forget every file of this job (a changed archive, or a fresh start).
    fn clear(&self) -> Result<(), String>;
}

/// Result of adopting a checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resume {
    /// No previous manifest for this job.
    Fresh,
    /// Same archive: files already on disk are kept.
    Adopted,
    /// A different archive: the old checkpoint and its files must go.
    Changed,
}

/// Live progress of one file-manifest install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Progress {
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub files_done: u32,
    pub files_total: u32,
    /// Smoothed transfer rate of the worker pool.
    pub speed_bps: u64,
    /// File currently being written, relative to the staging folder.
    pub current: Option<String>,
    /// Files already on disk when this attempt started.
    pub resumed_files: u32,
    /// True while the per-file SHA-256 is being checked (the #223 phase).
    pub verifying: bool,
    pub message: String,
}

/// What one completed run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub bytes: u64,
    pub files: usize,
    pub resumed_files: usize,
    pub archive_sha256: String,
}

/// Shared mutable state of one run.
struct Run<'a> {
    staging: &'a Path,
    parts: PathBuf,
    cancel: &'a CancellationToken,
    pause: &'a CancellationToken,
    checkpoints: &'a dyn FileCheckpoints,
    progress: &'a (dyn Fn(Progress) + Send + Sync),
    tuning: &'a Tuning,
    bytes_done: AtomicU64,
    files_done: AtomicU32,
    resumed_files: u32,
    files_total: u32,
    bytes_total: u64,
    stop: AtomicBool,
    current: Mutex<String>,
    last_emit: Mutex<Instant>,
    /// `(when, bytes)` of the previous rate sample, plus the last computed
    /// value so a throttled call still reports a rate (#231).
    speed_sample: Mutex<(Instant, u64)>,
    speed_bps: AtomicU64,
}

impl Run<'_> {
    fn stop(&self) -> Result<(), ManifestFailure> {
        if self.cancel.is_cancelled() {
            return Err(ManifestFailure::Cancelled);
        }
        if self.pause.is_cancelled() {
            return Err(ManifestFailure::Paused);
        }
        if self.stop.load(Ordering::Relaxed) {
            return Err(ManifestFailure::Failed(
                "another file of this install failed".to_string(),
            ));
        }
        Ok(())
    }

    fn emit(&self, current: Option<&str>, message: &str, verifying: bool, force: bool) {
        let now = Instant::now();
        {
            let mut last = match self.last_emit.lock() {
                Ok(last) => last,
                Err(poisoned) => poisoned.into_inner(),
            };
            if !force && now.duration_since(*last) < self.tuning.progress_interval {
                return;
            }
            *last = now;
        }
        let done = self
            .bytes_done
            .load(Ordering::Relaxed)
            .min(self.bytes_total);
        {
            let mut sample = match self.speed_sample.lock() {
                Ok(sample) => sample,
                Err(poisoned) => poisoned.into_inner(),
            };
            let elapsed = now.duration_since(sample.0);
            if elapsed >= Duration::from_millis(200) {
                let delta = done.saturating_sub(sample.1);
                let rate = (delta as f64 / elapsed.as_secs_f64().max(0.001)) as u64;
                self.speed_bps.store(rate, Ordering::Relaxed);
                *sample = (now, done);
            }
        }
        (self.progress)(Progress {
            bytes_done: done,
            bytes_total: self.bytes_total,
            files_done: self.files_done.load(Ordering::Relaxed),
            files_total: self.files_total,
            speed_bps: self.speed_bps.load(Ordering::Relaxed),
            current: current
                .map(str::to_string)
                .or_else(|| self.current.lock().ok().map(|value| value.clone())),
            resumed_files: self.resumed_files,
            verifying,
            message: message.to_string(),
        });
    }

    fn set_current(&self, relative: &str) {
        if let Ok(mut current) = self.current.lock() {
            current.clear();
            current.push_str(relative);
        }
    }

    fn count_bytes(&self, bytes: u64) {
        self.bytes_done.fetch_add(bytes, Ordering::Relaxed);
    }

    fn subtract_bytes(&self, bytes: u64) {
        let _ = self
            .bytes_done
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_sub(bytes))
            });
    }

    fn complete_file(&self) {
        self.files_done.fetch_add(1, Ordering::Relaxed);
    }
}

/// Download a validated plan into `staging`.
///
/// The caller resets `staging` when [`Resume::Changed`] is reported, commits the
/// folder with its usual staging → install-folder move, and keeps the same job
/// id so the checkpoints stay valid across a restart.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn install<T: FileTransport>(
    plan: &Plan,
    archive_sha256: &str,
    staging: &Path,
    transport: &T,
    checkpoints: &dyn FileCheckpoints,
    cancel: &CancellationToken,
    pause: &CancellationToken,
    tuning: &Tuning,
    progress: &(dyn Fn(Progress) + Send + Sync),
) -> Result<Outcome, ManifestFailure> {
    let resume = checkpoints
        .attest(archive_sha256, plan.total_bytes, plan.files.len() as u64)
        .map_err(ManifestFailure::Failed)?;
    if resume == Resume::Changed {
        // Never mix the files of two archives: drop the checkpoint and every
        // file this transport wrote in a previous attempt.
        tracing::warn!(
            staging = %staging.display(),
            "the archive changed on the server; restarting the file manifest install from zero"
        );
        checkpoints.clear().map_err(ManifestFailure::Failed)?;
    }
    if resume != Resume::Adopted && tokio::fs::try_exists(staging).await.unwrap_or(false) {
        tokio::fs::remove_dir_all(staging).await.map_err(|error| {
            ManifestFailure::Failed(format!(
                "Could not clear the staging folder {}: {error}",
                staging.display()
            ))
        })?;
    }
    tokio::fs::create_dir_all(staging).await.map_err(|error| {
        ManifestFailure::Failed(format!(
            "Could not create the staging folder {}: {error}",
            staging.display()
        ))
    })?;
    let parts = staging.join(PARTIAL_DIR);
    if resume == Resume::Adopted {
        if tokio::fs::try_exists(&parts).await.unwrap_or(false) {
            // Orphaned `.part` files are still resumable; only a fresh start
            // clears them.
            cleanup_parts(&parts, plan).await;
        }
    } else if tokio::fs::try_exists(&parts).await.unwrap_or(false) {
        let _ = tokio::fs::remove_dir_all(&parts).await;
    }
    tokio::fs::create_dir_all(&parts).await.map_err(|error| {
        ManifestFailure::Failed(format!(
            "Could not create the partial folder {}: {error}",
            parts.display()
        ))
    })?;

    // Adopt the files already on disk. A file that shrank, grew or vanished is
    // downloaded again: a size mismatch is enough to distrust it, and the full
    // SHA-256 is proven again by every fresh download.
    let mut done: HashSet<String> = HashSet::new();
    let mut resumed_bytes = 0_u64;
    let mut resumed_files = 0_u32;
    if resume == Resume::Adopted {
        let recorded = checkpoints.completed().map_err(ManifestFailure::Failed)?;
        for (relative, size) in recorded {
            let Some(file) = plan
                .files
                .iter()
                .find(|file| file.relative == relative && file.size == size)
            else {
                checkpoints
                    .forget(&relative)
                    .map_err(ManifestFailure::Failed)?;
                continue;
            };
            let target = staging.join(&file.path);
            let on_disk = tokio::fs::metadata(&target)
                .await
                .map(|metadata| metadata.is_file() && metadata.len() == size)
                .unwrap_or(false);
            if on_disk {
                done.insert(file.relative.clone());
                resumed_bytes = resumed_bytes.saturating_add(size);
                resumed_files += 1;
            } else {
                checkpoints
                    .forget(&relative)
                    .map_err(ManifestFailure::Failed)?;
            }
        }
    }

    let bytes_done = resumed_bytes
        + partial_bytes(&parts, plan, &done)
            .await
            .min(plan.total_bytes.saturating_sub(resumed_bytes));
    let run = Run {
        staging,
        parts: parts.clone(),
        cancel,
        pause,
        checkpoints,
        progress,
        tuning,
        bytes_done: AtomicU64::new(bytes_done),
        files_done: AtomicU32::new(resumed_files),
        resumed_files,
        files_total: plan.files.len() as u32,
        bytes_total: plan.total_bytes,
        stop: AtomicBool::new(false),
        current: Mutex::new(String::new()),
        last_emit: Mutex::new(Instant::now() - tuning.progress_interval),
        speed_sample: Mutex::new((Instant::now(), bytes_done)),
        speed_bps: AtomicU64::new(0),
    };
    run.emit(None, "Downloading files", false, true);

    let queue: Vec<&PlannedFile> = plan
        .files
        .iter()
        .filter(|file| !done.contains(&file.relative))
        .collect();
    let next = AtomicUsize::new(0);
    let workers = tuning.workers().min(queue.len().max(1));

    let results = futures_util::stream::iter(0..workers)
        .map(|_| worker(&run, &queue, &next, transport))
        .buffer_unordered(workers);
    let mut failure: Option<ManifestFailure> = None;
    futures_util::pin_mut!(results);
    while let Some(result) = results.next().await {
        if let Err(error) = result {
            // Stop the other workers from starting a new file, but let the
            // in-flight ones finish their chunk so a pause keeps a clean
            // `.part` at a known offset.
            run.stop.store(true, Ordering::Relaxed);
            if failure.is_none() {
                failure = Some(error);
            }
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    cancel_or_pause(cancel, pause)?;

    // Empty directories are part of the payload: create them once every file
    // is in place so a parent is never replaced by `create_dir_all` racing a
    // file write.
    for directory in &plan.directories {
        let target = staging.join(directory);
        if let Err(error) = tokio::fs::create_dir_all(&target).await {
            return Err(ManifestFailure::Failed(format!(
                "Could not create the folder {}: {error}",
                target.display()
            )));
        }
    }
    if tokio::fs::try_exists(&parts).await.unwrap_or(false) {
        let _ = tokio::fs::remove_dir_all(&parts).await;
    }

    Ok(Outcome {
        bytes: plan.total_bytes,
        files: plan.files.len(),
        resumed_files: resumed_files as usize,
        archive_sha256: archive_sha256.to_string(),
    })
}

fn cancel_or_pause(
    cancel: &CancellationToken,
    pause: &CancellationToken,
) -> Result<(), ManifestFailure> {
    if cancel.is_cancelled() {
        return Err(ManifestFailure::Cancelled);
    }
    if pause.is_cancelled() {
        return Err(ManifestFailure::Paused);
    }
    Ok(())
}

/// One worker: pull the next file until the queue or the run ends.
async fn worker<T: FileTransport>(
    run: &Run<'_>,
    queue: &[&PlannedFile],
    next: &AtomicUsize,
    transport: &T,
) -> Result<(), ManifestFailure> {
    loop {
        run.stop()?;
        let index = next.fetch_add(1, Ordering::Relaxed);
        let Some(file) = queue.get(index) else {
            return Ok(());
        };
        if let Err(error) = fetch_file(run, file, transport).await {
            run.stop.store(true, Ordering::Relaxed);
            return Err(error);
        }
    }
}

/// Download, verify and commit one file.
async fn fetch_file<T: FileTransport>(
    run: &Run<'_>,
    file: &PlannedFile,
    transport: &T,
) -> Result<(), ManifestFailure> {
    let target = run.staging.join(&file.path);
    let partial = run.parts.join(format!("{}.part", file.index));
    run.set_current(&file.relative);

    if let Ok(metadata) = tokio::fs::metadata(&target).await {
        if metadata.is_file() && metadata.len() == file.size {
            // A previous attempt committed it but the checkpoint row was lost
            // (crash between rename and record): still complete.
            run.checkpoints
                .record(&file.relative, file.size, &file.sha256)
                .map_err(ManifestFailure::Failed)?;
            run.complete_file();
            run.emit(Some(&file.relative), "Verified", false, true);
            return Ok(());
        }
        let _ = tokio::fs::remove_file(&target).await;
    }

    let mut attempt = 0_u32;
    let mut restart_from_zero = false;
    loop {
        run.stop()?;
        attempt += 1;
        if restart_from_zero {
            let _ = tokio::fs::remove_file(&partial).await;
        }
        let offset = tokio::fs::metadata(&partial)
            .await
            .map(|metadata| metadata.len())
            .unwrap_or(0)
            .min(file.size);
        if offset == file.size && file.size > 0 {
            // The whole body is already on disk: verify it instead of asking
            // for a zero-length range.
            return verify_partial(run, file, &partial, &target, offset, attempt)
                .await
                .map(|_| ());
        }

        // Re-hash the prefix before appending, so the digest still covers the
        // whole file. A local re-read is cheaper than a re-download and keeps
        // "resume" and "verify" the same code path.
        let mut hasher = Sha256::new();
        if offset > 0 {
            hash_prefix(&partial, offset, &mut hasher)
                .await
                .map_err(ManifestFailure::Failed)?;
        }

        let mut body = match transport
            .open(&file.relative, (offset > 0).then_some(offset))
            .await
        {
            Ok(body) => body,
            Err(error) => {
                if error.is_cache_miss() {
                    return Err(ManifestFailure::Fallback(format!(
                        "the server cannot serve \"{}\" ({})",
                        file.relative, error.message
                    )));
                }
                if attempt >= run.tuning.attempts_per_file {
                    return Err(ManifestFailure::Failed(format!(
                        "Could not download \"{}\": {}",
                        file.relative, error.message
                    )));
                }
                sleep_between(run, attempt).await?;
                continue;
            }
        };

        let status = body.status();
        // The server echoes the digest of the file in its cache. A different
        // value means the manifest and the cache disagree: never write those
        // bytes, the classic archive is the honest answer.
        if let Some(served) = body.file_sha256() {
            if !served.eq_ignore_ascii_case(&file.sha256) {
                return Err(ManifestFailure::Fallback(format!(
                    "the server's copy of \"{}\" does not match the manifest",
                    file.relative
                )));
            }
        }
        if let Some(length) = body.content_length() {
            if offset.saturating_add(length) > file.size {
                return Err(ManifestFailure::Fallback(format!(
                    "the server's copy of \"{}\" is larger than the manifest declares",
                    file.relative
                )));
            }
        }
        if !(200..300).contains(&status) {
            // A stale `.part` may be longer than the file the server now has:
            // drop it and ask from zero once.
            if status == 416 && offset > 0 {
                let _ = tokio::fs::remove_file(&partial).await;
                if attempt >= run.tuning.attempts_per_file {
                    return Err(ManifestFailure::Failed(format!(
                        "\"{}\" kept returning 416",
                        file.relative
                    )));
                }
                continue;
            }
            if matches!(status, 404 | 409 | 410 | 501 | 503) {
                return Err(ManifestFailure::Fallback(format!(
                    "the server cannot serve \"{}\" (HTTP {status})",
                    file.relative
                )));
            }
            if attempt >= run.tuning.attempts_per_file {
                return Err(ManifestFailure::Failed(format!(
                    "Could not download \"{}\": HTTP {status}",
                    file.relative
                )));
            }
            sleep_between(run, attempt).await?;
            continue;
        }

        // A 200 answer to a ranged request means the server ignored the range:
        // start over instead of appending the whole body twice.
        let mut written = offset;
        let restarted = offset > 0 && body.content_range_start().is_none() && status == 200;
        if restarted {
            written = 0;
            hasher = Sha256::new();
        }

        let mut out = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&partial)
            .await
            .map_err(|error| {
                ManifestFailure::Failed(format!("Could not open {}: {error}", partial.display()))
            })?;
        if restarted {
            out.set_len(0).await.map_err(|error| {
                ManifestFailure::Failed(format!("Could not reset {}: {error}", partial.display()))
            })?;
            run.subtract_bytes(offset);
        }

        let mut read_error: Option<String> = None;
        loop {
            if let Err(error) = run.stop() {
                // The bytes already written are a valid resume point (Range).
                if let Err(flush_error) = out.flush().await {
                    tracing::warn!(error = %flush_error, "could not flush a paused file download");
                }
                return Err(error);
            }
            let next = tokio::time::timeout(run.tuning.idle_timeout, body.next_chunk()).await;
            let chunk = match next {
                Err(_) => {
                    read_error = Some("the connection went silent".to_string());
                    break;
                }
                Ok(Err(error)) => {
                    read_error = Some(error);
                    break;
                }
                Ok(Ok(None)) => break,
                Ok(Ok(Some(bytes))) => bytes,
            };
            if chunk.is_empty() {
                continue;
            }
            out.write_all(&chunk).await.map_err(|error| {
                ManifestFailure::Failed(format!("Could not write {}: {error}", partial.display()))
            })?;
            hasher.update(&chunk);
            written = written.saturating_add(chunk.len() as u64);
            if written > file.size {
                // The server is serving a different (larger) file: never keep
                // it, and never commit it.
                read_error = Some("the server sent more bytes than the manifest declared".into());
                break;
            }
            run.count_bytes(chunk.len() as u64);
            run.emit(Some(&file.relative), "Downloading files", false, false);
        }

        if let Err(error) = out.flush().await {
            return Err(ManifestFailure::Failed(format!(
                "Could not flush {}: {error}",
                partial.display()
            )));
        }
        drop(out);
        run.emit(Some(&file.relative), "Downloading files", false, true);

        if let Some(error) = read_error {
            if written < file.size {
                if attempt >= run.tuning.attempts_per_file {
                    return Err(ManifestFailure::Failed(format!(
                        "Could not download \"{}\": {error} ({written} of {} bytes)",
                        file.relative, file.size
                    )));
                }
                sleep_between(run, attempt).await?;
                continue;
            }
            // Every declared byte arrived: the transport's footer error is
            // irrelevant and the digest below still covers the whole body.
        }

        if written < file.size {
            if attempt >= run.tuning.attempts_per_file {
                return Err(ManifestFailure::Failed(format!(
                    "\"{}\" is incomplete after {attempt} attempt(s): {written} of {} bytes",
                    file.relative, file.size
                )));
            }
            sleep_between(run, attempt).await?;
            continue;
        }

        // The streaming hasher already covers every byte written by this
        // attempt (a resumed prefix was hashed before the first append), so the
        // verification costs no second read of the file (#231/#223).
        run.emit(Some(&file.relative), "Verifying files", true, true);
        let digest = hex::encode(hasher.finalize());
        if digest == file.sha256 {
            commit(run, file, &partial, &target, &digest).await?;
            return Ok(());
        }
        tracing::warn!(
            path = %file.relative,
            expected = %file.sha256,
            actual = %digest,
            "file failed SHA-256 verification; downloading it again"
        );
        let _ = tokio::fs::remove_file(&partial).await;
        run.subtract_bytes(file.size);
        restart_from_zero = true;
        if attempt >= run.tuning.attempts_per_file {
            return Err(ManifestFailure::Failed(format!(
                "\"{}\" failed verification after {attempt} attempt(s)",
                file.relative
            )));
        }
    }
}

/// Verify a `.part` that already holds the whole file (a crash left the body
/// on disk but the rename never ran).
async fn verify_partial(
    run: &Run<'_>,
    file: &PlannedFile,
    partial: &Path,
    target: &Path,
    written: u64,
    attempt: u32,
) -> Result<Option<String>, ManifestFailure> {
    if written != file.size {
        return Ok(None);
    }
    run.emit(Some(&file.relative), "Verifying files", true, true);
    let mut hasher = Sha256::new();
    hash_prefix(partial, file.size, &mut hasher)
        .await
        .map_err(ManifestFailure::Failed)?;
    let digest = hex::encode(hasher.finalize());
    if digest != file.sha256 {
        tracing::warn!(
            path = %file.relative,
            expected = %file.sha256,
            actual = %digest,
            "file failed SHA-256 verification; downloading it again"
        );
        let _ = tokio::fs::remove_file(partial).await;
        run.subtract_bytes(file.size);
        if attempt >= run.tuning.attempts_per_file {
            return Err(ManifestFailure::Failed(format!(
                "\"{}\" failed verification after {attempt} attempt(s)",
                file.relative
            )));
        }
        return Ok(None);
    }
    commit(run, file, partial, target, &digest).await?;
    Ok(Some(digest))
}

async fn commit(
    run: &Run<'_>,
    file: &PlannedFile,
    partial: &Path,
    target: &Path,
    digest: &str,
) -> Result<(), ManifestFailure> {
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| {
            ManifestFailure::Failed(format!(
                "Could not create the folder {}: {error}",
                parent.display()
            ))
        })?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut mode = file.mode.unwrap_or(0o644) & 0o777;
        if file.executable && mode & 0o111 == 0 {
            mode |= 0o755;
        }
        if let Err(error) = std::fs::set_permissions(partial, std::fs::Permissions::from_mode(mode))
        {
            tracing::warn!(path = %partial.display(), error = %error, "could not set the file mode");
        }
    }
    // fsync before the rename: a crash must never leave a renamed file whose
    // bytes are not on disk.
    match tokio::fs::OpenOptions::new().read(true).open(partial).await {
        Ok(file) => {
            if let Err(error) = file.sync_all().await {
                tracing::warn!(path = %partial.display(), error = %error, "could not fsync a file");
            }
        }
        Err(error) => {
            tracing::warn!(path = %partial.display(), error = %error, "could not reopen a file to fsync it");
        }
    }
    tokio::fs::rename(partial, target).await.map_err(|error| {
        ManifestFailure::Failed(format!(
            "Could not move {} into place: {error}",
            target.display()
        ))
    })?;
    run.checkpoints
        .record(&file.relative, file.size, digest)
        .map_err(ManifestFailure::Failed)?;
    run.complete_file();
    run.emit(Some(&file.relative), "Downloading files", false, true);
    Ok(())
}

/// Hash `length` bytes of `path` into `hasher`.
async fn hash_prefix(path: &Path, length: u64, hasher: &mut Sha256) -> Result<(), String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    let mut buffer = vec![0_u8; REWIND_CHUNK];
    let mut remaining = length;
    while remaining > 0 {
        let want = buffer.len().min(remaining as usize);
        let read = file
            .read(&mut buffer[..want])
            .await
            .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
        if read == 0 {
            return Err(format!(
                "{} ended after {} of {length} bytes",
                path.display(),
                length - remaining
            ));
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    Ok(())
}

/// Bytes already present in the `.part` files, capped at the plan size.
async fn partial_bytes(parts: &Path, plan: &Plan, done: &HashSet<String>) -> u64 {
    let mut total = 0_u64;
    for file in plan
        .files
        .iter()
        .filter(|file| !done.contains(&file.relative))
    {
        let partial = parts.join(format!("{}.part", file.index));
        if let Ok(metadata) = tokio::fs::metadata(&partial).await {
            total = total.saturating_add(metadata.len().min(file.size));
        }
    }
    total.min(plan.total_bytes)
}

/// Drop `.part` files that belong to no plan entry.
async fn cleanup_parts(parts: &Path, plan: &Plan) {
    let expected: HashSet<String> = plan
        .files
        .iter()
        .map(|file| format!("{}.part", file.index))
        .collect();
    let Ok(mut entries) = tokio::fs::read_dir(parts).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if !expected.contains(&name) {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

/// Back off between two attempts of the same file, still honouring pause and
/// cancel. A short, fixed delay is enough: the classic long-horizon retry
/// policy lives in `install.rs` and wraps this whole run.
async fn sleep_between(run: &Run<'_>, attempt: u32) -> Result<(), ManifestFailure> {
    let delay = Duration::from_millis(250_u64.saturating_mul(u64::from(attempt.max(1))));
    tokio::select! {
        biased;
        _ = run.cancel.cancelled() => Err(ManifestFailure::Cancelled),
        _ = run.pause.cancelled() => Err(ManifestFailure::Paused),
        _ = tokio::time::sleep(delay.min(Duration::from_secs(5))) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A manifest entry plus the body the fake server serves for it.
    struct Payload {
        manifest: Manifest,
        bodies: HashMap<String, Vec<u8>>,
    }

    fn sha256(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    fn payload(files: &[(&str, &[u8])], directories: &[&str]) -> Payload {
        let mut bodies = HashMap::new();
        let mut entries = Vec::new();
        let mut total = 0_u64;
        for (path, body) in files {
            bodies.insert(path.to_string(), body.to_vec());
            total += body.len() as u64;
            entries.push(ManifestFile {
                path: path.to_string(),
                size: body.len() as u64,
                sha256: sha256(body),
                mode: Some(0o644),
                executable: false,
            });
        }
        Payload {
            manifest: Manifest {
                version_id: 7,
                archive_sha256: "a".repeat(64),
                status: "ready".to_string(),
                generated_at: "2026-11-15T00:00:00Z".to_string(),
                total_bytes: total,
                file_count: entries.len() as u64,
                files: entries,
                directories: directories.iter().map(|path| path.to_string()).collect(),
            },
            bodies,
        }
    }

    #[derive(Default)]
    struct FakeFiles {
        bodies: HashMap<String, Vec<u8>>,
        opens: Mutex<Vec<(String, Option<u64>)>>,
        active: AtomicUsize,
        max_active: AtomicUsize,
        /// Serve corrupt bytes on the first request of these paths.
        corrupt_once: Mutex<HashSet<String>>,
        /// Fail these paths with this status instead of serving them.
        fail: Mutex<HashMap<String, u16>>,
        delay: Duration,
    }

    struct FakeBody {
        bytes: Vec<u8>,
        cursor: usize,
        status: u16,
        range_start: Option<u64>,
    }

    impl FileBody for FakeBody {
        fn status(&self) -> u16 {
            self.status
        }
        fn content_length(&self) -> Option<u64> {
            Some(self.bytes.len() as u64)
        }
        fn content_range_start(&self) -> Option<u64> {
            self.range_start
        }
        async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, String> {
            if self.cursor >= self.bytes.len() {
                return Ok(None);
            }
            let end = (self.cursor + 64 * 1024).min(self.bytes.len());
            let chunk = self.bytes[self.cursor..end].to_vec();
            self.cursor = end;
            Ok(Some(chunk))
        }
    }

    struct ActiveGuard<'a>(&'a AtomicUsize);

    impl Drop for ActiveGuard<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl FileTransport for FakeFiles {
        type Body = FakeBody;

        async fn open(
            &self,
            path: &str,
            range_start: Option<u64>,
        ) -> Result<Self::Body, TransportError> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            let _guard = ActiveGuard(&self.active);
            // Scoped on purpose: no lock is held across an await.
            let injected = self.fail.lock().unwrap().get(path).copied();
            if let Some(status) = injected {
                tokio::time::sleep(self.delay).await;
                return Err(TransportError::new(Some(status), "injected"));
            }
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            self.opens
                .lock()
                .unwrap()
                .push((path.to_string(), range_start));
            let Some(mut body) = self.bodies.get(path).cloned() else {
                return Err(TransportError::new(Some(404), "no such file"));
            };
            if self.corrupt_once.lock().unwrap().remove(path) {
                if let Some(first) = body.first_mut() {
                    *first ^= 0xff;
                }
            }
            let start = range_start.unwrap_or(0).min(body.len() as u64) as usize;
            let served = body.split_off(start);
            Ok(FakeBody {
                bytes: served,
                cursor: 0,
                status: if range_start.is_some() { 206 } else { 200 },
                range_start,
            })
        }
    }

    #[derive(Default)]
    struct MemoryCheckpoints {
        archive: Mutex<Option<String>>,
        completed: Mutex<HashMap<String, u64>>,
        records: Mutex<u32>,
    }

    impl FileCheckpoints for MemoryCheckpoints {
        fn attest(
            &self,
            archive_sha256: &str,
            _total_bytes: u64,
            _file_count: u64,
        ) -> Result<Resume, String> {
            let mut stored = self.archive.lock().unwrap();
            match stored.as_deref() {
                Some(previous) if previous == archive_sha256 => Ok(Resume::Adopted),
                Some(_) => {
                    *stored = None;
                    Ok(Resume::Changed)
                }
                None => {
                    *stored = Some(archive_sha256.to_string());
                    Ok(Resume::Fresh)
                }
            }
        }
        fn completed(&self) -> Result<Vec<(String, u64)>, String> {
            Ok(self
                .completed
                .lock()
                .unwrap()
                .iter()
                .map(|(path, size)| (path.clone(), *size))
                .collect())
        }
        fn record(&self, relative: &str, size: u64, _sha256: &str) -> Result<(), String> {
            self.completed
                .lock()
                .unwrap()
                .insert(relative.to_string(), size);
            *self.records.lock().unwrap() += 1;
            Ok(())
        }
        fn clear(&self) -> Result<(), String> {
            self.completed.lock().unwrap().clear();
            Ok(())
        }
    }

    fn tuning(parallel: usize) -> Tuning {
        Tuning {
            parallel,
            progress_interval: Duration::ZERO,
            ..Tuning::default()
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ludex-files-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_install(
        plan: &Plan,
        payload: &Payload,
        files: &FakeFiles,
        checkpoints: &MemoryCheckpoints,
        staging: &Path,
        tuning: &Tuning,
        seen: &Mutex<Vec<Progress>>,
        pause: &CancellationToken,
    ) -> Result<Outcome, ManifestFailure> {
        let cancel = CancellationToken::new();
        let report = |progress: Progress| {
            if let Ok(mut seen) = seen.lock() {
                seen.push(progress);
            }
        };
        install(
            plan,
            &payload.manifest.archive_sha256,
            staging,
            files,
            checkpoints,
            &cancel,
            pause,
            tuning,
            &report,
        )
        .await
    }

    fn no_pause() -> CancellationToken {
        CancellationToken::new()
    }

    fn staged(staging: &Path, relative: &str) -> Vec<u8> {
        std::fs::read(staging.join(relative)).unwrap()
    }

    #[test]
    fn a_plan_rejects_escaping_and_duplicate_paths() {
        let mut fixture = payload(&[("Game/a.bin", b"abc")], &[]);
        fixture.manifest.files[0].path = "../escape.bin".to_string();
        assert!(matches!(plan(&fixture.manifest), Err(PlanError::Unsafe(_))));

        let mut fixture = payload(&[("Game/a.bin", b"abc")], &[]);
        fixture.manifest.files[0].path = "C:\\Windows\\evil.dll".to_string();
        assert!(matches!(plan(&fixture.manifest), Err(PlanError::Unsafe(_))));

        let mut fixture = payload(&[("Game/a.bin", b"abc"), ("Game/a.bin", b"abc")], &[]);
        fixture.manifest.total_bytes = 6;
        fixture.manifest.file_count = 2;
        assert!(matches!(plan(&fixture.manifest), Err(PlanError::Unsafe(_))));

        let mut fixture = payload(&[("Game/a.bin", b"abc")], &[]);
        fixture.manifest.files[0].path = format!("{PARTIAL_DIR}/a.part");
        assert!(matches!(plan(&fixture.manifest), Err(PlanError::Unsafe(_))));
    }

    #[test]
    fn a_plan_rejects_an_inconsistent_manifest() {
        let mut fixture = payload(&[("Game/a.bin", b"abc")], &[]);
        fixture.manifest.total_bytes = 99;
        assert!(matches!(
            plan(&fixture.manifest),
            Err(PlanError::Inconsistent(_))
        ));

        let mut fixture = payload(&[("Game/a.bin", b"abc")], &[]);
        fixture.manifest.file_count = 5;
        assert!(matches!(
            plan(&fixture.manifest),
            Err(PlanError::Inconsistent(_))
        ));

        let mut fixture = payload(&[("Game/a.bin", b"abc")], &[]);
        fixture.manifest.files[0].sha256 = "not-a-digest".into();
        assert!(matches!(
            plan(&fixture.manifest),
            Err(PlanError::Inconsistent(_))
        ));
    }

    #[test]
    fn a_plan_normalizes_separators_and_keeps_empty_directories() {
        let mut fixture = payload(
            &[("Game\\bin\\a.bin", b"abc")],
            &["Game/Saves", "Game\\Empty"],
        );
        let plan = plan(&fixture.manifest).expect("valid plan");
        assert_eq!(plan.files[0].relative, "Game/bin/a.bin");
        assert_eq!(
            plan.files[0].path,
            PathBuf::from("Game").join("bin").join("a.bin")
        );
        assert_eq!(plan.directories.len(), 2);
        assert_eq!(plan.total_bytes, 3);
        // A status that is not ready is a fallback reason, not a plan.
        fixture.manifest.status = "extracting".into();
        assert!(matches!(
            manifest_from_json(&serde_json::to_string(&fixture.manifest).unwrap()),
            Err(PlanError::NotReady(_))
        ));
    }

    #[tokio::test]
    async fn parallel_workers_overlap_more_than_one_file() {
        let payload = payload(
            &[
                ("a.bin", &[1_u8; 4096][..]),
                ("b.bin", &[2_u8; 4096][..]),
                ("c.bin", &[3_u8; 4096][..]),
                ("d.bin", &[4_u8; 4096][..]),
            ],
            &["Empty"],
        );
        let plan = plan(&payload.manifest).unwrap();
        let files = FakeFiles {
            bodies: payload.bodies.clone(),
            delay: Duration::from_millis(20),
            ..FakeFiles::default()
        };
        let checkpoints = MemoryCheckpoints::default();
        let staging = temp_dir("parallel");
        let seen = Mutex::new(Vec::new());
        let tuning = tuning(4);
        let outcome = run_install(
            &plan,
            &payload,
            &files,
            &checkpoints,
            &staging,
            &tuning,
            &seen,
            &no_pause(),
        )
        .await
        .expect("install");
        assert_eq!(outcome.files, 4);
        assert!(
            files.max_active.load(Ordering::SeqCst) >= 2,
            "the pool must overlap files, saw {}",
            files.max_active.load(Ordering::SeqCst)
        );
        assert!(files.max_active.load(Ordering::SeqCst) <= 4);
        assert!(staging.join("Empty").is_dir(), "an empty folder is kept");
        assert!(
            !staging.join(PARTIAL_DIR).exists(),
            "no temp folder is left"
        );
        let last = seen.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.files_done, 4);
        assert_eq!(last.bytes_done, last.bytes_total);
        std::fs::remove_dir_all(&staging).unwrap();
    }

    #[tokio::test]
    async fn a_partial_file_resumes_from_its_offset() {
        let body: Vec<u8> = (0..200_000_u32).map(|value| value as u8).collect();
        let payload = payload(&[("big.bin", &body[..])], &[]);
        let plan = plan(&payload.manifest).unwrap();
        let staging = temp_dir("resume-file");
        let parts = staging.join(PARTIAL_DIR);
        std::fs::create_dir_all(&parts).unwrap();
        std::fs::write(parts.join("0.part"), &body[..90_000]).unwrap();

        let files = FakeFiles {
            bodies: payload.bodies.clone(),
            ..FakeFiles::default()
        };
        // A paused job keeps its staging folder and its checkpoint header: the
        // `.part` below is the in-flight file of the previous attempt.
        let checkpoints = MemoryCheckpoints::default();
        *checkpoints.archive.lock().unwrap() = Some(payload.manifest.archive_sha256.clone());
        let seen = Mutex::new(Vec::new());
        let tuning = tuning(1);
        run_install(
            &plan,
            &payload,
            &files,
            &checkpoints,
            &staging,
            &tuning,
            &seen,
            &no_pause(),
        )
        .await
        .expect("install");
        assert_eq!(
            files.opens.lock().unwrap()[0].1,
            Some(90_000),
            "the second attempt must ask for the missing bytes only"
        );
        assert_eq!(staged(&staging, "big.bin"), body);
        std::fs::remove_dir_all(&staging).unwrap();
    }

    #[tokio::test]
    async fn a_checksum_mismatch_is_downloaded_again_and_never_committed() {
        let payload = payload(&[("bad.bin", b"the real body")], &[]);
        let plan = plan(&payload.manifest).unwrap();
        let staging = temp_dir("checksum");
        let files = FakeFiles {
            bodies: payload.bodies.clone(),
            ..FakeFiles::default()
        };
        files
            .corrupt_once
            .lock()
            .unwrap()
            .insert("bad.bin".to_string());
        let checkpoints = MemoryCheckpoints::default();
        let seen = Mutex::new(Vec::new());
        let tuning = tuning(1);
        let outcome = run_install(
            &plan,
            &payload,
            &files,
            &checkpoints,
            &staging,
            &tuning,
            &seen,
            &no_pause(),
        )
        .await
        .expect("the second attempt succeeds");
        assert_eq!(outcome.files, 1);
        assert_eq!(staged(&staging, "bad.bin"), b"the real body");
        assert_eq!(
            files.opens.lock().unwrap().len(),
            2,
            "one corrupt body, one good one"
        );
        assert_eq!(*checkpoints.records.lock().unwrap(), 1);
        assert!(seen
            .lock()
            .unwrap()
            .iter()
            .any(|progress| progress.verifying));
        std::fs::remove_dir_all(&staging).unwrap();
    }

    #[tokio::test]
    async fn a_checksum_mismatch_that_never_recovers_fails_without_committing() {
        let payload = payload(&[("bad.bin", b"the real body")], &[]);
        let plan = plan(&payload.manifest).unwrap();
        let staging = temp_dir("checksum-fail");
        // Every attempt serves a corrupted first byte.
        let mut corrupt = payload.bodies.clone();
        let bytes = corrupt.get_mut("bad.bin").unwrap();
        bytes[0] ^= 0xff;
        let files = FakeFiles {
            bodies: corrupt,
            ..FakeFiles::default()
        };
        let checkpoints = MemoryCheckpoints::default();
        let seen = Mutex::new(Vec::new());
        let mut tuning = tuning(1);
        tuning.attempts_per_file = 2;
        let error = run_install(
            &plan,
            &payload,
            &files,
            &checkpoints,
            &staging,
            &tuning,
            &seen,
            &no_pause(),
        )
        .await
        .expect_err("verification must fail");
        assert!(matches!(error, ManifestFailure::Failed(_)));
        assert!(!staging.join("bad.bin").exists(), "never committed");
        assert_eq!(*checkpoints.records.lock().unwrap(), 0);
        std::fs::remove_dir_all(&staging).unwrap();
    }

    /// The rename is the commit point: a body that never reaches the declared
    /// size leaves only the `.part`, never a game file.
    #[tokio::test]
    async fn an_incomplete_body_is_never_renamed_into_place() {
        let payload = payload(&[("short.bin", b"0123456789")], &[]);
        let plan = plan(&payload.manifest).unwrap();
        let staging = temp_dir("short");
        let files = FakeFiles {
            // Only half of the declared body arrives.
            bodies: HashMap::from([("short.bin".to_string(), b"01234".to_vec())]),
            ..FakeFiles::default()
        };
        let checkpoints = MemoryCheckpoints::default();
        let seen = Mutex::new(Vec::new());
        let mut tuning = tuning(1);
        tuning.attempts_per_file = 2;
        let error = run_install(
            &plan,
            &payload,
            &files,
            &checkpoints,
            &staging,
            &tuning,
            &seen,
            &no_pause(),
        )
        .await
        .expect_err("the body is incomplete");
        assert!(matches!(error, ManifestFailure::Failed(_)), "{error:?}");
        assert!(!staging.join("short.bin").exists(), "never committed");
        assert_eq!(
            std::fs::read(staging.join(PARTIAL_DIR).join("0.part")).unwrap(),
            b"01234",
            "the resume point survives"
        );
        assert_eq!(*checkpoints.records.lock().unwrap(), 0);
        std::fs::remove_dir_all(&staging).unwrap();
    }

    #[tokio::test]
    async fn a_restart_skips_completed_files_and_keeps_their_bytes() {
        let payload = payload(
            &[
                ("one.bin", &[7_u8; 300_000][..]),
                ("two.bin", &[9_u8; 300_000][..]),
            ],
            &[],
        );
        let plan = plan(&payload.manifest).unwrap();
        let checkpoints = MemoryCheckpoints::default();
        let first_staging = temp_dir("restart");

        // First run: `two.bin` is served by a broken route, so the job fails
        // after `one.bin` is committed.
        let failing = FakeFiles {
            bodies: payload.bodies.clone(),
            fail: Mutex::new(HashMap::from([("two.bin".to_string(), 500_u16)])),
            ..FakeFiles::default()
        };
        let mut first_tuning = tuning(1);
        first_tuning.attempts_per_file = 1;
        let seen = Mutex::new(Vec::new());
        run_install(
            &plan,
            &payload,
            &failing,
            &checkpoints,
            &first_staging,
            &first_tuning,
            &seen,
            &no_pause(),
        )
        .await
        .expect_err("the server fails the second file");
        assert!(first_staging.join("one.bin").is_file());

        // Second run, same job and staging: only `two.bin` is fetched.
        let files = FakeFiles {
            bodies: payload.bodies.clone(),
            ..FakeFiles::default()
        };
        let tuning = tuning(2);
        let outcome = run_install(
            &plan,
            &payload,
            &files,
            &checkpoints,
            &first_staging,
            &tuning,
            &seen,
            &no_pause(),
        )
        .await
        .expect("resume");
        assert_eq!(outcome.resumed_files, 1);
        let opens = files.opens.lock().unwrap();
        assert_eq!(opens.len(), 1);
        assert_eq!(opens[0].0, "two.bin");
        assert_eq!(staged(&first_staging, "one.bin"), vec![7_u8; 300_000]);
        std::fs::remove_dir_all(&first_staging).unwrap();
    }

    #[tokio::test]
    async fn a_changed_archive_discards_the_previous_files() {
        let payload = payload(&[("one.bin", b"new body")], &[]);
        let plan = plan(&payload.manifest).unwrap();
        let staging = temp_dir("changed");
        std::fs::write(staging.join("one.bin"), b"old body").unwrap();
        let checkpoints = MemoryCheckpoints::default();
        *checkpoints.archive.lock().unwrap() = Some("b".repeat(64));
        checkpoints
            .completed
            .lock()
            .unwrap()
            .insert("one.bin".to_string(), 8);
        let files = FakeFiles {
            bodies: payload.bodies.clone(),
            ..FakeFiles::default()
        };
        let seen = Mutex::new(Vec::new());
        let tuning = tuning(1);
        run_install(
            &plan,
            &payload,
            &files,
            &checkpoints,
            &staging,
            &tuning,
            &seen,
            &no_pause(),
        )
        .await
        .expect("install");
        assert_eq!(staged(&staging, "one.bin"), b"new body");
        assert_eq!(files.opens.lock().unwrap().len(), 1);
        std::fs::remove_dir_all(&staging).unwrap();
    }

    #[tokio::test]
    async fn a_cache_miss_is_a_fallback_not_a_failure() {
        let payload = payload(&[("one.bin", b"body")], &[]);
        let plan = plan(&payload.manifest).unwrap();
        let staging = temp_dir("fallback");
        let files = FakeFiles {
            bodies: payload.bodies.clone(),
            fail: Mutex::new(HashMap::from([("one.bin".to_string(), 409_u16)])),
            ..FakeFiles::default()
        };
        let checkpoints = MemoryCheckpoints::default();
        let seen = Mutex::new(Vec::new());
        let tuning = tuning(1);
        let error = run_install(
            &plan,
            &payload,
            &files,
            &checkpoints,
            &staging,
            &tuning,
            &seen,
            &no_pause(),
        )
        .await
        .expect_err("409 falls back");
        assert!(matches!(error, ManifestFailure::Fallback(_)), "{error:?}");
        assert!(!staging.join("one.bin").exists());
        std::fs::remove_dir_all(&staging).unwrap();
    }

    #[tokio::test]
    async fn no_archive_and_no_temp_files_are_left_behind() {
        let payload = payload(&[("Game/a.bin", b"aaaa"), ("Game/b.bin", b"bbbb")], &[]);
        let plan = plan(&payload.manifest).unwrap();
        let staging = temp_dir("no-archive");
        let files = FakeFiles {
            bodies: payload.bodies.clone(),
            ..FakeFiles::default()
        };
        let checkpoints = MemoryCheckpoints::default();
        let seen = Mutex::new(Vec::new());
        let tuning = tuning(2);
        run_install(
            &plan,
            &payload,
            &files,
            &checkpoints,
            &staging,
            &tuning,
            &seen,
            &no_pause(),
        )
        .await
        .expect("install");
        let mut entries: Vec<String> = std::fs::read_dir(&staging)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        entries.sort();
        assert_eq!(entries, vec!["Game".to_string()]);
        for archive in ["game.zip", "game.7z", "archive.part"] {
            assert!(!staging.join(archive).exists(), "no archive on disk");
        }
        std::fs::remove_dir_all(&staging).unwrap();
    }

    struct FakeManifest {
        bodies: Mutex<Vec<Result<String, TransportError>>>,
        extract_calls: AtomicU32,
    }

    impl ManifestTransport for FakeManifest {
        async fn fetch(&self) -> Result<String, TransportError> {
            let mut bodies = self.bodies.lock().unwrap();
            if bodies.len() > 1 {
                bodies.remove(0)
            } else {
                bodies
                    .pop()
                    .unwrap_or_else(|| Err(TransportError::new(Some(409), "missing")))
            }
        }
        async fn request_extract(&self) -> Result<String, TransportError> {
            self.extract_calls.fetch_add(1, Ordering::SeqCst);
            Ok("{\"status\":\"queued\"}".to_string())
        }
    }

    #[tokio::test]
    async fn a_409_falls_back_and_an_explicit_choice_warms_the_cache() {
        let payload = payload(&[("a.bin", b"body")], &[]);
        let ready = serde_json::to_string(&payload.manifest).unwrap();
        let cache_miss = Err(TransportError::new(
            Some(409),
            "extracted_cache_unavailable",
        ));

        // A probe without `warm_cache`: a 409 is a fallback reason and never
        // asks for an extraction.
        let transport = FakeManifest {
            bodies: Mutex::new(vec![cache_miss.clone()]),
            extract_calls: AtomicU32::new(0),
        };
        let error = prepare(&transport, &Tuning::default())
            .await
            .expect_err("not ready");
        assert!(matches!(error, PlanError::NotReady(_)));
        assert_eq!(transport.extract_calls.load(Ordering::SeqCst), 0);

        // `warm_cache` (automatic on the local server): ask for the extraction
        // once, then poll until ready.
        let transport = FakeManifest {
            bodies: Mutex::new(vec![cache_miss, Ok(ready.clone())]),
            extract_calls: AtomicU32::new(0),
        };
        let tuning = Tuning {
            warm_cache: true,
            cache_poll: Duration::from_millis(1),
            cache_wait: Duration::from_secs(1),
            ..Tuning::default()
        };
        let manifest = prepare(&transport, &tuning).await.expect("warmed");
        assert_eq!(manifest.version_id, 7);
        assert_eq!(transport.extract_calls.load(Ordering::SeqCst), 1);
    }

    /// A server whose extracted cache is disabled: the manifest is a 409 and
    /// asking for an extraction is refused (the real transport maps the
    /// `unavailable` acknowledgement to an error, #235).
    struct DisabledManifest {
        fetch_calls: AtomicU32,
    }

    impl ManifestTransport for DisabledManifest {
        async fn fetch(&self) -> Result<String, TransportError> {
            self.fetch_calls.fetch_add(1, Ordering::SeqCst);
            Err(TransportError::new(
                Some(409),
                "extracted_cache_unavailable",
            ))
        }
        async fn request_extract(&self) -> Result<String, TransportError> {
            Err(TransportError::new(
                Some(409),
                "extracted cache is disabled (LUDEX_EXTRACTED_CACHE=0)",
            ))
        }
    }

    #[tokio::test]
    async fn a_disabled_cache_falls_back_without_polling() {
        let transport = DisabledManifest {
            fetch_calls: AtomicU32::new(0),
        };
        let tuning = Tuning {
            warm_cache: true,
            cache_poll: Duration::from_millis(1),
            cache_wait: Duration::from_secs(30),
            ..Tuning::default()
        };
        let started = Instant::now();
        let error = prepare(&transport, &tuning)
            .await
            .expect_err("disabled cache");
        assert!(matches!(error, PlanError::NotReady(_)), "{error:?}");
        // One probe, one refused warm request, no polling loop: the automatic
        // install must not stall for the whole cache timeout (#235).
        assert_eq!(transport.fetch_calls.load(Ordering::SeqCst), 1);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn a_pause_keeps_the_partial_file_for_the_next_attempt() {
        let body: Vec<u8> = vec![5; 400_000];
        let payload = payload(&[("big.bin", &body[..])], &[]);
        let plan = plan(&payload.manifest).unwrap();
        let staging = temp_dir("pause");
        let files = FakeFiles {
            bodies: payload.bodies.clone(),
            ..FakeFiles::default()
        };
        let checkpoints = MemoryCheckpoints::default();
        let cancel = CancellationToken::new();
        let pause = CancellationToken::new();
        let pause_for_progress = pause.clone();
        let tuning = tuning(1);
        let seen = Mutex::new(Vec::new());
        let error = install(
            &plan,
            &payload.manifest.archive_sha256,
            &staging,
            &files,
            &checkpoints,
            &cancel,
            &pause,
            &tuning,
            &move |progress| {
                if progress.bytes_done > 100_000 {
                    pause_for_progress.cancel();
                }
                seen.lock().unwrap().push(progress);
            },
        )
        .await
        .expect_err("paused");
        assert_eq!(error, ManifestFailure::Paused);
        let partial = staging.join(PARTIAL_DIR).join("0.part");
        assert!(partial.is_file(), "the partial survives a pause");
        assert!(!staging.join("big.bin").exists());
        assert_eq!(*checkpoints.records.lock().unwrap(), 0);
        std::fs::remove_dir_all(&staging).unwrap();
    }
}
