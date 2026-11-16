//! Server-side extracted cache and file manifest (#231).
//!
//! The prototype in GitHub issue #231 asks one question: is it faster to
//! extract a large archive **once on the server** and let the client fetch the
//! files in parallel, than to download the archive and extract it on the
//! client? This module owns the server half.
//!
//! ## Layout
//!
//! One directory per archive, keyed by the archive's full content SHA-256
//! (`game_versions.content_sha256`, #216) plus the extraction options:
//!
//! ```text
//! {data_dir}/extracted/<archive_sha256>/
//!     manifest.json   # file list, sizes, per-file SHA-256, modes, empty dirs
//!     meta.json       # cache metadata (version, size, createdAt, lastUsedAt)
//!     files/          # the extracted tree, paths relative to the install root
//! ```
//!
//! Extraction runs into a hidden `.tmp-<sha>-<uuid>` sibling and is published
//! with a single `rename`, so a reader never sees a half-extracted tree and a
//! crash leaves only a temp directory that the startup scan deletes. The key is
//! the archive digest the server already trusts, never a client-supplied path.
//!
//! ## Policy
//!
//! - **Off in server mode** (`LUDEX_EXTRACTED_CACHE` defaults to `0`, #231),
//!   **on in the desktop's embedded local server** with a bounded budget
//!   (#241; the desktop setting `serverFileCache` can turn it off). Nothing is
//!   read or written while disabled; the extract route answers `unavailable`
//!   and the manifest route answers `409 extracted_cache_unavailable`, which is
//!   the client's signal to fall back to the classic download.
//! - **Caps**: at most [`MAX_FILES`] files, [`MAX_FILE_BYTES`] per file and
//!   [`MAX_EXTRACTED_BYTES`] in total. `verify_archive` rejects an archive whose
//!   declared totals exceed the caps before a single byte is extracted; the
//!   manifest walk re-checks what actually landed on disk.
//! - **Symlinks**: refused. `ludex_core::archive::extract_archive` already
//!   rejects link entries, and the manifest walk fails the entry if a symlink
//!   appears anyway. Nothing is followed outside the cache root.
//! - **Eviction**: least-recently-used by `lastUsedAt`, a TTL for entries
//!   unused for [`crate::config::DEFAULT_EXTRACTED_CACHE_TTL_HOURS`] days, and
//!   the byte budget after every insertion. An entry with a live
//!   [`EntryLease`] (a file being served) is never evicted.
//! - **Failure** deletes the temp directory: a failed extraction can never be
//!   mistaken for a ready cache entry.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use ludex_core::archive::{
    extract_archive, verify_archive, ArchiveError, ArchiveFormat, ExtractOptions, ExtractProgress,
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::config::Config;
use crate::events::{Actor, Event};
use crate::models::VersionRow;
use crate::state::AppState;

/// Manifest format version written by this build.
pub const MANIFEST_FORMAT: u32 = 1;
/// Extraction options baked into the cache key. Bump when extraction settings
/// (passwords, mode handling, path sanitising) change in a way that would make
/// an older tree wrong.
pub const OPTIONS_KEY: &str = "v1";
/// File names inside one cache entry.
pub const MANIFEST_NAME: &str = "manifest.json";
/// Cache metadata sidecar.
pub const META_NAME: &str = "meta.json";
/// Directory holding the extracted tree.
pub const FILES_DIR: &str = "files";
/// Prefix of in-progress extraction directories.
pub const TEMP_PREFIX: &str = ".tmp-";

/// Maximum number of files one entry may hold.
pub const MAX_FILES: usize = 200_000;
/// Maximum size of one extracted file (128 GiB).
pub const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024 * 1024;
/// Maximum total extracted size of one entry (512 GiB).
pub const MAX_EXTRACTED_BYTES: u64 = 512 * 1024 * 1024 * 1024;
/// A failed extraction stays reportable for this long before a new attempt is
/// started, so an admin polling the route does not retry a 40 GB failure in a
/// loop.
pub const FAILED_RETENTION: Duration = Duration::from_secs(60);
/// Minimum interval between two `meta.json` refreshes for a served entry.
const META_PERSIST_INTERVAL: Duration = Duration::from_secs(60);

/// One file of an extracted entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedFile {
    /// Path relative to the install root, `/` separated.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// Lowercase hex SHA-256 of the file content.
    pub sha256: String,
    /// Unix permission bits (`0o644` when the archive carried none).
    pub mode: u32,
    /// Convenience flag: `mode & 0o111 != 0`.
    pub executable: bool,
}

/// On-disk manifest of one extracted archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedManifest {
    /// Format version of this document.
    pub format_version: u32,
    /// Extraction options the entry was produced with.
    pub options_key: String,
    /// Archive content SHA-256 this manifest belongs to.
    pub archive_sha256: String,
    /// Extraction timestamp.
    pub generated_at: DateTime<Utc>,
    /// Sum of the file sizes.
    pub total_bytes: u64,
    /// Number of files.
    pub file_count: usize,
    /// Files, sorted by path.
    pub files: Vec<ExtractedFile>,
    /// Directories that contain no file, sorted by path.
    pub directories: Vec<String>,
}

/// Cache metadata sidecar. Written before the entry is published.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheEntryMeta {
    format_version: u32,
    options_key: String,
    archive_sha256: String,
    version_id: i64,
    bytes: u64,
    files: usize,
    created_at: DateTime<Utc>,
    last_used_at: DateTime<Utc>,
}

/// One ready cache entry, as the admin API and the eviction logic see it.
#[derive(Debug, Clone)]
pub struct ExtractedEntry {
    /// Archive content SHA-256.
    pub sha256: String,
    /// Version that first requested the extraction.
    pub version_id: i64,
    /// Extracted size in bytes.
    pub bytes: u64,
    /// Number of files.
    pub files: usize,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last time a manifest or file request used the entry.
    pub last_used_at: DateTime<Utc>,
    /// Parsed manifest, kept in memory so a file request never re-reads it.
    ///
    /// Bounded by the number of ready entries (a handful in practice); the
    /// files vector is sorted by path, which is also the path index.
    pub manifest: Arc<ExtractedManifest>,
    /// When `meta.json` was refreshed last (process-local clock).
    persisted_at: Instant,
}

/// State of an extraction job, as returned by the extract route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ExtractState {
    /// Accepted, not started yet.
    Queued,
    /// Extraction in progress.
    Extracting,
    /// Entry is ready to serve.
    Ready,
    /// Extraction failed; see `error`.
    Failed,
    /// The cache is disabled on this server.
    Unavailable,
}

impl ExtractState {
    /// Stable lowercase identifier used in the API payload.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Extracting => "extracting",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Unavailable => "unavailable",
        }
    }
}

/// `POST /api/admin/versions/{versionId}/extract` response.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExtractStatus {
    /// Version the extraction belongs to.
    pub version_id: i64,
    /// Archive content SHA-256, the cache key.
    pub archive_sha256: String,
    /// Job state.
    pub status: ExtractState,
    /// Files delivered so far (or the final count).
    pub files: usize,
    /// Bytes delivered so far (or the final count).
    pub bytes: u64,
    /// Failure detail for the administrator (archive error, cap, budget).
    pub error: Option<String>,
}

/// One extraction job, shared between the spawned task and status readers.
#[derive(Debug)]
pub struct Job {
    version_id: i64,
    state: ExtractState,
    files: usize,
    bytes: u64,
    error: Option<String>,
    failed_at: Option<Instant>,
}

impl Job {
    fn status(&self, sha: &str) -> ExtractStatus {
        ExtractStatus {
            version_id: self.version_id,
            archive_sha256: sha.to_string(),
            status: self.state,
            files: self.files,
            bytes: self.bytes,
            error: self.error.clone(),
        }
    }
}

/// Result of [`ExtractedCache::submit`].
pub enum Submission {
    /// The entry already exists and can be served.
    Ready(ExtractStatus),
    /// A job for this archive is already running (or just failed).
    Running(ExtractStatus),
    /// The caller owns the job and must spawn the extraction.
    Started(Arc<Mutex<Job>>),
}

/// Everything that can go wrong while building or serving an entry.
#[derive(Debug)]
pub enum CacheError {
    /// Filesystem error.
    Io(io::Error),
    /// Archive layer error.
    Archive(ArchiveError),
    /// Manifest serialisation error.
    Json(serde_json::Error),
    /// An entry path is not valid UTF-8.
    InvalidPath(String),
    /// Too many files for one entry.
    TooManyFiles(usize),
    /// One file is too large.
    FileTooLarge(String, u64),
    /// The extracted tree is too large.
    TooLarge(u64),
    /// The entry does not fit the cache budget.
    Budget(u64),
    /// The archive is encrypted.
    Encrypted,
    /// The archive is not a supported format.
    Unsupported,
    /// The archive could not be located or staged.
    Source(String),
    /// A blocking task panicked.
    Join(String),
    /// The entry is not ready.
    NotReady,
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::Archive(error) => write!(formatter, "archive error: {error}"),
            Self::Json(error) => write!(formatter, "manifest error: {error}"),
            Self::InvalidPath(path) => write!(formatter, "entry path is not valid UTF-8: {path}"),
            Self::TooManyFiles(count) => {
                write!(
                    formatter,
                    "archive has {count} files, above the {MAX_FILES} file cap"
                )
            }
            Self::FileTooLarge(path, size) => write!(
                formatter,
                "file {path} is {size} bytes, above the {MAX_FILE_BYTES} byte per-file cap"
            ),
            Self::TooLarge(size) => write!(
                formatter,
                "extracted tree is {size} bytes, above the {MAX_EXTRACTED_BYTES} byte cap"
            ),
            Self::Budget(size) => write!(
                formatter,
                "extracted tree is {size} bytes, above the extracted-cache budget"
            ),
            Self::Encrypted => formatter.write_str("archive is encrypted"),
            Self::Unsupported => formatter.write_str("archive format is not supported"),
            Self::Source(message) => write!(formatter, "archive is unavailable: {message}"),
            Self::Join(message) => write!(formatter, "extraction task failed: {message}"),
            Self::NotReady => formatter.write_str("extracted cache entry is not ready"),
        }
    }
}

impl std::error::Error for CacheError {}

impl From<io::Error> for CacheError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ArchiveError> for CacheError {
    fn from(error: ArchiveError) -> Self {
        Self::Archive(error)
    }
}

impl From<serde_json::Error> for CacheError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// A ready entry held by a handler for the lifetime of a response.
///
/// The lease increments a refcount the eviction logic respects: an entry that
/// is being served is never deleted under the reader.
#[derive(Debug)]
pub struct EntryLease {
    cache: ExtractedCache,
    sha256: String,
}

impl EntryLease {
    /// Archive digest of the leased entry.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Manifest of the leased entry. The lease keeps the entry alive, so the
    /// manifest cannot be evicted while the response is being built.
    #[must_use]
    pub fn manifest(&self) -> Arc<ExtractedManifest> {
        self.cache
            .state()
            .entries
            .get(&self.sha256)
            .map(|entry| Arc::clone(&entry.manifest))
            .unwrap_or_else(|| Arc::new(empty_manifest(&self.sha256)))
    }

    /// Records a use, refreshing the LRU position.
    pub fn touch(&self) {
        if let Some(entry) = self.cache.state().entries.get_mut(&self.sha256) {
            entry.last_used_at = Utc::now();
        }
    }
}

impl Drop for EntryLease {
    fn drop(&mut self) {
        let persist = {
            // One lock only: the standard mutex is not reentrant, and a second
            // `state()` call here would deadlock on drop.
            let mut guard = self.cache.state();
            let mut due = false;
            if let Some(entry) = guard.entries.get_mut(&self.sha256) {
                entry.last_used_at = Utc::now();
                if entry.persisted_at.elapsed() >= META_PERSIST_INTERVAL {
                    entry.persisted_at = Instant::now();
                    due = true;
                }
            }
            if let Some(count) = guard.active.get_mut(&self.sha256) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    guard.active.remove(&self.sha256);
                }
            }
            due
        };
        if persist {
            self.cache.persist_meta(&self.sha256);
        }
    }
}

/// Result carried out of one extraction.
#[derive(Debug, Clone)]
pub struct ExtractOutcome {
    /// Files written into the cache.
    pub files: usize,
    /// Bytes written into the cache.
    pub bytes: u64,
    /// Extraction timestamp recorded in the manifest.
    pub created_at: DateTime<Utc>,
    /// Manifest published with the entry.
    pub manifest: Arc<ExtractedManifest>,
}

#[derive(Debug, Default)]
struct CacheState {
    entries: HashMap<String, ExtractedEntry>,
    active: HashMap<String, usize>,
    jobs: HashMap<String, Arc<Mutex<Job>>>,
}

#[derive(Debug)]
struct Inner {
    enabled: bool,
    root: PathBuf,
    budget_bytes: u64,
    ttl: Duration,
    state: Mutex<CacheState>,
}

/// Server-side extracted cache handle. Cheap to clone.
#[derive(Debug, Clone)]
pub struct ExtractedCache {
    inner: Arc<Inner>,
}

impl ExtractedCache {
    /// Builds the cache from configuration without touching the disk.
    #[must_use]
    pub fn new(config: &Config) -> Self {
        Self {
            inner: Arc::new(Inner {
                enabled: config.extracted_cache_enabled,
                root: config.extracted_cache_dir.clone(),
                budget_bytes: config.extracted_cache_budget_bytes,
                ttl: config.extracted_cache_ttl,
                state: Mutex::new(CacheState::default()),
            }),
        }
    }

    /// Builds the cache and rebuilds its in-memory index from disk.
    ///
    /// Leftover `.tmp-*` directories from a crash are removed; a directory
    /// without a readable `manifest.json`/`meta.json` is dropped.
    pub async fn open(config: &Config) -> Self {
        let cache = Self::new(config);
        if cache.enabled() {
            let scanner = cache.clone();
            let _ = tokio::task::spawn_blocking(move || scanner.scan_root()).await;
        }
        cache
    }

    fn state(&self) -> MutexGuard<'_, CacheState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// Whether the extracted cache is enabled.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.inner.enabled
    }

    /// Cache root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// Configured budget in bytes.
    #[must_use]
    pub fn budget_bytes(&self) -> u64 {
        self.inner.budget_bytes
    }

    /// Configured TTL for unused entries.
    #[must_use]
    pub fn ttl(&self) -> Duration {
        self.inner.ttl
    }

    /// Interval of the TTL sweeper.
    #[must_use]
    pub fn sweep_interval(&self) -> Duration {
        if self.inner.ttl.is_zero() {
            return Duration::from_secs(3_600);
        }
        (self.inner.ttl / 4)
            .max(Duration::from_secs(1))
            .min(Duration::from_secs(3_600))
    }

    /// Directory of one entry.
    #[must_use]
    pub fn entry_dir(&self, sha256: &str) -> PathBuf {
        self.inner.root.join(sha256)
    }

    /// Files directory of one entry.
    #[must_use]
    pub fn files_dir(&self, sha256: &str) -> PathBuf {
        self.entry_dir(sha256).join(FILES_DIR)
    }

    /// Bytes currently held by ready entries.
    #[must_use]
    pub fn used_bytes(&self) -> u64 {
        self.state().entries.values().map(|entry| entry.bytes).sum()
    }

    /// Snapshot of every ready entry, sorted by creation time.
    #[must_use]
    pub fn entries(&self) -> Vec<ExtractedEntry> {
        let mut entries: Vec<ExtractedEntry> = self.state().entries.values().cloned().collect();
        entries.sort_by_key(|entry| (entry.created_at, entry.sha256.clone()));
        entries
    }

    /// One entry, if ready.
    #[must_use]
    pub fn entry(&self, sha256: &str) -> Option<ExtractedEntry> {
        self.state().entries.get(sha256).cloned()
    }

    /// Whether one entry is ready.
    #[must_use]
    pub fn has_entry(&self, sha256: &str) -> bool {
        self.state().entries.contains_key(sha256)
    }

    /// Whether an extraction job for this archive is queued or running.
    #[must_use]
    pub fn has_job(&self, sha256: &str) -> bool {
        self.state().jobs.contains_key(sha256)
    }

    /// Leases one entry for serving. Returns `None` when it is not cached.
    #[must_use]
    pub fn acquire(&self, sha256: &str) -> Option<EntryLease> {
        let mut guard = self.state();
        if !guard.entries.contains_key(sha256) {
            return None;
        }
        *guard.active.entry(sha256.to_string()).or_insert(0) += 1;
        Some(EntryLease {
            cache: self.clone(),
            sha256: sha256.to_string(),
        })
    }

    /// Reads the manifest of one entry.
    pub fn load_manifest(&self, sha256: &str) -> Result<Arc<ExtractedManifest>, CacheError> {
        self.state()
            .entries
            .get(sha256)
            .map(|entry| Arc::clone(&entry.manifest))
            .ok_or(CacheError::NotReady)
    }

    /// Starts or joins the extraction of one archive.
    ///
    /// The cache state and the job state each have their own mutex; they are
    /// never held at the same time in opposite orders.
    #[must_use]
    pub fn submit(&self, sha256: &str, version_id: i64) -> Submission {
        let existing = {
            let guard = self.state();
            if let Some(entry) = guard.entries.get(sha256) {
                return Submission::Ready(ExtractStatus {
                    version_id,
                    archive_sha256: sha256.to_string(),
                    status: ExtractState::Ready,
                    files: entry.files,
                    bytes: entry.bytes,
                    error: None,
                });
            }
            guard.jobs.get(sha256).cloned()
        };
        if let Some(job) = existing {
            let retained = {
                let job = job.lock().unwrap_or_else(|error| error.into_inner());
                job.state != ExtractState::Failed
                    || job
                        .failed_at
                        .is_none_or(|failed_at| failed_at.elapsed() <= FAILED_RETENTION)
            };
            if retained {
                let job = job.lock().unwrap_or_else(|error| error.into_inner());
                return Submission::Running(job.status(sha256));
            }
            self.state().jobs.remove(sha256);
        }
        let job = Arc::new(Mutex::new(Job {
            version_id,
            state: ExtractState::Queued,
            files: 0,
            bytes: 0,
            error: None,
            failed_at: None,
        }));
        self.state()
            .jobs
            .insert(sha256.to_string(), Arc::clone(&job));
        Submission::Started(job)
    }

    /// Marks a job as running. Called by the spawned extraction task.
    pub fn mark_extracting(&self, job: &Arc<Mutex<Job>>) {
        let mut job = job.lock().unwrap_or_else(|error| error.into_inner());
        job.state = ExtractState::Extracting;
    }

    /// Records extraction progress, throttled by the core's own 1 MiB step.
    pub fn report_progress(job: &Arc<Mutex<Job>>, progress: &ExtractProgress) {
        if let Ok(mut job) = job.lock() {
            job.files = progress.entries_done;
            job.bytes = progress.bytes_done;
        }
    }

    /// Publishes a finished extraction, or records the failure.
    ///
    /// A ready entry that alone exceeds the budget is removed again and the
    /// job reports [`CacheError::Budget`]; nothing is served that the cache
    /// cannot also evict.
    pub fn finish_extract(
        &self,
        sha256: &str,
        job: &Arc<Mutex<Job>>,
        outcome: Result<ExtractOutcome, CacheError>,
    ) -> Result<ExtractOutcome, CacheError> {
        match outcome {
            Ok(outcome) => {
                let version_id = job
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .version_id;
                let entry = ExtractedEntry {
                    sha256: sha256.to_string(),
                    version_id,
                    bytes: outcome.bytes,
                    files: outcome.files,
                    created_at: outcome.created_at,
                    last_used_at: outcome.created_at,
                    manifest: Arc::clone(&outcome.manifest),
                    persisted_at: Instant::now(),
                };
                {
                    let mut guard = self.state();
                    guard.jobs.remove(sha256);
                    guard.entries.insert(sha256.to_string(), entry);
                }
                self.enforce_budget(Some(sha256));
                if self.used_bytes() > self.budget_bytes() {
                    self.remove(sha256);
                    let error = CacheError::Budget(outcome.bytes);
                    self.mark_failed(sha256, job, &error);
                    return Err(error);
                }
                let mut job = job.lock().unwrap_or_else(|error| error.into_inner());
                job.state = ExtractState::Ready;
                job.files = outcome.files;
                job.bytes = outcome.bytes;
                job.error = None;
                Ok(outcome)
            }
            Err(error) => {
                self.mark_failed(sha256, job, &error);
                Err(error)
            }
        }
    }

    fn mark_failed(&self, sha256: &str, job: &Arc<Mutex<Job>>, error: &CacheError) {
        {
            let mut job = job.lock().unwrap_or_else(|poison| poison.into_inner());
            job.state = ExtractState::Failed;
            job.error = Some(error.to_string());
            job.failed_at = Some(Instant::now());
        }
        // Keep the failure readable for [`FAILED_RETENTION`] so an admin poll
        // does not restart a large extraction on every request.
        let mut guard = self.state();
        if !guard.entries.contains_key(sha256) {
            guard.jobs.insert(sha256.to_string(), Arc::clone(job));
        }
    }

    /// Deletes one entry. Returns `false` when it is absent or being served.
    pub fn remove(&self, sha256: &str) -> bool {
        let running_job = self.state().jobs.get(sha256).cloned();
        if let Some(job) = running_job {
            let running = {
                let job = job.lock().unwrap_or_else(|error| error.into_inner());
                job.state != ExtractState::Failed
            };
            if running {
                return false;
            }
        }
        let entry = {
            let mut guard = self.state();
            if guard.active.get(sha256).is_some_and(|count| *count > 0) {
                return false;
            }
            guard.entries.remove(sha256)
        };
        if entry.is_none() {
            return false;
        }
        match std::fs::remove_dir_all(self.entry_dir(sha256)) {
            Ok(()) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => true,
            Err(error) => {
                tracing::warn!(%error, sha256, "could not delete an extracted cache entry");
                false
            }
        }
    }

    /// Forgets an entry when its archive is deleted from the library.
    ///
    /// The caller checks that no other version shares the archive digest; this
    /// only removes the bytes and the index row.
    pub fn forget(&self, sha256: &str) -> bool {
        {
            let mut guard = self.state();
            guard.jobs.remove(sha256);
        }
        self.remove(sha256)
    }

    /// Evicts least-recently-used entries above the budget.
    ///
    /// `protect` names the entry just inserted, which must not be evicted by
    /// its own insertion. Entries being served, or with a running job, are
    /// skipped. Returns the number of evicted entries.
    pub fn enforce_budget(&self, protect: Option<&str>) -> usize {
        let mut evicted = Vec::new();
        {
            let mut guard = self.state();
            let mut used: u64 = guard.entries.values().map(|entry| entry.bytes).sum();
            if used <= self.inner.budget_bytes {
                return 0;
            }
            let mut candidates: Vec<(String, DateTime<Utc>)> = guard
                .entries
                .iter()
                .filter(|(sha256, _)| Some(sha256.as_str()) != protect)
                .filter(|(sha256, _)| !guard.active.contains_key(*sha256))
                .filter(|(sha256, _)| !guard.jobs.contains_key(*sha256))
                .map(|(sha256, entry)| (sha256.clone(), entry.last_used_at))
                .collect();
            candidates.sort_by_key(|candidate| candidate.1);
            for (sha256, _) in candidates {
                if used <= self.inner.budget_bytes {
                    break;
                }
                if let Some(entry) = guard.entries.remove(&sha256) {
                    used = used.saturating_sub(entry.bytes);
                    evicted.push(sha256);
                }
            }
        }
        for sha256 in &evicted {
            if let Err(error) = std::fs::remove_dir_all(self.entry_dir(sha256)) {
                if error.kind() != io::ErrorKind::NotFound {
                    tracing::warn!(%error, sha256, "could not delete an evicted cache entry");
                }
            }
        }
        if !evicted.is_empty() {
            tracing::info!(
                entries = evicted.len(),
                used = self.used_bytes(),
                budget = self.inner.budget_bytes,
                "extracted cache evicted least-recently-used entries"
            );
        }
        evicted.len()
    }

    /// Evicts entries unused for longer than the TTL, then enforces the budget.
    ///
    /// Returns the number of evicted entries.
    pub fn sweep(&self) -> usize {
        let ttl = self.inner.ttl;
        let mut evicted = 0;
        if !ttl.is_zero() {
            let now = Utc::now();
            let expired: Vec<String> = {
                let guard = self.state();
                guard
                    .entries
                    .iter()
                    .filter(|(sha256, _)| !guard.active.contains_key(*sha256))
                    .filter(|(sha256, _)| !guard.jobs.contains_key(*sha256))
                    .filter(|(_, entry)| {
                        now.signed_duration_since(entry.last_used_at)
                            .to_std()
                            .is_ok_and(|age| age > ttl)
                    })
                    .map(|(sha256, _)| sha256.clone())
                    .collect()
            };
            for sha256 in expired {
                if self.remove(&sha256) {
                    evicted += 1;
                    tracing::info!(sha256, "extracted cache entry expired");
                }
            }
        }
        evicted + self.enforce_budget(None)
    }

    /// Rebuilds the index from disk and drops unusable directories.
    fn scan_root(&self) {
        if let Err(error) = std::fs::create_dir_all(&self.inner.root) {
            tracing::warn!(
                %error,
                root = %self.inner.root.display(),
                "could not create the extracted cache root"
            );
            return;
        }
        let Ok(entries) = std::fs::read_dir(&self.inner.root) else {
            return;
        };
        let mut recovered = 0;
        let mut dropped = 0;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(TEMP_PREFIX) {
                let _ = std::fs::remove_dir_all(entry.path());
                dropped += 1;
                continue;
            }
            if !is_sha256_hex(&name) {
                continue;
            }
            match read_entry(&entry.path(), &name) {
                Some((meta, manifest)) => {
                    let mut guard = self.state();
                    guard.entries.insert(
                        name.clone(),
                        ExtractedEntry {
                            sha256: name,
                            version_id: meta.version_id,
                            bytes: meta.bytes,
                            files: meta.files,
                            created_at: meta.created_at,
                            last_used_at: meta.last_used_at,
                            manifest: Arc::new(manifest),
                            persisted_at: Instant::now(),
                        },
                    );
                    recovered += 1;
                }
                None => {
                    let _ = std::fs::remove_dir_all(entry.path());
                    dropped += 1;
                }
            }
        }
        tracing::info!(
            recovered,
            dropped,
            bytes = self.used_bytes(),
            "extracted cache scanned"
        );
        self.enforce_budget(None);
    }

    fn persist_meta(&self, sha256: &str) {
        let meta = {
            let guard = self.state();
            let Some(entry) = guard.entries.get(sha256) else {
                return;
            };
            CacheEntryMeta {
                format_version: MANIFEST_FORMAT,
                options_key: OPTIONS_KEY.to_string(),
                archive_sha256: entry.sha256.clone(),
                version_id: entry.version_id,
                bytes: entry.bytes,
                files: entry.files,
                created_at: entry.created_at,
                last_used_at: entry.last_used_at,
            }
        };
        let path = self.entry_dir(sha256).join(META_NAME);
        if let Err(error) = write_json(&path, &meta) {
            tracing::debug!(%error, "could not refresh extracted cache metadata");
        }
    }
}

/// TTL sweeper. Runs until the server shuts down; no-op while disabled.
pub async fn scheduled_worker(state: AppState, mut cancel: watch::Receiver<bool>) {
    let cache = Arc::clone(&state.extracted);
    if !cache.enabled() {
        return;
    }
    let mut ticker = tokio::time::interval(cache.sweep_interval());
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let evicted = cache.sweep();
                if evicted > 0 {
                    tracing::info!(evicted, "extracted cache sweep finished");
                }
            }
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    break;
                }
            }
        }
    }
}

/// Spawns the extraction of one version into the cache.
///
/// The job ticket comes from [`ExtractedCache::submit`]; the task resolves the
/// archive (staging an object-store archive when needed), extracts it in a
/// blocking thread, publishes the entry and emits `version.extracted`.
pub fn spawn_extract(
    state: AppState,
    version: VersionRow,
    sha256: String,
    job: Arc<Mutex<Job>>,
    actor: Actor,
    ip: Option<std::net::IpAddr>,
) {
    tokio::spawn(async move {
        let cache = Arc::clone(&state.extracted);
        cache.mark_extracting(&job);
        let started = Instant::now();
        let outcome = extract_version(&state, &version, &sha256, &job).await;
        let result = cache.finish_extract(&sha256, &job, outcome);
        match &result {
            Ok(outcome) => {
                state.events.emit(
                    Event::new(crate::audit::actions::VERSION_EXTRACTED, actor)
                        .target("game_version", version.id.to_string())
                        .metadata(serde_json::json!({
                            "versionId": version.id,
                            "archiveSha256": sha256,
                            "files": outcome.files,
                            "bytes": outcome.bytes,
                            "durationMs": started.elapsed().as_millis() as u64,
                        }))
                        .ip(ip),
                );
                tracing::info!(
                    version = version.id,
                    sha256,
                    files = outcome.files,
                    bytes = outcome.bytes,
                    "archive extracted into the server cache"
                );
            }
            Err(error) => {
                tracing::warn!(
                    version = version.id,
                    sha256,
                    %error,
                    "archive extraction into the server cache failed"
                );
            }
        }
    });
}

async fn extract_version(
    state: &AppState,
    version: &VersionRow,
    sha256: &str,
    job: &Arc<Mutex<Job>>,
) -> Result<ExtractOutcome, CacheError> {
    let declared = u64::try_from(version.size).unwrap_or(u64::MAX);
    if declared > state.extracted.budget_bytes() {
        return Err(CacheError::Budget(declared));
    }
    let source = crate::transfer::zip::resolve_stream_source(state, version)
        .await
        .map_err(|error| CacheError::Source(format!("{}: {}", error.code, error.message)))?;
    if source.format == ArchiveFormat::Unknown {
        crate::transfer::object::cleanup_staged(&source.path, source.staged).await;
        return Err(CacheError::Unsupported);
    }
    let archive = source.path.clone();
    let cache = Arc::clone(&state.extracted);
    let progress_job = Arc::clone(job);
    let key = sha256.to_string();
    let built =
        tokio::task::spawn_blocking(move || extract_entry(&cache, &archive, &key, &progress_job))
            .await;
    crate::transfer::object::cleanup_staged(&source.path, source.staged).await;
    match built {
        Ok(result) => result,
        Err(error) => Err(CacheError::Join(error.to_string())),
    }
}

/// Extracts one archive into a temp directory and publishes it atomically.
fn extract_entry(
    cache: &ExtractedCache,
    archive: &Path,
    sha256: &str,
    job: &Arc<Mutex<Job>>,
) -> Result<ExtractOutcome, CacheError> {
    let report = verify_archive(archive, None)?;
    if report.encrypted {
        return Err(CacheError::Encrypted);
    }
    if report.entry_count > MAX_FILES {
        return Err(CacheError::TooManyFiles(report.entry_count));
    }
    if report.total_uncompressed > MAX_EXTRACTED_BYTES {
        return Err(CacheError::TooLarge(report.total_uncompressed));
    }

    let dest = cache.entry_dir(sha256);
    let temp = cache
        .root()
        .join(format!("{TEMP_PREFIX}{sha256}-{}", uuid::Uuid::new_v4()));
    let mut guard = TempDir::new(temp.clone());
    let files_root = temp.join(FILES_DIR);
    std::fs::create_dir_all(&files_root)?;

    let mut on_progress = |progress: ExtractProgress| {
        ExtractedCache::report_progress(job, &progress);
    };
    let options = ExtractOptions {
        preserve_modes: true,
        ..ExtractOptions::default()
    };
    let report = extract_archive(archive, &files_root, &options, &mut on_progress)?;

    // #240: the extractor hashed every byte it wrote, so the manifest is
    // assembled from the extraction result. This walk is metadata-only; a file
    // the extractor did not report (a legacy or skipped entry) is still hashed
    // as a fallback.
    let manifest = build_manifest(&files_root, sha256, &report.hashes)?;
    if manifest.file_count > MAX_FILES {
        return Err(CacheError::TooManyFiles(manifest.file_count));
    }
    write_json(&temp.join(MANIFEST_NAME), &manifest)?;
    let meta = CacheEntryMeta {
        format_version: MANIFEST_FORMAT,
        options_key: OPTIONS_KEY.to_string(),
        archive_sha256: sha256.to_string(),
        version_id: job
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .version_id,
        bytes: manifest.total_bytes,
        files: manifest.file_count,
        created_at: manifest.generated_at,
        last_used_at: manifest.generated_at,
    };
    write_json(&temp.join(META_NAME), &meta)?;

    if dest.exists() {
        // Another process won the race; keep the published entry.
        let _ = std::fs::remove_dir_all(&temp);
    } else {
        std::fs::rename(&temp, &dest)?;
    }
    guard.disarm();
    Ok(ExtractOutcome {
        files: manifest.file_count,
        bytes: manifest.total_bytes,
        created_at: manifest.generated_at,
        manifest: Arc::new(manifest),
    })
}

/// Deletes a temp directory unless it was published.
struct TempDir {
    path: PathBuf,
    armed: bool,
}

impl TempDir {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Walks an extracted tree and produces its manifest.
///
/// `extracted` carries the digests the extractor computed while writing
/// (#240): every file it lists is not reopened, so publishing an extracted
/// cache costs no second read of the archive content. A file the extractor did
/// not report is hashed here, which keeps the manifest complete.
fn build_manifest(
    root: &Path,
    sha256: &str,
    extracted: &[ludex_core::archive::ExtractedFile],
) -> Result<ExtractedManifest, CacheError> {
    let known: HashMap<&str, &str> = extracted
        .iter()
        .map(|file| (file.path.as_str(), file.sha256.as_str()))
        .collect();
    let mut files = Vec::new();
    let mut all_dirs: HashSet<String> = HashSet::new();
    let mut used_dirs: HashSet<String> = HashSet::new();
    let mut total = 0_u64;
    let mut pending: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];

    while let Some((directory, prefix)) = pending.pop() {
        let entries = std::fs::read_dir(&directory)?;
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(CacheError::InvalidPath(
                    entry.path().to_string_lossy().to_string(),
                ));
            };
            let relative = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}/{name}")
            };
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                return Err(CacheError::InvalidPath(format!(
                    "symlink in extracted tree: {relative}"
                )));
            }
            if file_type.is_dir() {
                all_dirs.insert(relative.clone());
                pending.push((path, relative));
                continue;
            }
            if !file_type.is_file() {
                return Err(CacheError::InvalidPath(format!(
                    "special file in extracted tree: {relative}"
                )));
            }
            let size = metadata.len();
            if size > MAX_FILE_BYTES {
                return Err(CacheError::FileTooLarge(relative, size));
            }
            total = total.saturating_add(size);
            if total > MAX_EXTRACTED_BYTES {
                return Err(CacheError::TooLarge(total));
            }
            if files.len() >= MAX_FILES {
                return Err(CacheError::TooManyFiles(files.len() + 1));
            }
            let digest = match known.get(relative.as_str()) {
                Some(digest) => digest.to_string(),
                None => crate::content_hash::hash_file(&path)?.1,
            };
            let mode = file_mode(&metadata);
            files.push(ExtractedFile {
                path: relative.clone(),
                size,
                sha256: digest,
                mode,
                executable: mode & 0o111 != 0,
            });
            let mut parent = relative.as_str();
            while let Some(index) = parent.rfind('/') {
                parent = &parent[..index];
                used_dirs.insert(parent.to_string());
            }
        }
    }

    files.sort_by(|left, right| left.path.cmp(&right.path));
    let mut directories: Vec<String> = all_dirs.difference(&used_dirs).cloned().collect();
    directories.sort();
    Ok(ExtractedManifest {
        format_version: MANIFEST_FORMAT,
        options_key: OPTIONS_KEY.to_string(),
        archive_sha256: sha256.to_string(),
        generated_at: Utc::now(),
        total_bytes: total,
        file_count: files.len(),
        files,
        directories,
    })
}

#[cfg(unix)]
fn file_mode(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn file_mode(_metadata: &std::fs::Metadata) -> u32 {
    0o644
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), CacheError> {
    let bytes = serde_json::to_vec(value)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Reads and validates one cache directory.
fn read_entry(directory: &Path, sha256: &str) -> Option<(CacheEntryMeta, ExtractedManifest)> {
    if !directory.join(FILES_DIR).is_dir() {
        return None;
    }
    let meta: CacheEntryMeta =
        serde_json::from_slice(&std::fs::read(directory.join(META_NAME)).ok()?).ok()?;
    if meta.format_version != MANIFEST_FORMAT
        || meta.options_key != OPTIONS_KEY
        || meta.archive_sha256 != sha256
    {
        return None;
    }
    let manifest: ExtractedManifest =
        serde_json::from_slice(&std::fs::read(directory.join(MANIFEST_NAME)).ok()?).ok()?;
    if manifest.format_version != MANIFEST_FORMAT
        || manifest.options_key != OPTIONS_KEY
        || manifest.archive_sha256 != sha256
    {
        return None;
    }
    Some((meta, manifest))
}

/// Empty manifest, used only by the defensive path of [`EntryLease::manifest`].
fn empty_manifest(sha256: &str) -> ExtractedManifest {
    ExtractedManifest {
        format_version: MANIFEST_FORMAT,
        options_key: OPTIONS_KEY.to_string(),
        archive_sha256: sha256.to_string(),
        generated_at: Utc::now(),
        total_bytes: 0,
        file_count: 0,
        files: Vec::new(),
        directories: Vec::new(),
    }
}

/// Whether `value` is a lowercase hex SHA-256.
#[must_use]
pub fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::fs;

    fn cache_root() -> (tempfile::TempDir, Config, ExtractedCache) {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = Config::for_local(
            temp.path().join("games"),
            temp.path().join("data"),
            temp.path().join("media"),
        );
        config.extracted_cache_enabled = true;
        config.extracted_cache_budget_bytes = 4 * 1024 * 1024;
        let cache = ExtractedCache::new(&config);
        fs::create_dir_all(cache.root()).expect("root");
        (temp, config, cache)
    }

    fn publish(cache: &ExtractedCache, sha: &str, bytes: u64) {
        let dir = cache.entry_dir(sha);
        fs::create_dir_all(cache.files_dir(sha)).expect("files");
        let manifest = ExtractedManifest {
            format_version: MANIFEST_FORMAT,
            options_key: OPTIONS_KEY.to_string(),
            archive_sha256: sha.to_string(),
            generated_at: Utc::now(),
            total_bytes: bytes,
            file_count: 1,
            files: vec![ExtractedFile {
                path: "game.bin".to_string(),
                size: bytes,
                sha256: sha.to_string(),
                mode: 0o644,
                executable: false,
            }],
            directories: Vec::new(),
        };
        write_json(&dir.join(MANIFEST_NAME), &manifest).expect("manifest");
        write_json(
            &dir.join(META_NAME),
            &CacheEntryMeta {
                format_version: MANIFEST_FORMAT,
                options_key: OPTIONS_KEY.to_string(),
                archive_sha256: sha.to_string(),
                version_id: 1,
                bytes,
                files: 1,
                created_at: Utc::now(),
                last_used_at: Utc::now(),
            },
        )
        .expect("meta");
        cache.state().entries.insert(
            sha.to_string(),
            ExtractedEntry {
                sha256: sha.to_string(),
                version_id: 1,
                bytes,
                files: 1,
                created_at: Utc::now(),
                last_used_at: Utc::now(),
                manifest: Arc::new(manifest),
                persisted_at: Instant::now(),
            },
        );
    }

    fn sha(seed: char) -> String {
        seed.to_string().repeat(64)
    }

    #[test]
    fn sha256_keys_are_validated() {
        assert!(is_sha256_hex(&sha('a')));
        assert!(!is_sha256_hex(&"A".repeat(64)));
        assert!(!is_sha256_hex("../etc"));
        assert!(!is_sha256_hex(""));
    }

    #[test]
    fn budget_evicts_the_least_recently_used_entry() {
        let (_temp, config, cache) = cache_root();
        assert!(config.extracted_cache_enabled);
        let older = sha('a');
        let newer = sha('b');
        publish(&cache, &older, 2 * 1024 * 1024);
        cache
            .state()
            .entries
            .get_mut(&older)
            .expect("older")
            .last_used_at = Utc::now() - chrono::Duration::hours(2);
        publish(&cache, &newer, 2 * 1024 * 1024);
        assert_eq!(cache.used_bytes(), 4 * 1024 * 1024);
        assert_eq!(cache.enforce_budget(None), 0);

        publish(&cache, &sha('c'), 2 * 1024 * 1024);
        assert_eq!(cache.enforce_budget(Some(&sha('c'))), 1);
        assert!(!cache.has_entry(&older));
        assert!(cache.has_entry(&newer));
        assert!(!cache.entry_dir(&older).exists());
    }

    #[test]
    fn a_served_entry_is_never_evicted() {
        let (_temp, _config, cache) = cache_root();
        let leased = sha('a');
        let other = sha('b');
        let newest = sha('c');
        publish(&cache, &leased, 2 * 1024 * 1024);
        publish(&cache, &other, 2 * 1024 * 1024);
        publish(&cache, &newest, 2 * 1024 * 1024);

        let lease = cache.acquire(&leased).expect("lease");
        // The leased entry is the oldest, yet eviction takes the unprotected
        // one instead of deleting bytes under a live reader.
        assert_eq!(cache.enforce_budget(Some(&newest)), 1);
        assert!(cache.has_entry(&leased));
        assert!(!cache.has_entry(&other));
        assert!(cache.has_entry(&newest));
        // A served entry also refuses an explicit delete.
        assert!(!cache.remove(&leased));
        drop(lease);
        assert!(cache.remove(&leased));
    }

    #[test]
    fn ttl_removes_unused_entries() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = Config::for_local(
            temp.path().join("games"),
            temp.path().join("data"),
            temp.path().join("media"),
        );
        config.extracted_cache_enabled = true;
        config.extracted_cache_budget_bytes = 4 * 1024 * 1024;
        config.extracted_cache_ttl = Duration::from_secs(60);
        let cache = ExtractedCache::new(&config);
        fs::create_dir_all(cache.root()).expect("root");

        let stale = sha('a');
        publish(&cache, &stale, 1024);
        cache
            .state()
            .entries
            .get_mut(&stale)
            .expect("entry")
            .last_used_at = Utc::now() - chrono::Duration::hours(1);
        let fresh = sha('b');
        publish(&cache, &fresh, 1024);

        assert_eq!(cache.sweep(), 1);
        assert!(!cache.has_entry(&stale));
        assert!(cache.has_entry(&fresh));
    }

    #[test]
    fn submit_dedupes_a_running_job() {
        let (_temp, _config, cache) = cache_root();
        let sha_a = sha('a');
        let job = match cache.submit(&sha_a, 7) {
            Submission::Started(job) => job,
            _ => panic!("first submit must start a job"),
        };
        cache.mark_extracting(&job);
        match cache.submit(&sha_a, 7) {
            Submission::Running(status) => {
                assert_eq!(status.status, ExtractState::Extracting);
                assert_eq!(status.version_id, 7);
            }
            _ => panic!("second submit must join the running job"),
        }
        let outcome = ExtractOutcome {
            files: 1,
            bytes: 2,
            created_at: Utc::now(),
            manifest: Arc::new(empty_manifest(&sha_a)),
        };
        cache
            .finish_extract(&sha_a, &job, Ok(outcome))
            .expect("finish");
        match cache.submit(&sha_a, 7) {
            Submission::Ready(status) => assert_eq!(status.status, ExtractState::Ready),
            _ => panic!("ready entry must be reported"),
        }
    }

    #[test]
    fn a_failed_job_is_retained_then_retried() {
        let (_temp, _config, cache) = cache_root();
        let sha_a = sha('a');
        let job = match cache.submit(&sha_a, 3) {
            Submission::Started(job) => job,
            _ => panic!("job"),
        };
        cache
            .finish_extract(&sha_a, &job, Err(CacheError::Encrypted))
            .expect_err("failed");
        match cache.submit(&sha_a, 3) {
            Submission::Running(status) => {
                assert_eq!(status.status, ExtractState::Failed);
                assert!(status.error.is_some());
            }
            _ => panic!("failure must be reported for a while"),
        }
        {
            let guard = cache.state();
            guard
                .jobs
                .get(&sha_a)
                .expect("job")
                .lock()
                .expect("job lock")
                .failed_at = Some(Instant::now() - FAILED_RETENTION - Duration::from_secs(1));
        }
        assert!(matches!(cache.submit(&sha_a, 3), Submission::Started(_)));
    }

    #[test]
    fn startup_scan_rebuilds_and_drops_broken_entries() {
        let (_temp, _config, cache) = cache_root();
        let good = sha('a');
        publish(&cache, &good, 1024);
        let broken = sha('b');
        fs::create_dir_all(cache.files_dir(&broken)).expect("files");
        fs::write(cache.entry_dir(&broken).join(MANIFEST_NAME), b"{}").expect("manifest");
        let leftover = cache.root().join(format!("{TEMP_PREFIX}leftover"));
        fs::create_dir_all(&leftover).expect("temp");

        let rebuilt = ExtractedCache {
            inner: Arc::new(Inner {
                enabled: true,
                root: cache.root().to_path_buf(),
                budget_bytes: cache.budget_bytes(),
                ttl: cache.ttl(),
                state: Mutex::new(CacheState::default()),
            }),
        };
        rebuilt.scan_root();
        assert!(rebuilt.has_entry(&good));
        assert!(!rebuilt.has_entry(&broken));
        assert!(!leftover.exists());
        assert!(!rebuilt.entry_dir(&broken).exists());
    }

    #[test]
    fn manifest_walk_lists_files_modes_and_empty_directories() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        fs::create_dir_all(root.join("Game/bin")).expect("dirs");
        fs::create_dir_all(root.join("Game/empty")).expect("dirs");
        fs::write(root.join("Game/bin/game"), b"binary").expect("file");
        fs::write(root.join("Game/readme.txt"), b"hello").expect("file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                root.join("Game/bin/game"),
                fs::Permissions::from_mode(0o755),
            )
            .expect("chmod");
        }
        let manifest = build_manifest(root, &sha('a'), &[]).expect("manifest");
        assert_eq!(manifest.file_count, 2);
        assert_eq!(manifest.total_bytes, 11);
        assert_eq!(manifest.files[0].path, "Game/bin/game");
        assert!(manifest.files[0].executable);
        assert_eq!(manifest.files[1].path, "Game/readme.txt");
        assert_eq!(manifest.directories, vec!["Game/empty".to_string()]);
        let digest = manifest
            .files
            .iter()
            .find(|file| file.path == "Game/bin/game")
            .map(|file| file.sha256.clone())
            .expect("digest");
        use sha2::{Digest, Sha256};
        assert_eq!(digest, hex::encode(Sha256::digest(b"binary")));
    }
}
