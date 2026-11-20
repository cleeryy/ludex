//! Automatic cloud save sync (#108).
//!
//! After a game exits, the detected save paths (#107) are archived into one
//! bounded zip, hashed, and uploaded to the user's Ludex server as an `auto`
//! snapshot. Uploads are idempotent by content hash: an unchanged save is a
//! no-op, and the server returns the existing snapshot if the same bytes were
//! already uploaded from another machine.
//!
//! Everything here is local-first. Host paths never leave the machine and the
//! server only ever receives an opaque archive. Tokens and save contents are
//! never logged.
//!
//! Archive layout (restore relies on it):
//!
//! ```text
//! saves/00/<relative path inside the first save path>
//! saves/01/<relative path inside the second save path>
//! …
//! ```
//!
//! The index matches the order of `db::list_save_paths` (primary first, then
//! path order). A save path that is a file contains exactly one entry named
//! after the file. Symlinks are skipped both when archiving and when extracting.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::bridge::save_conflict::{
    decide, ConflictReason, SaveDecision, SaveSide, SaveSideKind, CLOCK_SKEW_LIMIT_SECS,
};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::db::{self, CloudSavePrefs, SavePathRecord};
use crate::saves::identity::{self, Layout, MappingPlan, SaveIdentity, SaveRoots};
use crate::settings::LocalSettings;
use crate::state::AppState;
use crate::util::{now_rfc3339, short_id};

/// Wait after `game://exited` before archiving, so the game's final writes
/// (and Steam Cloud, if any) settle first (#108).
const EXIT_DEBOUNCE: Duration = Duration::from_secs(3);
/// Upper bound for one archive, compressed or extracted. The server accepts
/// 2 GiB snapshots; the extra headroom keeps the error local and actionable.
const MAX_ARCHIVE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Upper bounds on entries and directory depth: a save folder is not a game
/// install, and these caps keep the walk bounded on pathological trees.
const MAX_ARCHIVE_ENTRIES: u64 = 50_000;
const MAX_WALK_DEPTH: usize = 64;
/// The game page preview is much tighter than an archive (#159): it is a
/// read-only status, so a wrong root (Documents, a shared folder, a junction)
/// must not walk the disk. A truncated preview still reports "files exist".
const MAX_PREVIEW_FILES: u64 = 4_000;
const MAX_PREVIEW_DEPTH: usize = 32;
const PREVIEW_BUDGET: Duration = Duration::from_millis(200);
/// The cloud-save page reuses a preview scan for this long. Writes that change
/// the mapping or the files (detection, sync, restore) invalidate it sooner.
const STATUS_CACHE_TTL: Duration = Duration::from_secs(45);
/// Bounded retry: five failures, exponential backoff starting at 30 s.
const MAX_RETRY_ATTEMPTS: i64 = 5;
const RETRY_BASE_SECS: i64 = 30;
/// File timestamps have sub-second slop; a save written right before an
/// upload must not look "changed" forever.
const CONFLICT_TOLERANCE_SECS: i64 = 2;
/// Mirrors the server's snapshot label limit.
const MAX_LABEL_CHARS: usize = 200;

/* ------------------------------------------------------------------ types */

/// Effective automatic-sync state: per-game override, else the global setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSaveToggle {
    pub enabled: bool,
    /// `on`, `global_off` or `game_off`.
    pub reason: String,
}

/// One save path as shown on the game page. The path itself stays local.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSavePath {
    pub path: String,
    pub kind: String,
    pub primary: bool,
    pub exists: bool,
    pub files: u64,
    pub size_bytes: u64,
    pub last_modified: Option<String>,
    /// Logical identity (#294): game + root kind + relative path. `None` when
    /// the path is outside every known root. Never contains a host path.
    pub identity: Option<String>,
    /// `documents`, `saved-games`, `roaming-app-data`, … when classified.
    pub root_kind: Option<String>,
    /// Path below the root, with `/` separators.
    pub relative_path: Option<String>,
    /// `windows`, `proton`, `linux`, `macos` or `unknown`.
    pub origin_platform: String,
    /// `present`, `translated`, `pending_mapping`, `other_platform` or
    /// `missing`. A root that only exists on the other platform is
    /// `other_platform`, never reported as "missing" (#294).
    pub availability: String,
    /// Folder the identity resolved to on this machine, when the stored path
    /// is absent but a validated equivalent exists.
    pub resolved_path: Option<String>,
}

/// One identity the current machine cannot resolve without a user choice
/// (#294). The game page asks once; nothing is picked silently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSavePendingMapping {
    pub game_id: i64,
    pub identity: String,
    pub root_kind: String,
    pub relative_path: String,
    /// `ambiguous` — several candidates validated; `rejected` — a candidate
    /// existed but did not look like the same save.
    pub reason: String,
    pub candidates: Vec<String>,
}

/// One detected save folder the game page reports automatically (#180).
/// `status` is `saving` (archived), `waiting` (no session proved it yet) or
/// `skipped` (rejected by the safety rules, with a readable `reason`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSaveCandidate {
    pub path: String,
    pub source: String,
    pub confidence: String,
    pub status: String,
    pub reason: Option<String>,
    pub primary: bool,
    pub size_bytes: u64,
}

/// Latest snapshot, as returned by the server (`SaveSnapshot`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSnapshotSummary {
    pub id: i64,
    pub game_id: i64,
    pub version_id: Option<i64>,
    pub label: String,
    pub file_name: String,
    pub size: i64,
    pub sha256: String,
    pub created_at: String,
    pub source: String,
    #[serde(default)]
    pub device_id: Option<String>,
    /// Newest save file mtime on the uploading device (#296).
    #[serde(default)]
    pub client_mtime: Option<String>,
    /// Device clock minus server clock at upload time (#296).
    #[serde(default)]
    pub clock_offset_secs: i64,
    /// The game's own save counter, when the uploading client knew it (#296).
    #[serde(default)]
    pub save_seq: Option<i64>,
    /// Set when this row is the losing side of a conflict (#296). Such a row is
    /// a restore point, never the latest save.
    #[serde(default)]
    pub conflict_of: Option<i64>,
}

impl SaveSnapshotSummary {
    /// The canonical save among `snapshots` (newest first): conflict copies are
    /// restore points and never become "latest".
    pub fn latest_of(snapshots: &[Self]) -> Option<&Self> {
        snapshots
            .iter()
            .find(|snapshot| snapshot.conflict_of.is_none())
    }
}

/// Pending upload state, persisted in SQLite so it survives a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSavePending {
    /// True while an upload is waiting for a retry or failed permanently.
    pub pending: bool,
    pub attempts: i64,
    pub next_attempt_at: Option<String>,
    pub error: Option<String>,
}

/// Conflict between local save files and a newer server snapshot.
///
/// Since #296 both sides are described so the UI can show them side by side;
/// the server keeps both regardless of the choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSaveConflict {
    pub local_newest_mtime: Option<String>,
    pub last_upload_at: Option<String>,
    pub last_hash: Option<String>,
    pub remote_sha256: String,
    pub remote_created_at: String,
    pub remote_device_id: Option<String>,
    /// This machine's device id and readable label.
    pub local_device_id: Option<String>,
    pub local_device_label: Option<String>,
    /// Local save folder totals, for the side-by-side choice.
    pub local_files: u64,
    pub local_bytes: u64,
    /// Server archive totals.
    pub remote_size: i64,
    pub remote_label: String,
    /// `both_changed`, `clock_skew` or `counter_diverged`.
    pub reason: String,
    /// Device clock minus server clock, in seconds (#296).
    pub clock_offset_secs: i64,
    /// True when the clock is too far from the server to order the saves.
    pub clock_skewed: bool,
    /// Open conflict row on the server, when the server saw this first.
    pub server_conflict_id: Option<i64>,
}

/// Per-game conflict policy (#296). `ask` is the default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSavePolicy {
    /// `ask` or `prefer_device`.
    pub policy: String,
    /// Device that always wins when `policy` is `prefer_device`.
    pub device_id: Option<String>,
}

impl Default for CloudSavePolicy {
    fn default() -> Self {
        Self {
            policy: "ask".to_string(),
            device_id: None,
        }
    }
}

impl CloudSavePolicy {
    /// True when this machine is the configured winner.
    pub fn prefers(&self, device_id: Option<&str>) -> bool {
        self.policy == "prefer_device"
            && self.device_id.is_some()
            && self.device_id.as_deref() == device_id
    }
}

/// One open conflict row from the server (#296).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSaveConflict {
    pub id: i64,
    pub game_id: i64,
    pub status: String,
    pub reason: String,
    pub detected_at: String,
    #[serde(default)]
    pub resolved_at: Option<String>,
    #[serde(default)]
    pub strategy: Option<String>,
    pub existing: SaveSnapshotSummary,
    pub candidate: SaveSnapshotSummary,
}

/// Everything the game page needs to render the cloud saves section.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSaveStatus {
    pub game_id: i64,
    /// A server URL and token are configured.
    pub server_configured: bool,
    pub paths: Vec<CloudSavePath>,
    /// Guessed folders and their automatic decision (#180).
    pub candidates: Vec<CloudSaveCandidate>,
    pub local_files: u64,
    pub local_bytes: u64,
    pub local_newest_mtime: Option<String>,
    pub latest: Option<SaveSnapshotSummary>,
    pub last_hash: Option<String>,
    pub last_upload_at: Option<String>,
    pub last_restore_at: Option<String>,
    pub toggle: CloudSaveToggle,
    pub conflict: Option<CloudSaveConflict>,
    /// Per-game conflict policy (#296), `ask` when the server is unreachable.
    pub policy: CloudSavePolicy,
    /// Device clock minus server clock, in seconds (#296). 0 when unknown.
    pub clock_offset_secs: i64,
    pub pending: CloudSavePending,
    /// Save identities that need one explicit choice on this machine (#294).
    pub pending_mappings: Vec<CloudSavePendingMapping>,
    pub device_id: Option<String>,
    /// When save detection last ran for this game, `None` when it never did.
    /// The game page uses it to decide whether to run detection on first open
    /// (#158).
    pub last_detection_at: Option<String>,
    /// Human-readable context, never an error dump. Remote failures use this
    /// field instead of failing the whole status command.
    pub message: Option<String>,
}

/// Result of `set_cloud_save_sync`. `gameId` is `null` for the global setting.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncToggleResponse {
    pub game_id: Option<i64>,
    pub global_enabled: bool,
    pub effective: CloudSaveToggle,
}

/// Restore strategy accepted by `restore_cloud_save`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreStrategy {
    /// Download the server snapshot and write it over the local save files.
    Remote,
    /// Keep the local files, adopt them as the new baseline and upload them.
    KeepLocal,
}

impl RestoreStrategy {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim() {
            "remote" => Ok(Self::Remote),
            "keep_local" => Ok(Self::KeepLocal),
            other => Err(format!(
                "Unknown restore strategy '{other}'. Use 'remote' or 'keep_local'."
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Remote => "remote",
            Self::KeepLocal => "keep_local",
        }
    }
}

/// Outcome of a restore or conflict resolution.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOutcome {
    pub restored: bool,
    pub strategy: String,
    pub snapshot_id: Option<i64>,
    /// Archive written before overwriting newer local files, if any.
    pub backup_path: Option<String>,
    pub message: String,
}

/* ------------------------------------------------------------ local scan */

/// Aggregate state of every configured save path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalSaveState {
    pub files: u64,
    pub bytes: u64,
    pub newest: Option<DateTime<Utc>>,
    pub paths: Vec<LocalPathState>,
    /// The walk stopped at its file or time cap before seeing everything. A
    /// truncated scan must not be reported as "no save files exist" (#159).
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPathState {
    pub path: String,
    pub kind: String,
    pub primary: bool,
    pub exists: bool,
    pub files: u64,
    pub bytes: u64,
    pub newest: Option<DateTime<Utc>>,
}

/// Caps for one scan. The archive walk sees everything it needs; the game page
/// preview is deliberately shallow and time-bounded so opening a page never
/// walks a mistaken `Documents` root (#159).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanLimits {
    pub max_files: u64,
    pub max_depth: usize,
    /// Stop when this budget is exhausted. `None` only for archiving.
    pub budget: Option<Duration>,
}

impl ScanLimits {
    /// Status/preview scan: bounded file count, bounded depth, hard deadline.
    pub const PREVIEW: Self = Self {
        max_files: MAX_PREVIEW_FILES,
        max_depth: MAX_PREVIEW_DEPTH,
        budget: Some(PREVIEW_BUDGET),
    };

    /// Archive scan: the full tree, up to the archive caps.
    pub const ARCHIVE: Self = Self {
        max_files: MAX_ARCHIVE_ENTRIES,
        max_depth: MAX_WALK_DEPTH,
        budget: None,
    };
}

/// Scan the configured save paths without following symlinks, with the archive
/// caps. The game page uses the bounded preview below instead (#159); this stays
/// for exact counts in tests and maintenance paths.
#[allow(dead_code)]
pub fn scan_local_state(rows: &[SavePathRecord]) -> LocalSaveState {
    scan_local_state_with(rows, ScanLimits::ARCHIVE)
}

/// Scan with explicit caps. Every `files`/`bytes` figure is a lower bound when
/// `truncated` is true.
pub fn scan_local_state_with(rows: &[SavePathRecord], limits: ScanLimits) -> LocalSaveState {
    let deadline = limits
        .budget
        .map(|budget| std::time::Instant::now() + budget);
    let mut state = LocalSaveState::default();
    for row in rows {
        let path = Path::new(&row.path);
        let metadata = fs::symlink_metadata(path).ok();
        let exists = metadata.is_some() && !is_symlink(&metadata);
        let mut walk = TreeScan {
            files: 0,
            bytes: 0,
            newest: None,
            seen: 0,
            truncated: false,
            limits,
            deadline,
        };
        if exists {
            walk.visit(path, 0);
        }
        state.files += walk.files;
        state.bytes += walk.bytes;
        state.truncated |= walk.truncated;
        state.newest = match (state.newest, walk.newest) {
            (Some(current), Some(candidate)) => Some(current.max(candidate)),
            (None, candidate) => candidate,
            (current, None) => current,
        };
        state.paths.push(LocalPathState {
            path: row.path.clone(),
            kind: row.kind.clone(),
            primary: row.primary,
            exists,
            files: walk.files,
            bytes: walk.bytes,
            newest: walk.newest,
        });
    }
    state
}

struct TreeScan {
    files: u64,
    bytes: u64,
    newest: Option<DateTime<Utc>>,
    /// Entries visited, used to check the time budget without a syscall per
    /// file.
    seen: u64,
    truncated: bool,
    limits: ScanLimits,
    deadline: Option<std::time::Instant>,
}

impl TreeScan {
    fn out_of_budget(&mut self) -> bool {
        if self.files >= self.limits.max_files {
            self.truncated = true;
            return true;
        }
        if let Some(deadline) = self.deadline {
            if self.seen.is_multiple_of(256) && std::time::Instant::now() >= deadline {
                self.truncated = true;
                return true;
            }
        }
        false
    }

    fn visit(&mut self, path: &Path, depth: usize) {
        self.seen += 1;
        if depth > self.limits.max_depth || self.out_of_budget() {
            return;
        }
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => return,
        };
        if metadata.file_type().is_symlink() {
            return;
        }
        if metadata.is_file() {
            if self.files >= self.limits.max_files {
                self.truncated = true;
                return;
            }
            self.files += 1;
            self.bytes = self.bytes.saturating_add(metadata.len());
            self.newest = max_time(self.newest, metadata.modified().ok());
            return;
        }
        if !metadata.is_dir() {
            return;
        }
        let mut entries = match fs::read_dir(path) {
            Ok(entries) => entries.flatten().collect::<Vec<_>>(),
            Err(_) => return,
        };
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if self.out_of_budget() {
                return;
            }
            self.visit(&entry.path(), depth + 1);
        }
    }
}

fn max_time(
    current: Option<DateTime<Utc>>,
    candidate: Option<std::time::SystemTime>,
) -> Option<DateTime<Utc>> {
    let candidate = candidate.map(DateTime::<Utc>::from);
    match (current, candidate) {
        (Some(current), Some(candidate)) => Some(current.max(candidate)),
        (None, candidate) => candidate,
        (current, None) => current,
    }
}

fn is_symlink(metadata: &Option<fs::Metadata>) -> bool {
    metadata
        .as_ref()
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

/* ------------------------------------------------------------ scan cache */

/// Stable fingerprint of the save-path mapping, so a changed path or primary
/// flag invalidates the cached scan without touching the disk.
pub fn rows_fingerprint(rows: &[SavePathRecord]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for row in rows {
        row.path.hash(&mut hasher);
        row.kind.hash(&mut hasher);
        row.primary.hash(&mut hasher);
    }
    hasher.finish()
}

struct CachedScan {
    fingerprint: u64,
    scanned_at: std::time::Instant,
    state: LocalSaveState,
}

/// Bounded cache of status scans. Unit-tested directly; the process-wide
/// instance below only wraps it in a mutex.
#[derive(Default)]
pub struct ScanCache {
    entries: HashMap<i64, CachedScan>,
}

impl ScanCache {
    pub fn get(
        &mut self,
        game_id: i64,
        rows: &[SavePathRecord],
        now: std::time::Instant,
        ttl: Duration,
    ) -> Option<LocalSaveState> {
        let cached = self.entries.get(&game_id)?;
        let fresh = now
            .checked_duration_since(cached.scanned_at)
            .map(|age| age <= ttl)
            .unwrap_or(false);
        if !fresh || cached.fingerprint != rows_fingerprint(rows) {
            self.entries.remove(&game_id);
            return None;
        }
        Some(cached.state.clone())
    }

    pub fn insert(
        &mut self,
        game_id: i64,
        rows: &[SavePathRecord],
        now: std::time::Instant,
        state: LocalSaveState,
    ) {
        self.entries.insert(
            game_id,
            CachedScan {
                fingerprint: rows_fingerprint(rows),
                scanned_at: now,
                state,
            },
        );
    }

    pub fn invalidate(&mut self, game_id: i64) {
        self.entries.remove(&game_id);
    }
}

fn scan_cache() -> &'static Mutex<ScanCache> {
    static CACHE: OnceLock<Mutex<ScanCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(ScanCache::default()))
}

/// Drop the cached preview for one game after any write that can change the
/// save files or the mapping (detection, sync, restore, toggle).
pub fn invalidate_scan_cache(game_id: i64) {
    if let Ok(mut cache) = scan_cache().lock() {
        cache.invalidate(game_id);
    }
}

/// Cached, bounded preview scan. Blocking: call it from `spawn_blocking`.
pub fn preview_scan(game_id: i64, rows: &[SavePathRecord]) -> LocalSaveState {
    let now = std::time::Instant::now();
    if let Ok(mut cache) = scan_cache().lock() {
        if let Some(state) = cache.get(game_id, rows, now, STATUS_CACHE_TTL) {
            return state;
        }
    }
    let state = scan_local_state_with(rows, ScanLimits::PREVIEW);
    if let Ok(mut cache) = scan_cache().lock() {
        cache.insert(game_id, rows, now, state.clone());
    }
    state
}

/* --------------------------------------------------- cross-platform identity */

/// Identity and reachability of one configured save path (#294).
#[derive(Debug, Clone, Default)]
struct SavePathIdentity {
    identity: Option<String>,
    root_kind: Option<String>,
    relative_path: Option<String>,
    origin_platform: String,
    availability: String,
    resolved_path: Option<String>,
}

/// Whether a save root can be reached on this machine at all. A Windows-only
/// root on Linux, or an XDG root on Windows, is one-platform — not missing.
fn base_availability(
    exists: bool,
    identity: Option<&SaveIdentity>,
    origin: Option<Layout>,
    roots: &SaveRoots,
) -> &'static str {
    if exists {
        return "present";
    }
    if identity.is_some_and(|identity| !roots.resolvable(identity)) {
        return "other_platform";
    }
    let other_platform = match origin {
        Some(Layout::Windows) => !cfg!(windows),
        Some(Layout::Proton) => cfg!(windows),
        Some(Layout::Linux) => !cfg!(target_os = "linux"),
        Some(Layout::Macos) => !cfg!(target_os = "macos"),
        None => false,
    };
    if other_platform {
        "other_platform"
    } else {
        "missing"
    }
}

/// Resolve every configured path to its logical identity and, when the path is
/// absent here, look for the same identity under this machine's roots. Blocking
/// (bounded tree reads); call it from `spawn_blocking`.
fn identity_report(
    game_id: i64,
    rows: &[SavePathRecord],
    local: &LocalSaveState,
    wine_prefix: &str,
    steam_root: &str,
    mappings: &[identity::SaveIdentityMapping],
) -> (Vec<SavePathIdentity>, Vec<CloudSavePendingMapping>) {
    let mut roots = SaveRoots::current(wine_prefix, steam_root);
    for row in rows {
        roots.add_prefix_from_path(Path::new(&row.path));
    }
    let mut states = Vec::with_capacity(rows.len());
    let mut pending = Vec::new();

    for (index, row) in rows.iter().enumerate() {
        let path = Path::new(&row.path);
        let exists = local
            .paths
            .get(index)
            .map(|state| state.exists)
            .unwrap_or(false);
        let classified = roots.classify(path);
        let identity = classified
            .as_ref()
            .and_then(|found| SaveIdentity::new(game_id, found.root, &found.relative).ok());
        let origin = classified
            .as_ref()
            .map(|found| found.layout)
            .or_else(|| identity::infer_layout(path));
        let mut availability = base_availability(exists, identity.as_ref(), origin, &roots);
        let mut resolved_path = None;

        if !exists {
            if let Some(identity) = &identity {
                let chosen = mappings
                    .iter()
                    .find(|mapping| {
                        mapping.game_id == game_id
                            && mapping
                                .identity
                                .eq_ignore_ascii_case(&identity.serialized())
                    })
                    .map(|mapping| PathBuf::from(mapping.path.trim()))
                    .filter(|path| path.exists());
                let plan = match chosen {
                    Some(path) => MappingPlan::Resolved { path },
                    None => identity::plan_mapping(identity, &roots, Some(path)),
                };
                match plan {
                    MappingPlan::Resolved { path } => {
                        availability = "translated";
                        resolved_path = Some(path.to_string_lossy().to_string());
                    }
                    MappingPlan::Ambiguous { candidates } => {
                        availability = "pending_mapping";
                        pending.push(CloudSavePendingMapping {
                            game_id,
                            identity: identity.serialized(),
                            root_kind: identity.root().slug().to_string(),
                            relative_path: identity.relative().to_string(),
                            reason: "ambiguous".to_string(),
                            candidates: candidates
                                .iter()
                                .map(|path| path.to_string_lossy().to_string())
                                .collect(),
                        });
                    }
                    MappingPlan::Rejected { candidate, reason } => {
                        tracing::debug!(
                            game_id,
                            candidate = %candidate.display(),
                            reason,
                            "save identity candidate rejected"
                        );
                        pending.push(CloudSavePendingMapping {
                            game_id,
                            identity: identity.serialized(),
                            root_kind: identity.root().slug().to_string(),
                            relative_path: identity.relative().to_string(),
                            reason: "rejected".to_string(),
                            candidates: vec![candidate.to_string_lossy().to_string()],
                        });
                    }
                    MappingPlan::Unresolved => {}
                }
            }
        }

        states.push(SavePathIdentity {
            identity: identity.as_ref().map(SaveIdentity::serialized),
            root_kind: identity
                .as_ref()
                .map(|identity| identity.root().slug().to_string()),
            relative_path: identity
                .as_ref()
                .map(|identity| identity.relative().to_string()),
            origin_platform: origin.map(Layout::as_str).unwrap_or("unknown").to_string(),
            availability: availability.to_string(),
            resolved_path,
        });
    }
    (states, pending)
}

/* -------------------------------------------------------------- archive */

/// A written archive and its content hash.
#[derive(Debug, Clone)]
pub struct ArchiveInfo {
    pub path: PathBuf,
    pub files: u64,
    pub bytes: u64,
    pub sha256: String,
    /// Newest save file mtime seen while archiving (#296), in the device clock.
    pub newest: Option<DateTime<Utc>>,
}

/// Archive every existing save path into one zip.
///
/// Missing paths are skipped (a game that has not created its saves yet is a
/// no-op, never an empty snapshot). A file path contributes one entry; a
/// directory contributes its whole tree. Symlinks are skipped.
pub fn create_archive(rows: &[SavePathRecord], dest: &Path) -> Result<ArchiveInfo, String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Could not create the save archive folder: {e}"))?;
    }
    let file = File::create(dest).map_err(|e| format!("Could not create the save archive: {e}"))?;
    // A fixed timestamp makes identical content produce identical archive
    // bytes, so a CRLF build (#294) still matches the previous upload's hash.
    let options = SimpleFileOptions::default()
        .large_file(true)
        .last_modified_time(zip::DateTime::default());
    let mut writer = ZipWriter::new(file);
    let mut builder = ArchiveBuilder {
        files: 0,
        bytes: 0,
        newest: None,
        archived: Vec::new(),
        seen: HashSet::new(),
    };

    for (index, row) in rows.iter().enumerate() {
        let root = PathBuf::from(&row.path);
        let metadata = match fs::symlink_metadata(&root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("Could not read a save path: {error}")),
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        // Two save paths can overlap (`<documents>/Game` and its subfolder);
        // archive the outer one once.
        if builder
            .archived
            .iter()
            .any(|kept: &PathBuf| root.starts_with(kept))
        {
            continue;
        }
        builder.archived.push(root.clone());

        let prefix = format!("saves/{index:02}");
        if metadata.is_file() {
            let name = root
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "save".to_string());
            let entry = format!("{prefix}/{}", sanitize_component(&name));
            builder.add_file(&mut writer, &root, &entry, options)?;
        } else if metadata.is_dir() {
            builder.add_directory(&mut writer, &prefix, options)?;
            builder.add_tree(&mut writer, &root, &prefix, 0, options)?;
        }
    }

    writer
        .finish()
        .map_err(|e| format!("Could not finish the save archive: {e}"))?;
    let sha256 = sha256_file(dest)?;
    let bytes = fs::metadata(dest).map(|meta| meta.len()).unwrap_or(0);
    Ok(ArchiveInfo {
        path: dest.to_path_buf(),
        files: builder.files,
        bytes,
        sha256,
        newest: builder.newest,
    })
}

struct ArchiveBuilder {
    files: u64,
    bytes: u64,
    newest: Option<DateTime<Utc>>,
    archived: Vec<PathBuf>,
    /// Lowercased entry names already written. Two files that differ only by
    /// case are one save: Windows cannot produce both, and keeping both would
    /// duplicate the same content on a case-sensitive host (#294).
    seen: HashSet<String>,
}

impl ArchiveBuilder {
    fn claim(&mut self, entry_name: &str) -> bool {
        self.seen.insert(entry_name.to_lowercase())
    }

    fn add_directory<W: Write + io::Seek>(
        &mut self,
        writer: &mut ZipWriter<W>,
        name: &str,
        options: SimpleFileOptions,
    ) -> Result<(), String> {
        if !self.claim(name) {
            return Ok(());
        }
        writer
            .add_directory(format!("{name}/"), options)
            .map_err(|e| format!("Could not write the save archive: {e}"))
    }

    fn add_tree<W: Write + io::Seek>(
        &mut self,
        writer: &mut ZipWriter<W>,
        root: &Path,
        prefix: &str,
        depth: usize,
        options: SimpleFileOptions,
    ) -> Result<(), String> {
        if depth > MAX_WALK_DEPTH {
            return Err("The save folder is nested too deeply to archive.".to_string());
        }
        let mut entries = fs::read_dir(root)
            .map_err(|e| format!("Could not read the save folder: {e}"))?
            .flatten()
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            if self.files >= MAX_ARCHIVE_ENTRIES {
                return Err(format!(
                    "The save folder has more than {MAX_ARCHIVE_ENTRIES} files; automatic snapshots are capped."
                ));
            }
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.is_empty() || name == "." || name == ".." {
                continue;
            }
            let entry_name = format!("{prefix}/{}", sanitize_component(&name));
            if metadata.is_dir() {
                self.add_directory(writer, &entry_name, options)?;
                self.add_tree(writer, &path, &entry_name, depth + 1, options)?;
            } else if metadata.is_file() {
                self.add_file(writer, &path, &entry_name, options)?;
            }
            // Sockets, FIFOs and device nodes are not save data.
        }
        Ok(())
    }

    fn add_file<W: Write + io::Seek>(
        &mut self,
        writer: &mut ZipWriter<W>,
        source: &Path,
        entry_name: &str,
        options: SimpleFileOptions,
    ) -> Result<(), String> {
        let metadata =
            fs::symlink_metadata(source).map_err(|e| format!("Could not read a save file: {e}"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Ok(());
        }
        if !self.claim(entry_name) {
            return Ok(());
        }
        self.bytes = self.bytes.saturating_add(metadata.len());
        self.newest = max_time(self.newest, metadata.modified().ok());
        if self.bytes > MAX_ARCHIVE_BYTES {
            return Err(format!(
                "The save files are larger than {}; automatic snapshots are capped.",
                crate::util::human_bytes(MAX_ARCHIVE_BYTES)
            ));
        }
        writer
            .start_file(entry_name, options)
            .map_err(|e| format!("Could not write the save archive: {e}"))?;
        // #294: CRLF and LF versions of a text save are the same save. Writing
        // LF makes the archive bytes — and therefore the content hash the
        // server dedupes on — identical across Windows and Proton.
        if let Some(bytes) = normalized_text(source, metadata.len()) {
            writer
                .write_all(&bytes)
                .map_err(|e| format!("Could not write the save archive: {e}"))?;
        } else {
            let mut reader = io::BufReader::new(
                File::open(source).map_err(|e| format!("Could not read a save file: {e}"))?,
            );
            io::copy(&mut reader, writer)
                .map_err(|e| format!("Could not write the save archive: {e}"))?;
        }
        self.files += 1;
        Ok(())
    }
}

/// LF-normalised bytes for a small text save, or `None` to copy it as-is.
fn normalized_text(source: &Path, size: u64) -> Option<Vec<u8>> {
    if size > identity::MAX_TEXT_NORMALIZE_BYTES {
        return None;
    }
    let name = source.file_name()?.to_string_lossy();
    if !identity::is_text_save_name(&name) {
        return None;
    }
    let bytes = fs::read(source).ok()?;
    identity::normalize_text_bytes(&bytes)
}

/// Filesystem-safe single path component for an archive entry. Host paths are
/// never embedded: only names inside the save folder are used.
fn sanitize_component(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|character| {
            if character == '/' || character == '\\' || character.is_control() {
                '_'
            } else {
                character
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() {
        "save".to_string()
    } else {
        cleaned.chars().take(180).collect()
    }
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("Could not open the save archive: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| format!("Could not read the save archive: {e}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// True when a freshly built archive is byte-identical to the last upload.
pub fn is_unchanged(last_hash: Option<&str>, sha256: &str) -> bool {
    last_hash == Some(sha256)
}

/* --------------------------------------------------------------- toggle */

/// Per-game override wins over the global setting.
pub fn effective_sync(global: bool, game_override: Option<bool>) -> CloudSaveToggle {
    match game_override {
        Some(true) => CloudSaveToggle {
            enabled: true,
            reason: "on".to_string(),
        },
        Some(false) => CloudSaveToggle {
            enabled: false,
            reason: "game_off".to_string(),
        },
        None if global => CloudSaveToggle {
            enabled: true,
            reason: "on".to_string(),
        },
        None => CloudSaveToggle {
            enabled: false,
            reason: "global_off".to_string(),
        },
    }
}

/* ------------------------------------------------------------- conflicts */

/// Extra facts the game page needs to describe both conflict sides (#296).
#[derive(Debug, Clone, Default)]
pub struct ConflictContext {
    pub device_id: Option<String>,
    pub device_label: Option<String>,
    /// Device clock minus server clock, in seconds.
    pub clock_offset_secs: i64,
    /// The per-game policy prefers this machine: the user already answered.
    pub prefer_local: bool,
    /// Newest open conflict row on the server, when there is one.
    pub server_conflict_id: Option<i64>,
    pub server_reason: Option<String>,
    /// The offline queue recorded a conflict against this exact remote hash
    /// (#296); it stays offered even if the mtime heuristic alone would not.
    pub policy_conflict: bool,
}

/// A conflict exists when both sides changed: local save files are newer than
/// the last upload from this machine, and the newest server snapshot is not
/// the hash this machine uploaded. Any one-sided change is handled silently
/// (local-only changes are uploaded, remote-only changes are informational).
pub fn conflict_between(
    local: &LocalSaveState,
    prefs: Option<&CloudSavePrefs>,
    latest: Option<&SaveSnapshotSummary>,
    context: &ConflictContext,
) -> Option<CloudSaveConflict> {
    let latest = latest?;
    if local.files == 0 {
        return None;
    }
    if context.prefer_local {
        // "Always prefer this device": the local side wins without asking.
        return None;
    }
    let last_hash = prefs.and_then(|prefs| prefs.last_hash.as_deref());
    if last_hash == Some(latest.sha256.as_str()) {
        return None;
    }
    // The user already chose "keep local" for this exact server snapshot.
    if prefs.and_then(|prefs| prefs.ignored_remote_hash.as_deref()) == Some(latest.sha256.as_str())
    {
        return None;
    }
    let last_upload_at = prefs.and_then(|prefs| prefs.last_upload_at.clone());
    let last_upload = last_upload_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc));
    let local_newest = local.newest;
    let local_changed = context.policy_conflict
        || match (local_newest, last_upload) {
            (Some(local), Some(upload)) => {
                local.timestamp() > upload.timestamp() + CONFLICT_TOLERANCE_SECS
            }
            // Local files exist but this install never uploaded: there is no
            // trustworthy baseline, so treat it as a change.
            (Some(_), None) => true,
            _ => false,
        };
    if !local_changed {
        return None;
    }
    let clock_skewed = context.clock_offset_secs.abs() > CLOCK_SKEW_LIMIT_SECS;
    let reason = context.server_reason.clone().unwrap_or_else(|| {
        if clock_skewed {
            ConflictReason::ClockSkew.as_str().to_string()
        } else {
            ConflictReason::BothChanged.as_str().to_string()
        }
    });
    Some(CloudSaveConflict {
        local_newest_mtime: local_newest.map(format_time),
        last_upload_at,
        last_hash: last_hash.map(str::to_string),
        remote_sha256: latest.sha256.clone(),
        remote_created_at: latest.created_at.clone(),
        remote_device_id: latest.device_id.clone(),
        local_device_id: context.device_id.clone(),
        local_device_label: context.device_label.clone(),
        local_files: local.files,
        local_bytes: local.bytes,
        remote_size: latest.size,
        remote_label: latest.label.clone(),
        reason,
        clock_offset_secs: context.clock_offset_secs,
        clock_skewed,
        server_conflict_id: context.server_conflict_id,
    })
}

/// Builds the policy view of one stored snapshot.
fn snapshot_side(snapshot: &SaveSnapshotSummary) -> SaveSide {
    let recorded_at = DateTime::parse_from_rfc3339(&snapshot.created_at)
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    SaveSide {
        sha256: snapshot.sha256.clone(),
        size: snapshot.size,
        client_mtime: snapshot
            .client_mtime
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc)),
        clock_offset_secs: snapshot.clock_offset_secs,
        save_seq: snapshot.save_seq,
        device_id: snapshot.device_id.clone(),
        recorded_at,
    }
}

/// Builds the policy view of the archive this machine just built.
fn local_side(archive: &ArchiveInfo, device_id: &str, clock_offset_secs: i64) -> SaveSide {
    SaveSide {
        sha256: archive.sha256.clone(),
        size: i64::try_from(archive.bytes).unwrap_or(i64::MAX),
        client_mtime: archive.newest,
        clock_offset_secs,
        // The desktop has no source for the game's own counter yet; the field
        // is plumbed end to end so a future manifest can supply it.
        save_seq: None,
        device_id: Some(device_id.to_string()),
        recorded_at: Utc::now(),
    }
}

/* ------------------------------------------------------------------ retry */

/// Exponential backoff for a failed automatic upload. `None` once the bounded
/// attempt budget is exhausted.
pub fn retry_delay(attempts: i64) -> Option<i64> {
    if attempts <= 0 || attempts > MAX_RETRY_ATTEMPTS {
        return None;
    }
    let exponent = u32::try_from(attempts - 1).unwrap_or(0).min(8);
    Some((RETRY_BASE_SECS * 2_i64.pow(exponent)).min(3600))
}

/* ------------------------------------------------------------- HTTP client */

struct ClientContext {
    base: String,
    token: String,
}

fn client_context(settings: &LocalSettings) -> Result<ClientContext, String> {
    let base = settings.server_url.trim().trim_end_matches('/').to_string();
    if base.is_empty() {
        return Err("Connect Ludex to a server before using cloud saves.".to_string());
    }
    let token = settings
        .token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| "Sign in to the server before using cloud saves.".to_string())?;
    Ok(ClientContext {
        base,
        token: token.to_string(),
    })
}

fn auth_error(status: reqwest::StatusCode) -> String {
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        "The server rejected the saved credentials. Sign in again.".to_string()
    } else {
        format!("The server returned {status}.")
    }
}

async fn response_error(response: reqwest::Response) -> String {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(|message| message.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.trim().chars().take(200).collect());
    if detail.is_empty() {
        auth_error(status)
    } else {
        format!("The server returned {status}: {detail}")
    }
}

/// Clock offset from a response `Date` header, in the policy convention
/// (device clock minus server clock).
fn server_clock_offset(response: &reqwest::Response) -> i64 {
    response
        .headers()
        .get(reqwest::header::DATE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| DateTime::parse_from_rfc2822(value).ok())
        .map(|server| {
            Utc::now()
                .signed_duration_since(server.with_timezone(&Utc))
                .num_seconds()
        })
        .unwrap_or(0)
}

async fn fetch_snapshots(
    app: &AppHandle,
    settings: &LocalSettings,
    game_id: i64,
) -> Result<(Vec<SaveSnapshotSummary>, i64), String> {
    let context = client_context(settings)?;
    let http = app.state::<AppState>().http.clone();
    let response = http
        .get(format!("{}/api/games/{game_id}/saves", context.base))
        .bearer_auth(&context.token)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|error| format!("Could not reach the server: {error}"))?;
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    let clock_offset_secs = server_clock_offset(&response);
    let snapshots = response
        .json::<Vec<SaveSnapshotSummary>>()
        .await
        .map_err(|error| format!("The server sent an unreadable save list: {error}"))?;
    Ok((snapshots, clock_offset_secs))
}

/// Fetch the per-game policy; an unreachable server falls back to `ask` so the
/// local status still renders offline.
async fn fetch_policy(
    app: &AppHandle,
    settings: &LocalSettings,
    game_id: i64,
) -> Result<CloudSavePolicy, String> {
    let context = client_context(settings)?;
    let http = app.state::<AppState>().http.clone();
    let response = http
        .get(format!("{}/api/games/{game_id}/saves/policy", context.base))
        .bearer_auth(&context.token)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|error| format!("Could not reach the server: {error}"))?;
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    response
        .json::<CloudSavePolicy>()
        .await
        .map_err(|error| format!("The server sent an unreadable save policy: {error}"))
}

async fn put_policy(
    app: &AppHandle,
    settings: &LocalSettings,
    game_id: i64,
    policy: &CloudSavePolicy,
) -> Result<CloudSavePolicy, String> {
    let context = client_context(settings)?;
    let http = app.state::<AppState>().http.clone();
    let response = http
        .put(format!("{}/api/games/{game_id}/saves/policy", context.base))
        .bearer_auth(&context.token)
        .json(policy)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|error| format!("Could not reach the server: {error}"))?;
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    response
        .json::<CloudSavePolicy>()
        .await
        .map_err(|error| format!("The server sent an unreadable save policy: {error}"))
}

async fn fetch_conflicts(
    app: &AppHandle,
    settings: &LocalSettings,
    game_id: i64,
) -> Result<Vec<ServerSaveConflict>, String> {
    let context = client_context(settings)?;
    let http = app.state::<AppState>().http.clone();
    let response = http
        .get(format!(
            "{}/api/games/{game_id}/saves/conflicts",
            context.base
        ))
        .bearer_auth(&context.token)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|error| format!("Could not reach the server: {error}"))?;
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    response
        .json::<Vec<ServerSaveConflict>>()
        .await
        .map_err(|error| format!("The server sent an unreadable conflict list: {error}"))
}

/// Resolve the newest open server conflict for one game.
///
/// Best effort: a local sync that already succeeded must not fail because the
/// server could not record the choice. Returns the resolved conflict id.
async fn resolve_open_conflict(
    app: &AppHandle,
    settings: &LocalSettings,
    game_id: i64,
    strategy: &str,
) -> Result<Option<i64>, String> {
    let conflicts = fetch_conflicts(app, settings, game_id).await?;
    let Some(conflict) = conflicts.first() else {
        return Ok(None);
    };
    let context = client_context(settings)?;
    let http = app.state::<AppState>().http.clone();
    let response = http
        .post(format!(
            "{}/api/games/{game_id}/saves/conflicts/{}/resolve",
            context.base, conflict.id
        ))
        .bearer_auth(&context.token)
        .json(&serde_json::json!({ "strategy": strategy }))
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|error| format!("Could not reach the server: {error}"))?;
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    Ok(Some(conflict.id))
}

/// Best-effort receipt for a server snapshot restored on this device (#295).
///
/// The server pins the snapshot as a restore point and shows the receipt in the
/// game history. A failure never fails the restore: the local files are already
/// written and the restore point row already exists on this machine.
async fn record_server_restore(
    app: &AppHandle,
    settings: &LocalSettings,
    snapshot_id: i64,
    device_id: Option<&str>,
) {
    let Ok(context) = client_context(settings) else {
        return;
    };
    let http = app.state::<AppState>().http.clone();
    let response = http
        .post(format!("{}/api/saves/{snapshot_id}/restore", context.base))
        .bearer_auth(&context.token)
        .json(&serde_json::json!({
            "deviceId": device_id,
            "kind": "server",
        }))
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    match response {
        Ok(response) if response.status().is_success() => {}
        Ok(response) => {
            tracing::debug!(status = %response.status(), snapshot_id, "restore receipt rejected");
        }
        Err(error) => {
            tracing::debug!(error = %error, snapshot_id, "restore receipt not delivered");
        }
    }
}

enum UploadError {
    /// Worth retrying (network, 5xx, throttling).
    Retryable(String),
    /// Retrying cannot help (4xx, auth, invalid archive).
    Permanent(String),
}

/// Facts the server needs to arbitrate a save upload (#296).
struct UploadFacts {
    device_id: String,
    device_label: String,
    /// Newest local save mtime, in device time.
    client_mtime: Option<DateTime<Utc>>,
    /// Device clock at request time; the server computes the skew.
    client_sent_at: DateTime<Utc>,
}

async fn upload_snapshot(
    app: &AppHandle,
    settings: &LocalSettings,
    game_id: i64,
    archive: &ArchiveInfo,
    facts: &UploadFacts,
) -> Result<SaveSnapshotSummary, UploadError> {
    let context = client_context(settings).map_err(UploadError::Permanent)?;
    let file = tokio::fs::File::open(&archive.path)
        .await
        .map_err(|error| {
            UploadError::Permanent(format!("Could not open the save archive: {error}"))
        })?;
    let length = file
        .metadata()
        .await
        .map(|metadata| metadata.len())
        .map_err(|error| {
            UploadError::Permanent(format!("Could not read the save archive: {error}"))
        })?;
    let part = reqwest::multipart::Part::stream_with_length(
        reqwest::Body::wrap_stream(ReaderStream::new(file)),
        length,
    )
    .file_name("ludex-save.zip")
    .mime_str("application/zip")
    .map_err(|error| UploadError::Permanent(format!("Could not prepare the upload: {error}")))?;

    let label: String = format!("auto · {}", facts.device_label)
        .chars()
        .take(MAX_LABEL_CHARS)
        .collect();
    let mut form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("source", "auto")
        .text("deviceId", facts.device_id.clone())
        .text("deviceLabel", facts.device_label.clone())
        .text("clientSentAt", format_time(facts.client_sent_at))
        .text("label", label);
    if let Some(mtime) = facts.client_mtime {
        form = form.text("clientMtime", format_time(mtime));
    }

    let http = app.state::<AppState>().http.clone();
    let response = http
        .post(format!("{}/api/games/{game_id}/saves", context.base))
        .bearer_auth(&context.token)
        .multipart(form)
        .timeout(Duration::from_secs(300))
        .send()
        .await
        .map_err(|error| UploadError::Retryable(format!("Could not reach the server: {error}")))?;

    let status = response.status();
    if !status.is_success() {
        let message = response_error(response).await;
        let retryable = status.is_server_error()
            || status == reqwest::StatusCode::REQUEST_TIMEOUT
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
        return Err(if retryable {
            UploadError::Retryable(message)
        } else {
            UploadError::Permanent(message)
        });
    }
    response
        .json::<SaveSnapshotSummary>()
        .await
        .map_err(|error| {
            UploadError::Retryable(format!("The server sent an unreadable snapshot: {error}"))
        })
}

/* --------------------------------------------------------------- identity */

/// Host name fallback for the device label; never a hardware fingerprint.
fn default_device_label_from(host: Option<&str>) -> String {
    let cleaned: String = host
        .unwrap_or_default()
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "Desktop".to_string()
    } else {
        cleaned.chars().take(60).collect()
    }
}

fn default_device_label() -> String {
    let host = std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok());
    default_device_label_from(host.as_deref())
}

/// Return the stable device id, generating it (and a label) on first use.
/// The id is a random UUID stored in `settings.json`, not a machine id.
pub(crate) fn ensure_device_identity(state: &AppState) -> Result<(String, String), String> {
    let settings = state.settings_snapshot();
    let needs_id = settings.cloud_device_id.trim().is_empty();
    let needs_label = settings.cloud_device_label.is_none();
    if !needs_id && !needs_label {
        return Ok((
            settings.cloud_device_id,
            settings
                .cloud_device_label
                .unwrap_or_else(|| "Desktop".to_string()),
        ));
    }
    let updated = state.update_settings(|settings| {
        if settings.cloud_device_id.trim().is_empty() {
            settings.cloud_device_id = uuid::Uuid::new_v4().to_string();
        }
        if settings.cloud_device_label.is_none() {
            settings.cloud_device_label = Some(default_device_label());
        }
    })?;
    Ok((
        updated.cloud_device_id,
        updated
            .cloud_device_label
            .unwrap_or_else(|| "Desktop".to_string()),
    ))
}

/* ------------------------------------------------------------- sync engine */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOutcome {
    Uploaded,
    Unchanged,
    /// Both sides changed: nothing was uploaded, the choice is offered (#296).
    Conflict,
    /// The server already has a strict descendant; the local archive is stale.
    Stale,
    NoPaths,
    NoFiles,
    Disabled,
    Failed,
}

/// Everything one remote read yields: snapshots, per-game policy and the
/// server clock offset used to correct device mtimes.
struct RemoteState {
    snapshots: Vec<SaveSnapshotSummary>,
    policy: CloudSavePolicy,
    clock_offset_secs: i64,
}

async fn fetch_remote_state(
    app: &AppHandle,
    settings: &LocalSettings,
    game_id: i64,
) -> Result<RemoteState, String> {
    let (snapshots, clock_offset_secs) = fetch_snapshots(app, settings, game_id).await?;
    // The policy is a convenience, not a prerequisite: an old server without
    // the route, or a transient failure, keeps the asking behaviour.
    let policy = match fetch_policy(app, settings, game_id).await {
        Ok(policy) => policy,
        Err(error) => {
            tracing::debug!(error = %error, "cloud save policy unavailable; asking");
            CloudSavePolicy::default()
        }
    };
    Ok(RemoteState {
        snapshots,
        policy,
        clock_offset_secs,
    })
}

/// Remember the remote state an offline/conflicted upload saw (#296).
fn record_queued(app: &AppHandle, game_id: i64, remote_sha256: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    db::record_cloud_save_queued(&connection, game_id, Some(remote_sha256), &now_rfc3339())
}

fn temp_archive_path(app: &AppHandle, game_id: i64) -> PathBuf {
    let state = app.state::<AppState>();
    state.app_data.join("cloud-saves").join("tmp").join(format!(
        "game-{game_id}-{}.zip",
        short_id(&uuid::Uuid::new_v4().to_string())
    ))
}

/// Archive and upload one game's saves. `automatic` respects the effective
/// toggle; a manual `sync_cloud_save_now` is always allowed. `force` skips the
/// local pre-check and is only used by an explicit "keep this device" choice:
/// the server still arbitrates and keeps both sides.
async fn sync_game(
    app: &AppHandle,
    game_id: i64,
    automatic: bool,
    force: bool,
) -> Result<SyncOutcome, String> {
    let (settings, rows, prefs) = {
        let state = app.state::<AppState>();
        let settings = state.settings_snapshot();
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        (
            settings,
            db::list_save_paths(&connection, game_id)?,
            db::get_cloud_save_prefs(&connection, game_id)?,
        )
    };

    let toggle = effective_sync(
        settings.cloud_auto_sync,
        prefs.as_ref().and_then(|prefs| prefs.auto_sync),
    );
    if automatic && !toggle.enabled {
        return Ok(SyncOutcome::Disabled);
    }
    if rows.is_empty() {
        return Ok(SyncOutcome::NoPaths);
    }

    let (device_id, device_label) = {
        let state = app.state::<AppState>();
        ensure_device_identity(&state)?
    };

    // #159: archiving walks the save tree and hashes the zip. Both are blocking
    // filesystem work, so they run on the blocking pool instead of stalling the
    // async runtime (which also serves IPC).
    let temp = temp_archive_path(app, game_id);
    let archive_rows = rows.clone();
    let archive = {
        let dest = temp.clone();
        match tauri::async_runtime::spawn_blocking(move || {
            let started = std::time::Instant::now();
            let result = create_archive(&archive_rows, &dest);
            tracing::debug!(
                game_id,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "cloud save archive walk"
            );
            result
        })
        .await
        {
            Ok(Ok(archive)) => archive,
            Ok(Err(error)) => {
                record_failure(app, game_id, &error, false)?;
                let _ = fs::remove_file(&temp);
                return Ok(SyncOutcome::Failed);
            }
            Err(error) => {
                let message = format!("The save archive task failed: {error}");
                record_failure(app, game_id, &message, false)?;
                let _ = fs::remove_file(&temp);
                return Ok(SyncOutcome::Failed);
            }
        }
    };
    if archive.files == 0 {
        let _ = fs::remove_file(&archive.path);
        return Ok(SyncOutcome::NoFiles);
    }
    tracing::debug!(
        game_id,
        files = archive.files,
        bytes = archive.bytes,
        "cloud save archive ready"
    );
    if is_unchanged(
        prefs.as_ref().and_then(|prefs| prefs.last_hash.as_deref()),
        &archive.sha256,
    ) {
        clear_pending(app, game_id)?;
        let _ = fs::remove_file(&archive.path);
        return Ok(SyncOutcome::Unchanged);
    }

    // Read the server before uploading so a multi-device divergence is offered
    // instead of silently overwriting (#296). Offline, this is a no-op and the
    // upload path queues the sync as before; the next retry reads the server
    // again and re-evaluates the conflict against the then-current bytes.
    let remote = if client_context(&settings).is_ok() {
        match fetch_remote_state(app, &settings, game_id).await {
            Ok(state) => Some(state),
            Err(error) => {
                tracing::debug!(error = %error, "cloud save remote state unavailable");
                None
            }
        }
    } else {
        None
    };

    if !force {
        if let Some(remote) = remote.as_ref() {
            if let Some(latest) = SaveSnapshotSummary::latest_of(&remote.snapshots) {
                if latest.sha256 == archive.sha256 {
                    // Another device already uploaded these exact bytes.
                    clear_pending(app, game_id)?;
                    let _ = fs::remove_file(&archive.path);
                    return Ok(SyncOutcome::Unchanged);
                }
                let candidate = local_side(&archive, &device_id, remote.clock_offset_secs);
                let prefer = if remote.policy.policy == "prefer_device" {
                    remote.policy.device_id.as_deref()
                } else {
                    None
                };
                match decide(&snapshot_side(latest), &candidate, prefer) {
                    SaveDecision::Conflict(reason) => {
                        record_queued(app, game_id, &latest.sha256)?;
                        let _ = fs::remove_file(&archive.path);
                        tracing::debug!(
                            game_id,
                            reason = reason.as_str(),
                            "cloud save conflict offered"
                        );
                        return Ok(SyncOutcome::Conflict);
                    }
                    SaveDecision::FastForward(SaveSideKind::Existing) => {
                        // The server holds a strict descendant: uploading this
                        // stale archive would only add noise.
                        record_queued(app, game_id, &latest.sha256)?;
                        let _ = fs::remove_file(&archive.path);
                        return Ok(SyncOutcome::Stale);
                    }
                    _ => {}
                }
            }
        }
    }

    let facts = UploadFacts {
        device_id,
        device_label,
        client_mtime: archive.newest,
        client_sent_at: Utc::now(),
    };
    let result = upload_snapshot(app, &settings, game_id, &archive, &facts).await;
    let _ = fs::remove_file(&archive.path);

    match result {
        Ok(snapshot) => {
            let state = app.state::<AppState>();
            let connection = state
                .db
                .lock()
                .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
            // Trust the server's hash: a 200 dedupe returns an older snapshot
            // whose `createdAt` is still a valid baseline for this content.
            db::record_cloud_save_upload(&connection, game_id, &snapshot.sha256, &now_rfc3339())?;
            invalidate_scan_cache(game_id);
            if snapshot.conflict_of.is_some() {
                Ok(SyncOutcome::Conflict)
            } else {
                Ok(SyncOutcome::Uploaded)
            }
        }
        Err(UploadError::Retryable(error)) => {
            record_failure(app, game_id, &error, true)?;
            Ok(SyncOutcome::Failed)
        }
        Err(UploadError::Permanent(error)) => {
            record_failure(app, game_id, &error, false)?;
            Ok(SyncOutcome::Failed)
        }
    }
}

fn record_failure(
    app: &AppHandle,
    game_id: i64,
    message: &str,
    retryable: bool,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    let prefs = db::get_cloud_save_prefs(&connection, game_id)?;
    let attempts = prefs
        .as_ref()
        .map(|prefs| prefs.pending_attempts)
        .unwrap_or(0);
    if !retryable {
        db::record_cloud_save_failure(&connection, game_id, 0, None, message)?;
        return Ok(());
    }
    let attempts = attempts + 1;
    let next = retry_delay(attempts).map(|seconds| {
        (Utc::now() + chrono::Duration::seconds(seconds))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    });
    let message = if next.is_none() {
        format!("{message} (automatic retries exhausted)")
    } else {
        message.to_string()
    };
    db::record_cloud_save_failure(&connection, game_id, attempts, next.as_deref(), &message)
}

fn clear_pending(app: &AppHandle, game_id: i64) -> Result<(), String> {
    let state = app.state::<AppState>();
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    db::clear_cloud_save_pending(&connection, game_id)
}

/* --------------------------------------------------------------- public API */

/// Full cloud save status for the game page. Remote failures are reported in
/// `message` so the local status still renders offline.
pub async fn status(app: &AppHandle, game_id: i64) -> Result<CloudSaveStatus, String> {
    let settings = {
        let state = app.state::<AppState>();
        match ensure_device_identity(&state) {
            Ok(_) => state.settings_snapshot(),
            Err(error) => {
                tracing::debug!(error = %error, "cloud device identity unavailable");
                state.settings_snapshot()
            }
        }
    };
    let (rows, candidates, prefs, restore, detection) = {
        let state = app.state::<AppState>();
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        (
            db::list_save_paths(&connection, game_id)?,
            db::list_save_candidates(&connection, game_id)?,
            db::get_cloud_save_prefs(&connection, game_id)?,
            db::get_cloud_save_restore(&connection, game_id)?,
            db::get_save_detection(&connection, game_id)?,
        )
    };
    // #159: the recursive walk runs on the blocking pool, is capped, and is
    // reused for the next page open. It never runs on the async worker. The
    // cross-platform identity pass (#294) reuses the same walk's `exists`
    // answers and only reads trees for paths that are absent here.
    let (local, identities, pending_mappings) = {
        let rows = rows.clone();
        let wine_prefix = settings.wine_prefix.clone();
        let steam_root = settings.steam_root.clone();
        let mappings = settings.save_identity_mappings.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let started = std::time::Instant::now();
            let state = preview_scan(game_id, &rows);
            tracing::debug!(
                game_id,
                elapsed_ms = started.elapsed().as_millis() as u64,
                files = state.files,
                truncated = state.truncated,
                "cloud save preview scan"
            );
            let (identities, pending) =
                identity_report(game_id, &rows, &state, &wine_prefix, &steam_root, &mappings);
            (state, identities, pending)
        })
        .await
        .map_err(|error| format!("The save scan task failed: {error}"))?
    };
    let server_configured = client_context(&settings).is_ok();
    let (latest, policy, clock_offset_secs, server_conflict, remote_message) = if server_configured
    {
        let started = std::time::Instant::now();
        let fetched = match fetch_snapshots(app, &settings, game_id).await {
            Ok((snapshots, offset)) => {
                let policy = fetch_policy(app, &settings, game_id)
                    .await
                    .unwrap_or_else(|error| {
                        tracing::debug!(error = %error, "cloud save policy unavailable; asking");
                        CloudSavePolicy::default()
                    });
                let conflict = fetch_conflicts(app, &settings, game_id)
                    .await
                    .ok()
                    .and_then(|conflicts| conflicts.into_iter().next());
                (snapshots, policy, offset, conflict, None)
            }
            Err(error) => (Vec::new(), CloudSavePolicy::default(), 0, None, Some(error)),
        };
        tracing::debug!(
            game_id,
            elapsed_ms = started.elapsed().as_millis() as u64,
            ok = fetched.4.is_none(),
            "cloud save snapshot query"
        );
        fetched
    } else {
        (Vec::new(), CloudSavePolicy::default(), 0, None, None)
    };
    let latest = SaveSnapshotSummary::latest_of(&latest).cloned();
    let toggle = effective_sync(
        settings.cloud_auto_sync,
        prefs.as_ref().and_then(|prefs| prefs.auto_sync),
    );
    let device_id = Some(settings.cloud_device_id.clone()).filter(|id| !id.is_empty());
    let policy_conflict = latest.as_ref().is_some_and(|latest| {
        prefs
            .as_ref()
            .and_then(|prefs| prefs.queued_remote_sha256.as_deref())
            == Some(latest.sha256.as_str())
    });
    let context = ConflictContext {
        device_id: device_id.clone(),
        device_label: settings.cloud_device_label.clone(),
        clock_offset_secs,
        prefer_local: policy.prefers(device_id.as_deref()),
        server_conflict_id: server_conflict.as_ref().map(|conflict| conflict.id),
        server_reason: server_conflict
            .as_ref()
            .map(|conflict| conflict.reason.clone()),
        policy_conflict,
    };
    let conflict = conflict_between(&local, prefs.as_ref(), latest.as_ref(), &context);
    let pending = prefs
        .as_ref()
        .map(|prefs| CloudSavePending {
            pending: prefs.pending_error.is_some() || prefs.pending_next_at.is_some(),
            attempts: prefs.pending_attempts,
            next_attempt_at: prefs.pending_next_at.clone(),
            error: prefs.pending_error.clone(),
        })
        .unwrap_or(CloudSavePending {
            pending: false,
            attempts: 0,
            next_attempt_at: None,
            error: None,
        });
    let snapshot: Option<crate::saves::detect::Detection> = detection
        .as_ref()
        .and_then(|record| serde_json::from_str(&record.result_json).ok());
    let message = if rows.is_empty() {
        // #158: reuse the detection message, which carries the per-root reason
        // ("Saved Games (missing), Documents (no matching folder), …").
        snapshot
            .as_ref()
            .map(|snapshot| snapshot.message.trim())
            .filter(|message| !message.is_empty())
            .map(str::to_string)
            .or_else(|| Some(crate::saves::detect::NO_SAVE_MESSAGE.to_string()))
    } else if local.files == 0 && !local.truncated {
        // #294: a root that only exists on the other OS is not a missing save,
        // and an ambiguous mapping needs one explicit choice instead of the
        // usual "launch the game once".
        let ambiguous = pending_mappings
            .iter()
            .filter(|mapping| mapping.reason == "ambiguous")
            .count();
        if ambiguous > 0 {
            Some(format!(
                "{ambiguous} save folder candidate(s) match this save on this machine; choose one to sync it."
            ))
        } else if identities
            .iter()
            .any(|path| path.availability == "other_platform")
        {
            Some(
                "This save root only exists on the other platform; nothing is missing on this machine."
                    .to_string(),
            )
        } else {
            Some(
                "No save files exist yet. Launch the game once so it creates its saves, then sync again."
                    .to_string(),
            )
        }
    } else {
        remote_message
    };

    let cloud_candidates: Vec<CloudSaveCandidate> = candidates
        .iter()
        .map(|candidate| {
            let saved = rows.iter().find(|row| row.path == candidate.path);
            let status = if saved.is_some() {
                crate::saves::detect::CANDIDATE_STATUS_SAVING
            } else if candidate.auto_status == crate::saves::detect::CANDIDATE_STATUS_SKIPPED {
                crate::saves::detect::CANDIDATE_STATUS_SKIPPED
            } else {
                crate::saves::detect::CANDIDATE_STATUS_WAITING
            };
            CloudSaveCandidate {
                path: candidate.path.clone(),
                source: candidate.source.clone(),
                confidence: candidate.confidence.clone(),
                status: status.to_string(),
                reason: if status == crate::saves::detect::CANDIDATE_STATUS_SKIPPED {
                    candidate.auto_reason.clone()
                } else {
                    None
                },
                primary: saved.map(|row| row.primary).unwrap_or(false),
                size_bytes: candidate.size_bytes.max(0) as u64,
            }
        })
        .collect();

    Ok(CloudSaveStatus {
        game_id,
        server_configured,
        candidates: cloud_candidates,
        paths: local
            .paths
            .iter()
            .enumerate()
            .map(|(index, path)| {
                let identity = identities.get(index).cloned().unwrap_or_default();
                CloudSavePath {
                    path: path.path.clone(),
                    kind: path.kind.clone(),
                    primary: path.primary,
                    exists: path.exists,
                    files: path.files,
                    size_bytes: path.bytes,
                    last_modified: path.newest.map(format_time),
                    identity: identity.identity,
                    root_kind: identity.root_kind,
                    relative_path: identity.relative_path,
                    origin_platform: identity.origin_platform,
                    availability: identity.availability,
                    resolved_path: identity.resolved_path,
                }
            })
            .collect(),
        local_files: local.files,
        local_bytes: local.bytes,
        local_newest_mtime: local.newest.map(format_time),
        latest,
        last_hash: prefs.as_ref().and_then(|prefs| prefs.last_hash.clone()),
        last_upload_at: prefs
            .as_ref()
            .and_then(|prefs| prefs.last_upload_at.clone()),
        last_restore_at: prefs
            .as_ref()
            .and_then(|prefs| prefs.last_restore_at.clone())
            .or_else(|| restore.map(|restore| restore.restored_at)),
        toggle,
        conflict,
        policy: policy.clone(),
        clock_offset_secs,
        pending,
        pending_mappings,
        device_id: Some(settings.cloud_device_id).filter(|id| !id.is_empty()),
        last_detection_at: detection.as_ref().map(|record| record.detected_at.clone()),
        message,
    })
}

/// Record the user's choice for an ambiguous cross-platform save identity
/// (#294). The path must classify to the same identity, so the decision cannot
/// point at an unrelated folder. Asks once: the mapping is persisted in
/// `settings.json` and reused by later status calls.
pub async fn resolve_identity(
    app: &AppHandle,
    game_id: i64,
    identity: &str,
    path: &str,
) -> Result<CloudSaveStatus, String> {
    let parsed = SaveIdentity::parse(identity)
        .ok_or_else(|| "This save identity is not valid; refresh the save status.".to_string())?;
    if parsed.game_id != game_id {
        return Err("This save identity belongs to another game.".to_string());
    }
    let candidate = PathBuf::from(path.trim());
    if !candidate.is_absolute() {
        return Err("Choose an absolute save folder.".to_string());
    }
    if !candidate.exists() {
        return Err("The chosen save folder does not exist on this machine.".to_string());
    }
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let mut roots = SaveRoots::current(&settings.wine_prefix, &settings.steam_root);
    roots.add_prefix_from_path(&candidate);
    let classified = roots.classify(&candidate).ok_or_else(|| {
        format!(
            "The chosen folder is not under a {} root.",
            parsed.root().label()
        )
    })?;
    let found = SaveIdentity::new(game_id, classified.root, &classified.relative)?;
    if found.key() != parsed.key() {
        return Err(format!(
            "The chosen folder is not the {} save ({}).",
            parsed.root().label(),
            parsed.relative()
        ));
    }
    let serialized = parsed.serialized();
    state.update_settings(|settings| {
        settings.save_identity_mappings.retain(|mapping| {
            !(mapping.game_id == game_id && mapping.identity.eq_ignore_ascii_case(&serialized))
        });
        settings
            .save_identity_mappings
            .push(identity::SaveIdentityMapping {
                game_id,
                identity: serialized,
                path: candidate.to_string_lossy().to_string(),
            });
    })?;
    status(app, game_id).await
}

/// Change the global default (`game_id = None`) or a per-game override.
pub fn set_sync(
    app: &AppHandle,
    game_id: Option<i64>,
    enabled: bool,
) -> Result<SyncToggleResponse, String> {
    let state = app.state::<AppState>();
    match game_id {
        Some(game_id) => {
            let connection = state
                .db
                .lock()
                .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
            db::set_cloud_save_auto_sync(&connection, game_id, Some(enabled))?;
            if !enabled {
                db::clear_cloud_save_pending(&connection, game_id)?;
            }
        }
        None => {
            state.update_settings(|settings| settings.cloud_auto_sync = enabled)?;
            if !enabled {
                let connection = state
                    .db
                    .lock()
                    .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
                db::clear_all_cloud_save_pending(&connection)?;
            }
        }
    }
    let settings = state.settings_snapshot();
    let prefs = match game_id {
        Some(game_id) => {
            let connection = state
                .db
                .lock()
                .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
            db::get_cloud_save_prefs(&connection, game_id)?
        }
        None => None,
    };
    Ok(SyncToggleResponse {
        game_id,
        global_enabled: settings.cloud_auto_sync,
        effective: effective_sync(
            settings.cloud_auto_sync,
            prefs.as_ref().and_then(|prefs| prefs.auto_sync),
        ),
    })
}

/// Change the per-game conflict policy (#296).
///
/// `prefer_device` without an explicit id means "this machine": the id is the
/// same random UUID the sync engine already sends.
pub async fn set_policy(
    app: &AppHandle,
    game_id: i64,
    policy: &str,
    device_id: Option<&str>,
) -> Result<CloudSavePolicy, String> {
    let settings = {
        let state = app.state::<AppState>();
        state.settings_snapshot()
    };
    let policy = policy.trim();
    let body = match policy {
        "ask" => CloudSavePolicy::default(),
        "prefer_device" => {
            let device_id = match device_id.map(str::trim).filter(|id| !id.is_empty()) {
                Some(id) => id.to_string(),
                None => {
                    let state = app.state::<AppState>();
                    ensure_device_identity(&state)?.0
                }
            };
            CloudSavePolicy {
                policy: "prefer_device".to_string(),
                device_id: Some(device_id),
            }
        }
        other => {
            return Err(format!(
                "Unknown save policy '{other}'. Use 'ask' or 'prefer_device'."
            ))
        }
    };
    put_policy(app, &settings, game_id, &body).await
}

/// Manual upload for one game. Failures still return a status carrying the
/// pending retry/error instead of losing the local archive.
pub async fn sync_now(app: &AppHandle, game_id: i64) -> Result<CloudSaveStatus, String> {
    // The status carries whether an upload happened, stayed unchanged, was
    // offered as a conflict, or was queued for retry, so the UI never has to
    // parse an error string.
    let _ = sync_game(app, game_id, false, false).await?;
    status(app, game_id).await
}

/// Restore a snapshot, or keep the local files.
pub async fn restore(
    app: &AppHandle,
    game_id: i64,
    strategy: &str,
) -> Result<RestoreOutcome, String> {
    let strategy = RestoreStrategy::parse(strategy)?;
    let (settings, rows, restore_record) = {
        let state = app.state::<AppState>();
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        (
            state.settings_snapshot(),
            db::list_save_paths(&connection, game_id)?,
            db::get_cloud_save_restore(&connection, game_id)?,
        )
    };
    if rows.is_empty() {
        return Err(crate::saves::detect::NO_SAVE_MESSAGE.to_string());
    }
    let (snapshots, _) = fetch_snapshots(app, &settings, game_id).await?;
    let latest = SaveSnapshotSummary::latest_of(&snapshots)
        .cloned()
        .ok_or_else(|| "There is no cloud snapshot to restore for this game.".to_string())?;

    if strategy == RestoreStrategy::Remote {
        if let Some(record) = restore_record.as_ref() {
            if record.snapshot_id == latest.id {
                return Ok(RestoreOutcome {
                    restored: false,
                    strategy: strategy.as_str().to_string(),
                    snapshot_id: Some(latest.id),
                    backup_path: record.local_backup_path.clone(),
                    message: "This snapshot was already restored on this device.".to_string(),
                });
            }
        }
    }

    match strategy {
        RestoreStrategy::KeepLocal => {
            // An explicit choice: skip the local pre-check, but let the server
            // arbitrate and keep both sides (#296).
            let outcome = sync_game(app, game_id, false, true).await?;
            if outcome == SyncOutcome::Conflict {
                if let Err(error) =
                    resolve_open_conflict(app, &settings, game_id, "keep_local").await
                {
                    tracing::warn!(error = %error, game_id, "could not record the keep-local choice");
                }
            }
            {
                let state = app.state::<AppState>();
                let connection = state
                    .db
                    .lock()
                    .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
                // Acknowledge this server snapshot as intentionally ignored.
                // This is not a restore: the restore-once slot stays free so
                // the user can still restore the snapshot later if they want.
                db::record_cloud_save_ignored_remote(&connection, game_id, Some(&latest.sha256))?;
            }
            let message = match outcome {
                SyncOutcome::Uploaded => {
                    "Kept the local saves, uploaded them as a new automatic snapshot.".to_string()
                }
                SyncOutcome::Unchanged => {
                    "Kept the local saves; they already match the last uploaded snapshot."
                        .to_string()
                }
                SyncOutcome::Conflict => {
                    "Kept the local saves. The server copy is kept as a restore point.".to_string()
                }
                SyncOutcome::Failed => {
                    "Kept the local saves. The upload failed and is queued for retry.".to_string()
                }
                _ => "Kept the local saves.".to_string(),
            };
            Ok(RestoreOutcome {
                restored: false,
                strategy: strategy.as_str().to_string(),
                snapshot_id: Some(latest.id),
                backup_path: None,
                message,
            })
        }
        RestoreStrategy::Remote => {
            let archive = download_snapshot(app, &settings, &latest).await?;
            let hash = sha256_file(&archive)?;
            if hash != latest.sha256 {
                let _ = fs::remove_file(&archive);
                return Err(
                    "The downloaded snapshot did not match its checksum; nothing was restored."
                        .to_string(),
                );
            }

            let local = {
                let rows = rows.clone();
                tauri::async_runtime::spawn_blocking(move || preview_scan(game_id, &rows))
                    .await
                    .map_err(|error| format!("The save scan task failed: {error}"))?
            };
            let backup_path = if local.files > 0 || local.truncated {
                let backup_rows = rows.clone();
                let app = app.clone();
                match tauri::async_runtime::spawn_blocking(move || {
                    create_backup(&app, game_id, &backup_rows)
                })
                .await
                {
                    Ok(Ok(backup)) => Some(backup.path.to_string_lossy().to_string()),
                    Ok(Err(error)) => {
                        let _ = fs::remove_file(&archive);
                        return Err(format!(
                            "Could not back up the local saves; nothing was restored: {error}"
                        ));
                    }
                    Err(error) => {
                        let _ = fs::remove_file(&archive);
                        return Err(format!(
                            "The local save backup task failed; nothing was restored: {error}"
                        ));
                    }
                }
            } else {
                None
            };

            let extract_rows = rows.clone();
            let extract_source = archive.clone();
            match tauri::async_runtime::spawn_blocking(move || {
                extract_archive(&extract_source, &extract_rows)
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    let _ = fs::remove_file(&archive);
                    return Err(error);
                }
                Err(error) => {
                    let _ = fs::remove_file(&archive);
                    return Err(format!("The save restore task failed: {error}"));
                }
            }
            let _ = fs::remove_file(&archive);

            let now = now_rfc3339();
            {
                let state = app.state::<AppState>();
                let connection = state
                    .db
                    .lock()
                    .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
                db::record_cloud_save_restore(
                    &connection,
                    game_id,
                    latest.id,
                    &now,
                    backup_path.as_deref(),
                )?;
                // The restored bytes are the new baseline: do not immediately
                // upload them as a brand new snapshot.
                db::record_cloud_save_restored_hash(&connection, game_id, &latest.sha256)?;
                db::clear_cloud_save_pending(&connection, game_id)?;
            }
            invalidate_scan_cache(game_id);
            // #295: let the server pin the snapshot and record the receipt.
            let device = Some(settings.cloud_device_id.as_str()).filter(|id| !id.trim().is_empty());
            record_server_restore(app, &settings, latest.id, device).await;

            // A server-conflict copy is no longer needed once the canonical
            // snapshot was restored locally: mark it resolved, keep the loser.
            if let Err(error) = resolve_open_conflict(app, &settings, game_id, "keep_remote").await
            {
                tracing::warn!(error = %error, game_id, "could not record the keep-remote choice");
            }

            Ok(RestoreOutcome {
                restored: true,
                strategy: strategy.as_str().to_string(),
                snapshot_id: Some(latest.id),
                backup_path,
                message: "Restored the server snapshot into the local save folder.".to_string(),
            })
        }
    }
}

/// Explicit conflict resolution: delegates to `restore` and clears the pending
/// conflict by updating the baseline/restore record.
pub async fn resolve_conflict(
    app: &AppHandle,
    game_id: i64,
    strategy: &str,
) -> Result<RestoreOutcome, String> {
    restore(app, game_id, strategy).await
}

/* --------------------------------------------------------------- download */

async fn download_snapshot(
    app: &AppHandle,
    settings: &LocalSettings,
    snapshot: &SaveSnapshotSummary,
) -> Result<PathBuf, String> {
    let context = client_context(settings)?;
    let state = app.state::<AppState>();
    let dir = state.app_data.join("cloud-saves").join("downloads");
    fs::create_dir_all(&dir).map_err(|e| format!("Could not create the download folder: {e}"))?;
    let dest = dir.join(format!(
        "snapshot-{}-{}.zip",
        snapshot.id,
        short_id(&uuid::Uuid::new_v4().to_string())
    ));
    let http = state.http.clone();
    let response = http
        .get(format!(
            "{}/api/saves/{}/download",
            context.base, snapshot.id
        ))
        .bearer_auth(&context.token)
        .timeout(Duration::from_secs(300))
        .send()
        .await
        .map_err(|error| format!("Could not download the snapshot: {error}"))?;
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    let mut file = tokio::fs::File::create(&dest)
        .await
        .map_err(|error| format!("Could not write the snapshot: {error}"))?;
    let mut stream = response.bytes_stream();
    let mut total = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("Could not download the snapshot: {error}"))?;
        total = total.saturating_add(chunk.len() as u64);
        if total > MAX_ARCHIVE_BYTES {
            let _ = tokio::fs::remove_file(&dest).await;
            return Err("The snapshot is larger than the supported archive size.".to_string());
        }
        file.write_all(&chunk)
            .await
            .map_err(|error| format!("Could not write the snapshot: {error}"))?;
    }
    file.flush()
        .await
        .map_err(|error| format!("Could not flush the snapshot: {error}"))?;
    Ok(dest)
}

/* ------------------------------------------------------------- extraction */

/// Extract a verified snapshot into the mapped save roots.
///
/// Rejects absolute paths, `..`, backslashes, control characters, symlink
/// entries and anything that escapes its save root.
pub(crate) fn extract_archive(archive: &Path, rows: &[SavePathRecord]) -> Result<(), String> {
    let file = File::open(archive).map_err(|e| format!("Could not open the save archive: {e}"))?;
    let mut zip = ZipArchive::new(file)
        .map_err(|e| format!("The save archive is not a readable zip: {e}"))?;
    if zip.len() as u64 > MAX_ARCHIVE_ENTRIES {
        return Err("The save archive has too many entries.".to_string());
    }

    let mut total = 0u64;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| format!("Could not read the save archive: {e}"))?;
        let is_dir = entry.is_dir();
        let raw_name = entry.name().to_string();
        let name = if is_dir {
            raw_name.trim_end_matches('/').to_string()
        } else {
            raw_name
        };
        validate_entry_name(&name)?;
        if entry.unix_mode().map(is_symlink_mode).unwrap_or(false) {
            return Err(format!(
                "The save archive contains a symlink entry ({name}); refusing to extract."
            ));
        }
        let mut components = name.split('/');
        if components.next() != Some("saves") {
            return Err(format!(
                "The save archive contains an unexpected top-level entry ({name})."
            ));
        }
        let folder = components
            .next()
            .ok_or_else(|| format!("The save archive contains a malformed entry ({name})."))?;
        let path_index = folder
            .parse::<usize>()
            .map_err(|_| format!("The save archive contains a malformed entry ({name})."))?;
        let row = rows
            .get(path_index)
            .ok_or_else(|| format!("The save archive references an unknown save path ({name})."))?;
        let relative = components.collect::<Vec<_>>();

        if is_dir {
            if relative.is_empty() {
                continue;
            }
            let root = prepare_root(row)?;
            let target = root.join(relative.iter().collect::<PathBuf>());
            ensure_within(&root, &target)?;
            fs::create_dir_all(&target)
                .map_err(|e| format!("Could not create a save folder: {e}"))?;
            continue;
        }

        total = total.saturating_add(entry.size());
        if total > MAX_ARCHIVE_BYTES {
            return Err("The save archive expands beyond the supported size.".to_string());
        }

        let (base, target) = match row.kind.as_str() {
            "file" => {
                if relative.len() != 1 {
                    return Err(format!(
                        "The save archive contains an unexpected file for a single-file save ({name})."
                    ));
                }
                let saved = PathBuf::from(&row.path);
                let file_name = saved
                    .file_name()
                    .map(PathBuf::from)
                    .ok_or_else(|| "The save path has no file name.".to_string())?;
                let parent = saved
                    .parent()
                    .map(Path::to_path_buf)
                    .ok_or_else(|| "The save path has no parent folder.".to_string())?;
                ensure_root(&parent)?;
                let base = fs::canonicalize(&parent)
                    .map_err(|e| format!("Could not resolve the save folder: {e}"))?;
                (base.clone(), base.join(file_name))
            }
            _ => {
                let root = prepare_root(row)?;
                let target = root.join(relative.iter().collect::<PathBuf>());
                (root, target)
            }
        };
        ensure_within(&base, &target)?;
        reject_symlink_target(&target)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Could not create a save folder: {e}"))?;
            let canonical = fs::canonicalize(parent)
                .map_err(|e| format!("Could not resolve a save folder: {e}"))?;
            if !canonical.starts_with(&base) {
                return Err(format!(
                    "The save archive tries to write outside its save folder ({name})."
                ));
            }
        }
        let mut output = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&target)
            .map_err(|e| format!("Could not write a restored save file: {e}"))?;
        io::copy(&mut entry, &mut output)
            .map_err(|e| format!("Could not write a restored save file: {e}"))?;
    }
    Ok(())
}

fn validate_entry_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.chars().any(char::is_control)
        || name.len() > 512
    {
        return Err(format!(
            "The save archive contains an unsafe path ({name})."
        ));
    }
    for component in name.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(format!(
                "The save archive contains an unsafe path ({name})."
            ));
        }
        if component.contains(':') {
            return Err(format!(
                "The save archive contains an unsafe path ({name})."
            ));
        }
    }
    Ok(())
}

fn is_symlink_mode(mode: u32) -> bool {
    mode & 0o170_000 == 0o120_000
}

fn prepare_root(row: &SavePathRecord) -> Result<PathBuf, String> {
    let root = PathBuf::from(&row.path);
    ensure_root(&root)?;
    fs::canonicalize(&root).map_err(|e| format!("Could not resolve the save folder: {e}"))
}

fn ensure_root(root: &Path) -> Result<(), String> {
    if root.exists() {
        let metadata = fs::symlink_metadata(root)
            .map_err(|e| format!("Could not read the save folder: {e}"))?;
        if metadata.file_type().is_symlink() {
            return Err("A save path is a symlink; refusing to restore into it.".to_string());
        }
        if !metadata.is_dir() {
            return Err("A save path is not a folder; refusing to restore into it.".to_string());
        }
        return Ok(());
    }
    fs::create_dir_all(root).map_err(|e| format!("Could not create the save folder: {e}"))
}

fn ensure_within(base: &Path, target: &Path) -> Result<(), String> {
    if target.starts_with(base) {
        return Ok(());
    }
    // Lexical checks are enough because entry components were already
    // validated; the canonical check below catches symlinked parents.
    let normalized = target
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect::<PathBuf>();
    if normalized.starts_with(base) {
        Ok(())
    } else {
        Err("The save archive tries to write outside its save folder.".to_string())
    }
}

fn reject_symlink_target(target: &Path) -> Result<(), String> {
    if let Ok(metadata) = fs::symlink_metadata(target) {
        if metadata.file_type().is_symlink() {
            return Err("A save file is a symlink; refusing to overwrite it.".to_string());
        }
    }
    Ok(())
}

fn create_backup(
    app: &AppHandle,
    game_id: i64,
    rows: &[SavePathRecord],
) -> Result<ArchiveInfo, String> {
    let state = app.state::<AppState>();
    let dir = state.app_data.join("cloud-saves").join("backups");
    let stamp = Utc::now().format("%Y%m%d-%H%M%S");
    let dest = dir.join(format!("game-{game_id}-{stamp}.zip"));
    create_archive(rows, &dest)
}

/* ------------------------------------------------------------ scheduling */

/// Whether an automatic sync has to re-run save detection (#158):
/// - no persisted path yet (the first sessions create the save folder);
/// - detection never ran;
/// - the Ludusavi manifest changed after the last detection.
///
/// Pure so the "don't parse the manifest on every exit" rule is testable.
pub fn detection_is_stale(
    has_paths: bool,
    detected_at: Option<&str>,
    manifest_modified: Option<std::time::SystemTime>,
) -> bool {
    if !has_paths {
        return true;
    }
    let Some(detected_at) = detected_at else {
        return true;
    };
    let Ok(detected) = DateTime::parse_from_rfc3339(detected_at) else {
        return true;
    };
    let Some(modified) = manifest_modified else {
        return false;
    };
    DateTime::<Utc>::from(modified) > detected.with_timezone(&Utc)
}

fn should_detect_after_exit(app: &AppHandle, game_id: i64) -> bool {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let (has_paths, detected_at) = match state.db.lock() {
        Ok(connection) => (
            db::list_save_paths(&connection, game_id)
                .map(|rows| !rows.is_empty())
                .unwrap_or(false),
            db::get_save_detection(&connection, game_id)
                .ok()
                .flatten()
                .map(|record| record.detected_at),
        ),
        Err(_) => return false,
    };
    let mut stamps: Vec<std::time::SystemTime> = Vec::new();
    let configured = settings.ludusavi_manifest_path.trim();
    if !configured.is_empty() {
        if let Ok(modified) = fs::metadata(configured).and_then(|meta| meta.modified()) {
            stamps.push(modified);
        }
    }
    let managed = crate::saves::manifest::ManifestPaths::for_app_data(&state.app_data).manifest;
    if let Ok(modified) = fs::metadata(managed).and_then(|meta| meta.modified()) {
        stamps.push(modified);
    }
    detection_is_stale(has_paths, detected_at.as_deref(), stamps.into_iter().max())
}

fn scheduled() -> &'static Mutex<HashSet<i64>> {
    static SCHEDULED: OnceLock<Mutex<HashSet<i64>>> = OnceLock::new();
    SCHEDULED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Debounced automatic sync after a game exits (#108). Never blocks the exit
/// path and never toasts: failures are queued and retried in the background.
///
/// Detection runs first (#158): a game often creates its save folder only
/// during the first session, and the Ludusavi manifest may have been downloaded
/// after startup. Detection re-reads the current settings and manifest path, so
/// automatic sync archives the folders the game page shows — never a stale
/// mapping from install time.
pub fn schedule_after_exit(app: AppHandle, game_id: i64) {
    {
        let mut games = match scheduled().lock() {
            Ok(games) => games,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !games.insert(game_id) {
            return;
        }
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(EXIT_DEBOUNCE).await;
        {
            let mut games = match scheduled().lock() {
                Ok(games) => games,
                Err(poisoned) => poisoned.into_inner(),
            };
            games.remove(&game_id);
        }

        // The save files may have changed even when automatic sync is off; the
        // next game page must not reuse a pre-session preview (#159, #180).
        invalidate_scan_cache(game_id);

        // #180: first promote the guessed folder the session actually wrote
        // to. It runs on the blocking pool and only touches candidates that
        // have a pre-launch snapshot; a failure leaves the mapping untouched.
        let promote_app = app.clone();
        match tauri::async_runtime::spawn_blocking(move || {
            let started = std::time::Instant::now();
            let report = crate::commands::promote_save_candidates(&promote_app, game_id);
            tracing::debug!(
                game_id,
                elapsed_ms = started.elapsed().as_millis() as u64,
                promoted = report
                    .as_ref()
                    .map(|report| report.promoted.len())
                    .unwrap_or(0),
                skipped = report
                    .as_ref()
                    .map(|report| report.skipped.len())
                    .unwrap_or(0),
                "save candidate promotion after exit"
            );
            report
        })
        .await
        {
            Ok(Ok(report)) if !report.promoted.is_empty() => {
                tracing::info!(
                    game_id,
                    paths = report.promoted.len(),
                    primary = report.primary.as_deref().unwrap_or(""),
                    "promoted changed save folders"
                );
            }
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                tracing::debug!(error = %error, game_id, "save candidate promotion failed");
            }
            Err(error) => {
                tracing::debug!(error = %error, game_id, "save candidate promotion task failed");
            }
        }

        // Same detection entry point as the game page and Settings → Saves, so
        // the automatic path cannot drift to another environment.
        if should_detect_after_exit(&app, game_id) {
            let detect_app = app.clone();
            let detected = tauri::async_runtime::spawn_blocking(move || {
                let started = std::time::Instant::now();
                let result = crate::commands::detect_and_persist(&detect_app, game_id);
                tracing::debug!(
                    game_id,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    ok = result.is_ok(),
                    "save detection after game exit"
                );
                result
            })
            .await;
            match detected {
                Ok(Ok(_)) => {
                    let _ = app.emit("cloud-save://detected", game_id);
                }
                Ok(Err(error)) => {
                    // A detection failure must not block the upload: the persisted
                    // mapping may still be valid.
                    tracing::debug!(error = %error, game_id, "save detection after exit failed");
                }
                Err(error) => {
                    tracing::debug!(error = %error, game_id, "save detection task failed");
                }
            }
        } else {
            tracing::debug!(
                game_id,
                "save detection skipped: paths and manifest are current"
            );
        }

        // #295: keep this session's save state locally before the upload, so a
        // corrupt save or a wrong-way sync is recoverable without the server.
        // Detection above persisted the paths a first session created; this
        // runs even when automatic sync is off.
        crate::saves::history::snapshot_after_exit(app.clone(), game_id).await;

        match sync_game(&app, game_id, true, false).await {
            Ok(SyncOutcome::Uploaded | SyncOutcome::Unchanged) => {
                let _ = app.emit("cloud-save://synced", game_id);
            }
            // A conflict is surfaced, never resolved automatically (#296).
            Ok(SyncOutcome::Conflict | SyncOutcome::Stale) => {
                let _ = app.emit("cloud-save://conflict", game_id);
            }
            Ok(SyncOutcome::Disabled | SyncOutcome::NoPaths | SyncOutcome::NoFiles) => {}
            Ok(SyncOutcome::Failed) => {
                tracing::debug!(game_id, "automatic cloud save upload queued for retry");
                let _ = app.emit("cloud-save://pending", game_id);
            }
            Err(error) => {
                tracing::warn!(error = %error, game_id, "automatic cloud save sync failed");
            }
        }
    });
}

/// Background retry worker: checks once a minute for games whose backoff has
/// elapsed. Bounded by `MAX_RETRY_ATTEMPTS` per failure streak.
pub fn start_retry_worker(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if let Err(error) = retry_due(&app).await {
                tracing::debug!(error = %error, "cloud save retry pass failed");
            }
        }
    });
}

async fn retry_due(app: &AppHandle) -> Result<(), String> {
    let due = {
        let state = app.state::<AppState>();
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::due_cloud_save_retries(&connection, &now_rfc3339())?
    };
    for game_id in due {
        // Re-evaluation happens here, on reconnect: `sync_game` re-reads the
        // server and runs the policy against the current remote bytes, never
        // against the verdict that queued the retry (#296).
        match sync_game(app, game_id, true, false).await {
            Ok(SyncOutcome::Uploaded | SyncOutcome::Unchanged) => {
                let _ = app.emit("cloud-save://synced", game_id);
            }
            Ok(SyncOutcome::Conflict | SyncOutcome::Stale) => {
                let _ = app.emit("cloud-save://conflict", game_id);
            }
            Ok(_) => {}
            Err(error) => tracing::debug!(error = %error, game_id, "cloud save retry failed"),
        }
    }
    Ok(())
}

fn format_time(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/* ------------------------------------------------------------------ tests */

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-cloud-{label}-{}",
            short_id(&uuid::Uuid::new_v4().to_string())
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    struct Fixture {
        root: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn fixture(label: &str) -> Fixture {
        Fixture {
            root: temp_root(label),
        }
    }

    fn save_row(game_id: i64, path: &Path, kind: &str, primary: bool) -> SavePathRecord {
        SavePathRecord {
            game_id,
            path: path.to_string_lossy().to_string(),
            kind: kind.to_string(),
            source: "manual".to_string(),
            primary,
            created_at: "2026-11-10T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn empty_detection_is_a_noop() {
        let fixture = fixture("empty");
        let rows = vec![save_row(
            1,
            &fixture.root.join("missing"),
            "directory",
            true,
        )];
        let dest = fixture.root.join("archive.zip");
        let archive = create_archive(&rows, &dest).unwrap();
        // The caller treats `files == 0` as a no-op and never uploads; the zip
        // itself only contains the empty end-of-archive record.
        assert_eq!(archive.files, 0);
        assert!(fs::metadata(&archive.path).unwrap().len() < 128);
    }

    #[test]
    fn archive_preserves_layout_and_skips_symlinks() {
        let fixture = fixture("layout");
        let saves = fixture.root.join("saves");
        fs::create_dir_all(saves.join("slot-1")).unwrap();
        fs::write(saves.join("slot-1/save.dat"), b"save-one").unwrap();
        fs::write(saves.join("settings.ini"), b"cfg").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", saves.join("escape")).unwrap();

        let rows = vec![save_row(1, &saves, "directory", true)];
        let archive = create_archive(&rows, &fixture.root.join("save.zip")).unwrap();
        assert_eq!(archive.files, 2);

        let file = File::open(&archive.path).unwrap();
        let mut zip = ZipArchive::new(file).unwrap();
        let mut names = Vec::new();
        for index in 0..zip.len() {
            names.push(zip.by_index(index).unwrap().name().to_string());
        }
        assert!(names.contains(&"saves/00/slot-1/save.dat".to_string()));
        assert!(names.contains(&"saves/00/settings.ini".to_string()));
        assert!(!names.iter().any(|name| name.contains("escape")));
    }

    #[test]
    fn archives_hash_identically_across_line_endings() {
        let fixture = fixture("line-endings");
        let windows = fixture.root.join("windows/Hades");
        let linux = fixture.root.join("linux/Hades");
        fs::create_dir_all(&windows).unwrap();
        fs::create_dir_all(&linux).unwrap();
        fs::write(windows.join("settings.ini"), b"slot=1\r\nname=Hades\r\n").unwrap();
        fs::write(linux.join("settings.ini"), b"slot=1\nname=Hades\n").unwrap();

        let windows_archive = create_archive(
            &[save_row(1, &windows, "directory", true)],
            &fixture.root.join("windows.zip"),
        )
        .unwrap();
        let linux_archive = create_archive(
            &[save_row(1, &linux, "directory", true)],
            &fixture.root.join("linux.zip"),
        )
        .unwrap();

        // Same logical save, same archive bytes: the upload is skipped instead
        // of creating a second divergent snapshot (#294).
        assert_eq!(windows_archive.files, 1);
        assert_eq!(windows_archive.sha256, linux_archive.sha256);
        assert_eq!(windows_archive.bytes, linux_archive.bytes);

        let file = File::open(&linux_archive.path).unwrap();
        let mut zip = ZipArchive::new(file).unwrap();
        let mut entry = zip.by_name("saves/00/settings.ini").unwrap();
        let mut content = Vec::new();
        entry.read_to_end(&mut content).unwrap();
        assert_eq!(content, b"slot=1\nname=Hades\n");
    }

    #[cfg(unix)]
    #[test]
    fn case_only_duplicate_entries_collapse_to_one_file() {
        let fixture = fixture("case-duplicates");
        let saves = fixture.root.join("saves");
        fs::create_dir_all(&saves).unwrap();
        fs::write(saves.join("Save.dat"), b"upper").unwrap();
        fs::write(saves.join("save.dat"), b"lower").unwrap();

        let archive = create_archive(
            &[save_row(1, &saves, "directory", true)],
            &fixture.root.join("save.zip"),
        )
        .unwrap();
        assert_eq!(archive.files, 1);
    }

    #[test]
    fn one_platform_roots_are_not_reported_missing() {
        let fixture = fixture("availability");
        let mut roots = SaveRoots::default();
        roots.add(
            identity::RootKind::Documents,
            fixture.root.join("Documents"),
            Layout::Windows,
        );

        let xdg = SaveIdentity::new(1, identity::RootKind::XdgData, "Hades").unwrap();
        assert_eq!(
            base_availability(false, Some(&xdg), Some(Layout::current()), &roots),
            "other_platform"
        );

        let documents = SaveIdentity::new(1, identity::RootKind::Documents, "Hades").unwrap();
        assert_eq!(
            base_availability(false, Some(&documents), Some(Layout::current()), &roots),
            "missing"
        );
        assert_eq!(
            base_availability(true, Some(&documents), Some(Layout::current()), &roots),
            "present"
        );
    }

    #[test]
    fn file_save_paths_keep_their_file_name() {
        let fixture = fixture("file");
        let file = fixture.root.join("profile.sav");
        fs::write(&file, b"bytes").unwrap();
        let rows = vec![save_row(2, &file, "file", true)];
        let archive = create_archive(&rows, &fixture.root.join("save.zip")).unwrap();
        assert_eq!(archive.files, 1);

        let file = File::open(&archive.path).unwrap();
        let mut zip = ZipArchive::new(file).unwrap();
        assert_eq!(zip.by_index(0).unwrap().name(), "saves/00/profile.sav");
    }

    #[test]
    fn unchanged_hash_skips_the_upload() {
        assert!(is_unchanged(Some("abc"), "abc"));
        assert!(!is_unchanged(Some("abc"), "def"));
        assert!(!is_unchanged(None, "def"));
    }

    #[test]
    fn toggle_precedence_and_reason() {
        assert_eq!(
            effective_sync(true, None),
            CloudSaveToggle {
                enabled: true,
                reason: "on".into()
            }
        );
        assert_eq!(
            effective_sync(false, None),
            CloudSaveToggle {
                enabled: false,
                reason: "global_off".into()
            }
        );
        assert_eq!(
            effective_sync(true, Some(false)),
            CloudSaveToggle {
                enabled: false,
                reason: "game_off".into()
            }
        );
        assert_eq!(
            effective_sync(false, Some(true)),
            CloudSaveToggle {
                enabled: true,
                reason: "on".into()
            }
        );
    }

    fn snapshot(sha: &str, created_at: &str) -> SaveSnapshotSummary {
        SaveSnapshotSummary {
            id: 5,
            game_id: 1,
            version_id: None,
            label: "auto".into(),
            file_name: "save.zip".into(),
            size: 12,
            sha256: sha.into(),
            created_at: created_at.into(),
            source: "auto".into(),
            device_id: Some("device-b".into()),
            client_mtime: None,
            clock_offset_secs: 0,
            save_seq: None,
            conflict_of: None,
        }
    }

    fn prefs(hash: Option<&str>, uploaded_at: Option<&str>) -> CloudSavePrefs {
        CloudSavePrefs {
            game_id: 1,
            auto_sync: None,
            last_hash: hash.map(str::to_string),
            last_upload_at: uploaded_at.map(str::to_string),
            last_restore_at: None,
            pending_attempts: 0,
            pending_next_at: None,
            pending_error: None,
            ignored_remote_hash: None,
            queued_remote_sha256: None,
            queued_at: None,
        }
    }

    fn local_state(files: u64, newest: Option<&str>) -> LocalSaveState {
        LocalSaveState {
            files,
            bytes: 10,
            newest: newest.map(|value| {
                DateTime::parse_from_rfc3339(value)
                    .unwrap()
                    .with_timezone(&Utc)
            }),
            paths: Vec::new(),
            truncated: false,
        }
    }

    #[test]
    fn conflict_needs_both_local_and_remote_changes() {
        let latest = snapshot("remote-hash", "2026-11-10T12:00:00Z");

        // Local changed after the upload, remote hash differs: conflict.
        let conflict = conflict_between(
            &local_state(2, Some("2026-11-10T13:00:00Z")),
            Some(&prefs(Some("uploaded"), Some("2026-11-10T12:00:00Z"))),
            Some(&latest),
            &ConflictContext::default(),
        );
        assert!(conflict.is_some());
        assert_eq!(conflict.unwrap().remote_sha256, "remote-hash");

        // Local changed but the remote is still the uploaded hash: no conflict.
        let unchanged_remote = snapshot("uploaded", "2026-11-10T12:00:00Z");
        assert!(conflict_between(
            &local_state(2, Some("2026-11-10T13:00:00Z")),
            Some(&prefs(Some("uploaded"), Some("2026-11-10T12:00:00Z"))),
            Some(&unchanged_remote),
            &ConflictContext::default(),
        )
        .is_none());

        // Remote changed but local files predate the upload: no conflict.
        assert!(conflict_between(
            &local_state(2, Some("2026-11-10T11:00:00Z")),
            Some(&prefs(Some("uploaded"), Some("2026-11-10T12:00:00Z"))),
            Some(&latest),
            &ConflictContext::default(),
        )
        .is_none());

        // No local files: nothing to conflict with.
        assert!(conflict_between(
            &local_state(0, None),
            Some(&prefs(None, None)),
            Some(&latest),
            &ConflictContext::default(),
        )
        .is_none());

        // Local files and a remote snapshot but no baseline: conflict.
        assert!(conflict_between(
            &local_state(1, Some("2026-11-10T13:00:00Z")),
            None,
            Some(&latest),
            &ConflictContext::default(),
        )
        .is_some());

        // An acknowledged "keep local" suppresses the same conflict, while a
        // different remote snapshot still conflicts.
        let mut acknowledged = prefs(Some("uploaded"), Some("2026-11-10T12:00:00Z"));
        acknowledged.ignored_remote_hash = Some("remote-hash".to_string());
        assert!(conflict_between(
            &local_state(2, Some("2026-11-10T13:00:00Z")),
            Some(&acknowledged),
            Some(&latest),
            &ConflictContext::default(),
        )
        .is_none());
        let other = snapshot("another-hash", "2026-11-11T12:00:00Z");
        assert!(conflict_between(
            &local_state(2, Some("2026-11-10T13:00:00Z")),
            Some(&acknowledged),
            Some(&other),
            &ConflictContext::default(),
        )
        .is_some());
    }

    #[test]
    fn conflict_context_describes_both_sides() {
        let latest = snapshot("remote-hash", "2026-11-10T12:00:00Z");
        let pref = prefs(Some("uploaded"), Some("2026-11-10T12:00:00Z"));
        let context = ConflictContext {
            device_id: Some("laptop".to_string()),
            device_label: Some("Laptop".to_string()),
            clock_offset_secs: 7_200,
            prefer_local: false,
            server_conflict_id: Some(44),
            server_reason: Some("clock_skew".to_string()),
            policy_conflict: false,
        };
        let conflict = conflict_between(
            &local_state(3, Some("2026-11-10T13:00:00Z")),
            Some(&pref),
            Some(&latest),
            &context,
        )
        .expect("conflict");
        // The dialog shows two candidates side by side: local totals and the
        // server archive, each with its device and date.
        assert_eq!(conflict.local_files, 3);
        assert_eq!(conflict.local_device_label.as_deref(), Some("Laptop"));
        assert_eq!(conflict.remote_size, 12);
        assert_eq!(conflict.remote_label, "auto");
        assert_eq!(conflict.remote_device_id.as_deref(), Some("device-b"));
        assert_eq!(conflict.clock_offset_secs, 7_200);
        assert!(conflict.clock_skewed);
        assert_eq!(conflict.reason, "clock_skew");
        assert_eq!(conflict.server_conflict_id, Some(44));

        // A two-hour clock difference is reported as a skew even without a
        // server row; the dialog warns instead of letting it win.
        let skewed = conflict_between(
            &local_state(3, Some("2026-11-10T13:00:00Z")),
            Some(&pref),
            Some(&latest),
            &ConflictContext {
                clock_offset_secs: CLOCK_SKEW_LIMIT_SECS + 1,
                ..ConflictContext::default()
            },
        )
        .expect("conflict");
        assert_eq!(skewed.reason, "clock_skew");
        assert!(skewed.clock_skewed);

        // An offline queue entry for this exact remote hash keeps the choice
        // offered even when the local mtime is not newer than the upload.
        let queued = conflict_between(
            &local_state(3, Some("2026-11-10T11:00:00Z")),
            Some(&pref),
            Some(&latest),
            &ConflictContext {
                policy_conflict: true,
                ..ConflictContext::default()
            },
        );
        assert!(queued.is_some(), "queued policy conflict must be offered");

        // "Always prefer this device" already answered: no dialog.
        assert!(conflict_between(
            &local_state(3, Some("2026-11-10T13:00:00Z")),
            Some(&pref),
            Some(&latest),
            &ConflictContext {
                prefer_local: true,
                ..ConflictContext::default()
            },
        )
        .is_none());
    }

    #[test]
    fn prefer_device_policy_matches_the_local_device_only() {
        let policy = CloudSavePolicy {
            policy: "prefer_device".to_string(),
            device_id: Some("laptop".to_string()),
        };
        assert!(policy.prefers(Some("laptop")));
        assert!(!policy.prefers(Some("steam-deck")));
        assert!(!policy.prefers(None));
        assert!(!CloudSavePolicy::default().prefers(Some("laptop")));
    }

    /// #296: an offline upload is queued without a verdict. On reconnect the
    /// policy runs again against the remote bytes that exist at that moment,
    /// so the same local archive can stop or start a conflict.
    #[test]
    fn offline_queue_is_re_evaluated_against_the_current_remote() {
        let local = SaveSide {
            sha256: "local".into(),
            size: 180,
            client_mtime: Some(
                DateTime::parse_from_rfc3339("2026-11-10T15:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            clock_offset_secs: 0,
            save_seq: None,
            device_id: Some("laptop".into()),
            recorded_at: Utc::now(),
        };
        let side = |sha: &str, size: i64, mtime: &str| SaveSide {
            sha256: sha.into(),
            size,
            client_mtime: Some(
                DateTime::parse_from_rfc3339(mtime)
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            clock_offset_secs: 0,
            save_seq: None,
            device_id: Some("steam-deck".into()),
            recorded_at: Utc::now(),
        };

        // Queued at 15:00: both sides changed in incompatible directions.
        let at_queue_time = side("remote-a", 200, "2026-11-10T12:00:00Z");
        assert_eq!(
            decide(&at_queue_time, &local, None),
            SaveDecision::Conflict(ConflictReason::BothChanged)
        );

        // Reconnect later: the remote advanced to a strict descendant of the
        // local archive, so the local upload is stale and must not overwrite.
        let on_reconnect = side("remote-b", 260, "2026-11-10T16:00:00Z");
        assert_eq!(
            decide(&on_reconnect, &local, None),
            SaveDecision::FastForward(SaveSideKind::Existing)
        );

        // A different reconnect: the remote went backwards, so the local
        // archive is now the descendant and the upload fast-forwards.
        let remote_behind = side("remote-c", 150, "2026-11-10T10:00:00Z");
        assert_eq!(
            decide(&remote_behind, &local, None),
            SaveDecision::FastForward(SaveSideKind::Candidate)
        );
    }

    #[test]
    fn restore_goes_into_an_empty_folder() {
        let fixture = fixture("restore-empty");
        let saves = fixture.root.join("saves");
        fs::create_dir_all(&saves).unwrap();
        let rows = vec![save_row(2, &saves, "directory", true)];

        // Build a snapshot archive first.
        let source = fixture.root.join("source");
        fs::create_dir_all(source.join("slot")).unwrap();
        fs::write(source.join("slot/one.sav"), b"restored").unwrap();
        let archive = create_archive(
            &[save_row(2, &source, "directory", true)],
            &fixture.root.join("snap.zip"),
        )
        .unwrap();
        assert!(scan_local_state(&rows).files == 0);

        extract_archive(&archive.path, &rows).unwrap();
        assert_eq!(fs::read(saves.join("slot/one.sav")).unwrap(), b"restored");
        assert_eq!(scan_local_state(&rows).files, 1);
    }

    #[test]
    fn backup_archive_contains_the_local_state() {
        let fixture = fixture("backup");
        let saves = fixture.root.join("saves");
        fs::create_dir_all(&saves).unwrap();
        fs::write(saves.join("current.sav"), b"local").unwrap();
        let rows = vec![save_row(2, &saves, "directory", true)];

        let backup = create_archive(&rows, &fixture.root.join("backup.zip")).unwrap();
        assert_eq!(backup.files, 1);

        assert_eq!(
            read_zip_entry(&backup.path, "saves/00/current.sav").as_deref(),
            Some("local")
        );

        // Overwriting the live file must not touch the backup archive.
        fs::write(saves.join("current.sav"), b"server").unwrap();
        assert_eq!(
            read_zip_entry(&backup.path, "saves/00/current.sav").as_deref(),
            Some("local")
        );
    }

    fn read_zip_entry(path: &Path, wanted: &str) -> Option<String> {
        let file = File::open(path).unwrap();
        let mut zip = ZipArchive::new(file).unwrap();
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).unwrap();
            if entry.name() == wanted {
                let mut contents = String::new();
                entry.read_to_string(&mut contents).unwrap();
                return Some(contents);
            }
        }
        None
    }

    #[test]
    fn path_traversal_entries_are_rejected() {
        let fixture = fixture("traversal");
        let saves = fixture.root.join("saves");
        fs::create_dir_all(&saves).unwrap();
        let rows = vec![save_row(2, &saves, "directory", true)];

        for name in [
            "../escape.txt",
            "saves/00/../../escape.txt",
            "/etc/passwd",
            "saves/00/..\\escape.txt",
        ] {
            let path = fixture.root.join("evil.zip");
            write_zip_entry(&path, name, b"nope");
            let error = extract_archive(&path, &rows).unwrap_err();
            assert!(
                error.contains("unsafe")
                    || error.contains("outside")
                    || error.contains("unexpected"),
                "unexpected error for {name}: {error}"
            );
            assert!(!fixture.root.join("escape.txt").exists());
        }
    }

    #[test]
    fn symlink_modes_are_detected() {
        assert!(is_symlink_mode(0o120_777));
        assert!(!is_symlink_mode(0o100_644));
        assert!(!is_symlink_mode(0o040_755));
    }

    #[test]
    fn retry_backoff_is_bounded() {
        assert_eq!(retry_delay(0), None);
        assert_eq!(retry_delay(1), Some(30));
        assert_eq!(retry_delay(2), Some(60));
        assert_eq!(retry_delay(3), Some(120));
        assert_eq!(retry_delay(5), Some(480));
        assert_eq!(retry_delay(6), None);
    }

    #[test]
    fn device_label_falls_back_and_is_bounded() {
        assert_eq!(default_device_label_from(None), "Desktop");
        assert_eq!(default_device_label_from(Some("   ")), "Desktop");
        assert_eq!(
            default_device_label_from(Some("Living room PC")),
            "Living room PC"
        );
        let long = "x".repeat(120);
        assert_eq!(default_device_label_from(Some(&long)).chars().count(), 60);
    }

    #[test]
    fn strategy_parsing_is_strict() {
        assert_eq!(
            RestoreStrategy::parse("remote").unwrap(),
            RestoreStrategy::Remote
        );
        assert_eq!(
            RestoreStrategy::parse("keep_local").unwrap(),
            RestoreStrategy::KeepLocal
        );
        assert!(RestoreStrategy::parse("merge").is_err());
    }

    fn write_zip_entry(path: &Path, name: &str, contents: &[u8]) {
        let file = File::create(path).unwrap();
        let mut writer = ZipWriter::new(file);
        writer
            .start_file(name, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(contents).unwrap();
        writer.finish().unwrap();
    }
    fn bounded_save_tree(label: &str, files: usize) -> Fixture {
        let fixture = fixture(label);
        let saves = fixture.root.join("saves");
        fs::create_dir_all(&saves).unwrap();
        for index in 0..files {
            fs::write(saves.join(format!("slot-{index:04}.sav")), b"x").unwrap();
        }
        fixture
    }

    #[test]
    fn preview_scan_is_bounded_on_a_large_tree() {
        let fixture = bounded_save_tree("bounded", 300);
        let saves = fixture.root.join("saves");
        let rows = vec![save_row(1, &saves, "directory", true)];

        let started = std::time::Instant::now();
        let bounded = scan_local_state_with(
            &rows,
            ScanLimits {
                max_files: 40,
                max_depth: 16,
                budget: None,
            },
        );
        assert_eq!(bounded.files, 40, "{bounded:?}");
        assert!(bounded.truncated);
        // A hard cap, not a wall-clock assertion: the walk must never see the
        // whole 300-file tree.
        assert!(started.elapsed() < Duration::from_secs(10));

        let full = scan_local_state(&rows);
        assert_eq!(full.files, 300);
        assert!(!full.truncated);
    }

    #[test]
    fn scan_cache_is_bounded_by_ttl_and_mapping() {
        let fixture = bounded_save_tree("cache", 3);
        let saves = fixture.root.join("saves");
        let rows = vec![save_row(7, &saves, "directory", true)];
        let state = scan_local_state_with(&rows, ScanLimits::PREVIEW);
        assert_eq!(state.files, 3);

        let mut cache = ScanCache::default();
        let started = std::time::Instant::now();
        cache.insert(7, &rows, started, state.clone());
        assert!(cache
            .get(7, &rows, started + Duration::from_secs(1), STATUS_CACHE_TTL)
            .is_some());
        assert!(cache
            .get(
                7,
                &rows,
                started + STATUS_CACHE_TTL + Duration::from_secs(1),
                STATUS_CACHE_TTL
            )
            .is_none());

        // A changed mapping (primary flag) is a different cache entry.
        cache.insert(7, &rows, started, state.clone());
        let mut changed = rows.clone();
        changed[0].primary = false;
        assert!(cache.get(7, &changed, started, STATUS_CACHE_TTL).is_none());

        cache.insert(7, &rows, started, state);
        cache.invalidate(7);
        assert!(cache.get(7, &rows, started, STATUS_CACHE_TTL).is_none());
    }

    #[test]
    fn detection_feeds_automatic_sync_with_the_detected_paths() {
        let fixture = fixture("auto-detect");
        let install = fixture.root.join("game");
        let save_dir = install.join("saves/Hades");
        fs::create_dir_all(&save_dir).unwrap();
        fs::write(save_dir.join("slot1.sav"), b"12345").unwrap();

        let manifest = fixture.root.join("manifest.yaml");
        fs::write(
            &manifest,
            "\"Hades\":\n  files:\n    - <base>/saves/Hades\n",
        )
        .unwrap();
        let settings = LocalSettings {
            ludusavi_manifest_path: manifest.to_string_lossy().to_string(),
            ..LocalSettings::default()
        };

        let game = db::InstalledGame {
            game_id: 1,
            version_id: 1,
            title: "Hades".to_string(),
            version: None,
            install_path: install.to_string_lossy().to_string(),
            executable: None,
            size_bytes: 0,
            installed_at: "2026-11-11T00:00:00Z".to_string(),
            last_launched_at: None,
            archive_hash: None,
            install_root: None,
            source: db::SOURCE_LUDEX.to_string(),
            launch_uri: None,
        };

        let detection = crate::saves::detect::detect_for_game_with_ignored(
            &settings,
            &game,
            &fixture.root,
            &[],
            None,
            None,
            &[],
        );
        assert!(detection.detected, "{detection:?}");
        assert!(!detection.saves.is_empty(), "{detection:?}");

        let rows = crate::saves::detect::records_from_detection(
            1,
            &detection,
            Some(&detection.saves[0].path),
            "2026-11-11T00:00:00Z",
        );
        let local = scan_local_state(&rows);
        assert_eq!(local.files, 1, "{local:?}");
        assert!(local.bytes > 0);
    }
    #[test]
    fn stale_detection_is_only_rerun_when_it_can_change_something() {
        use std::time::Duration as StdDuration;
        let detected = "2026-11-11T12:00:00Z";
        let detected_at = DateTime::parse_from_rfc3339(detected)
            .unwrap()
            .with_timezone(&Utc)
            .into();

        // No path yet: always detect (first sessions create the folder).
        assert!(detection_is_stale(false, Some(detected), None));
        // A path but no detection record: detect.
        assert!(detection_is_stale(true, None, Some(detected_at)));
        // A manifest older than the last detection changes nothing.
        assert!(!detection_is_stale(
            true,
            Some(detected),
            Some(detected_at - StdDuration::from_secs(60))
        ));
        // A manifest downloaded after the last detection must be used.
        assert!(detection_is_stale(
            true,
            Some(detected),
            Some(detected_at + StdDuration::from_secs(60))
        ));
        // No managed manifest at all: the persisted mapping is authoritative.
        assert!(!detection_is_stale(true, Some(detected), None));
        // An unreadable timestamp: detect rather than silently keep a guess.
        assert!(detection_is_stale(
            true,
            Some("not a date"),
            Some(detected_at)
        ));
    }
}
