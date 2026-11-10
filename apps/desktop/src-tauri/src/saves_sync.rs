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

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

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
    pub device_id: Option<String>,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSaveConflict {
    pub local_newest_mtime: Option<String>,
    pub last_upload_at: Option<String>,
    pub last_hash: Option<String>,
    pub remote_sha256: String,
    pub remote_created_at: String,
    pub remote_device_id: Option<String>,
}

/// Everything the game page needs to render the cloud saves section.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSaveStatus {
    pub game_id: i64,
    /// A server URL and token are configured.
    pub server_configured: bool,
    pub paths: Vec<CloudSavePath>,
    pub local_files: u64,
    pub local_bytes: u64,
    pub local_newest_mtime: Option<String>,
    pub latest: Option<SaveSnapshotSummary>,
    pub last_hash: Option<String>,
    pub last_upload_at: Option<String>,
    pub last_restore_at: Option<String>,
    pub toggle: CloudSaveToggle,
    pub conflict: Option<CloudSaveConflict>,
    pub pending: CloudSavePending,
    pub device_id: Option<String>,
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

/// Scan the configured save paths without following symlinks.
pub fn scan_local_state(rows: &[SavePathRecord]) -> LocalSaveState {
    let mut state = LocalSaveState::default();
    for row in rows {
        let path = Path::new(&row.path);
        let metadata = fs::symlink_metadata(path).ok();
        let exists = metadata.is_some() && !is_symlink(&metadata);
        let mut files = 0u64;
        let mut bytes = 0u64;
        let mut newest = None;
        if exists {
            scan_tree(path, 0, &mut files, &mut bytes, &mut newest);
        }
        state.files += files;
        state.bytes += bytes;
        state.newest = match (state.newest, newest) {
            (Some(current), Some(candidate)) => Some(current.max(candidate)),
            (None, candidate) => candidate,
            (current, None) => current,
        };
        state.paths.push(LocalPathState {
            path: row.path.clone(),
            kind: row.kind.clone(),
            primary: row.primary,
            exists,
            files,
            bytes,
            newest,
        });
    }
    state
}

fn scan_tree(
    path: &Path,
    depth: usize,
    files: &mut u64,
    bytes: &mut u64,
    newest: &mut Option<DateTime<Utc>>,
) {
    if depth > MAX_WALK_DEPTH || *files >= MAX_ARCHIVE_ENTRIES {
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
        *files += 1;
        *bytes = bytes.saturating_add(metadata.len());
        *newest = max_time(*newest, metadata.modified().ok());
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
        if *files >= MAX_ARCHIVE_ENTRIES {
            return;
        }
        scan_tree(&entry.path(), depth + 1, files, bytes, newest);
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

/* -------------------------------------------------------------- archive */

/// A written archive and its content hash.
#[derive(Debug, Clone)]
pub struct ArchiveInfo {
    pub path: PathBuf,
    pub files: u64,
    pub bytes: u64,
    pub sha256: String,
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
    let options = SimpleFileOptions::default().large_file(true);
    let mut writer = ZipWriter::new(file);
    let mut builder = ArchiveBuilder {
        files: 0,
        bytes: 0,
        archived: Vec::new(),
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
    })
}

struct ArchiveBuilder {
    files: u64,
    bytes: u64,
    archived: Vec<PathBuf>,
}

impl ArchiveBuilder {
    fn add_directory<W: Write + io::Seek>(
        &mut self,
        writer: &mut ZipWriter<W>,
        name: &str,
        options: SimpleFileOptions,
    ) -> Result<(), String> {
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
        self.bytes = self.bytes.saturating_add(metadata.len());
        if self.bytes > MAX_ARCHIVE_BYTES {
            return Err(format!(
                "The save files are larger than {}; automatic snapshots are capped.",
                crate::util::human_bytes(MAX_ARCHIVE_BYTES)
            ));
        }
        writer
            .start_file(entry_name, options)
            .map_err(|e| format!("Could not write the save archive: {e}"))?;
        let mut reader = io::BufReader::new(
            File::open(source).map_err(|e| format!("Could not read a save file: {e}"))?,
        );
        io::copy(&mut reader, writer)
            .map_err(|e| format!("Could not write the save archive: {e}"))?;
        self.files += 1;
        Ok(())
    }
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

/// A conflict exists when both sides changed: local save files are newer than
/// the last upload from this machine, and the newest server snapshot is not
/// the hash this machine uploaded. Any one-sided change is handled silently
/// (local-only changes are uploaded, remote-only changes are informational).
pub fn conflict_between(
    local: &LocalSaveState,
    prefs: Option<&CloudSavePrefs>,
    latest: Option<&SaveSnapshotSummary>,
) -> Option<CloudSaveConflict> {
    let latest = latest?;
    if local.files == 0 {
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
    let local_changed = match (local_newest, last_upload) {
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
    Some(CloudSaveConflict {
        local_newest_mtime: local_newest.map(format_time),
        last_upload_at,
        last_hash: last_hash.map(str::to_string),
        remote_sha256: latest.sha256.clone(),
        remote_created_at: latest.created_at.clone(),
        remote_device_id: latest.device_id.clone(),
    })
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

async fn fetch_snapshots(
    app: &AppHandle,
    settings: &LocalSettings,
    game_id: i64,
) -> Result<Vec<SaveSnapshotSummary>, String> {
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
    response
        .json::<Vec<SaveSnapshotSummary>>()
        .await
        .map_err(|error| format!("The server sent an unreadable save list: {error}"))
}

enum UploadError {
    /// Worth retrying (network, 5xx, throttling).
    Retryable(String),
    /// Retrying cannot help (4xx, auth, invalid archive).
    Permanent(String),
}

async fn upload_snapshot(
    app: &AppHandle,
    settings: &LocalSettings,
    game_id: i64,
    archive: &ArchiveInfo,
    device_id: &str,
    device_label: &str,
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

    let label: String = format!("auto · {device_label}")
        .chars()
        .take(MAX_LABEL_CHARS)
        .collect();
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("source", "auto")
        .text("deviceId", device_id.to_string())
        .text("label", label);

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
fn ensure_device_identity(state: &AppState) -> Result<(String, String), String> {
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
    NoPaths,
    NoFiles,
    Disabled,
    Failed,
}

fn temp_archive_path(app: &AppHandle, game_id: i64) -> PathBuf {
    let state = app.state::<AppState>();
    state.app_data.join("cloud-saves").join("tmp").join(format!(
        "game-{game_id}-{}.zip",
        short_id(&uuid::Uuid::new_v4().to_string())
    ))
}

/// Archive and upload one game's saves. `automatic` respects the effective
/// toggle; a manual `sync_cloud_save_now` is always allowed.
async fn sync_game(app: &AppHandle, game_id: i64, automatic: bool) -> Result<SyncOutcome, String> {
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
    let local = scan_local_state(&rows);
    if local.files == 0 {
        return Ok(SyncOutcome::NoFiles);
    }

    let (device_id, device_label) = {
        let state = app.state::<AppState>();
        ensure_device_identity(&state)?
    };

    let temp = temp_archive_path(app, game_id);
    let archive = match create_archive(&rows, &temp) {
        Ok(archive) => archive,
        Err(error) => {
            record_failure(app, game_id, &error, false)?;
            let _ = fs::remove_file(&temp);
            return Ok(SyncOutcome::Failed);
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

    let result =
        upload_snapshot(app, &settings, game_id, &archive, &device_id, &device_label).await;
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
            Ok(SyncOutcome::Uploaded)
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
    let (rows, prefs, restore) = {
        let state = app.state::<AppState>();
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        (
            db::list_save_paths(&connection, game_id)?,
            db::get_cloud_save_prefs(&connection, game_id)?,
            db::get_cloud_save_restore(&connection, game_id)?,
        )
    };
    let local = scan_local_state(&rows);
    let server_configured = client_context(&settings).is_ok();
    let (latest, remote_message) = if server_configured {
        match fetch_snapshots(app, &settings, game_id).await {
            Ok(snapshots) => (snapshots.into_iter().next(), None),
            Err(error) => (None, Some(error)),
        }
    } else {
        (None, None)
    };
    let toggle = effective_sync(
        settings.cloud_auto_sync,
        prefs.as_ref().and_then(|prefs| prefs.auto_sync),
    );
    let conflict = conflict_between(&local, prefs.as_ref(), latest.as_ref());
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
    let message = if rows.is_empty() {
        Some(crate::saves_detect::NO_SAVE_MESSAGE.to_string())
    } else if local.files == 0 {
        Some(
            "No save files exist yet. Launch the game once so it creates its saves, then sync again."
                .to_string(),
        )
    } else {
        remote_message
    };

    Ok(CloudSaveStatus {
        game_id,
        server_configured,
        paths: local
            .paths
            .iter()
            .map(|path| CloudSavePath {
                path: path.path.clone(),
                kind: path.kind.clone(),
                primary: path.primary,
                exists: path.exists,
                files: path.files,
                size_bytes: path.bytes,
                last_modified: path.newest.map(format_time),
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
        pending,
        device_id: Some(settings.cloud_device_id).filter(|id| !id.is_empty()),
        message,
    })
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

/// Manual upload for one game. Failures still return a status carrying the
/// pending retry/error instead of losing the local archive.
pub async fn sync_now(app: &AppHandle, game_id: i64) -> Result<CloudSaveStatus, String> {
    // The status carries whether an upload happened, stayed unchanged, or was
    // queued for retry, so the UI never has to parse an error string.
    let _ = sync_game(app, game_id, false).await?;
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
        return Err(crate::saves_detect::NO_SAVE_MESSAGE.to_string());
    }
    let snapshots = fetch_snapshots(app, &settings, game_id).await?;
    let latest = snapshots
        .first()
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
            let outcome = sync_game(app, game_id, false).await?;
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
            let archive = download_snapshot(app, &settings, latest).await?;
            let hash = sha256_file(&archive)?;
            if hash != latest.sha256 {
                let _ = fs::remove_file(&archive);
                return Err(
                    "The downloaded snapshot did not match its checksum; nothing was restored."
                        .to_string(),
                );
            }

            let local = scan_local_state(&rows);
            let backup_path = if local.files > 0 {
                match create_backup(app, game_id, &rows) {
                    Ok(backup) => Some(backup.path.to_string_lossy().to_string()),
                    Err(error) => {
                        let _ = fs::remove_file(&archive);
                        return Err(format!(
                            "Could not back up the local saves; nothing was restored: {error}"
                        ));
                    }
                }
            } else {
                None
            };

            if let Err(error) = extract_archive(&archive, &rows) {
                let _ = fs::remove_file(&archive);
                return Err(error);
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
fn extract_archive(archive: &Path, rows: &[SavePathRecord]) -> Result<(), String> {
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

fn scheduled() -> &'static Mutex<HashSet<i64>> {
    static SCHEDULED: OnceLock<Mutex<HashSet<i64>>> = OnceLock::new();
    SCHEDULED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Debounced automatic sync after a game exits (#108). Never blocks the exit
/// path and never toasts: failures are queued and retried in the background.
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
        match sync_game(&app, game_id, true).await {
            Ok(SyncOutcome::Uploaded | SyncOutcome::Unchanged) => {
                let _ = app.emit("cloud-save://synced", game_id);
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
        match sync_game(app, game_id, true).await {
            Ok(SyncOutcome::Uploaded | SyncOutcome::Unchanged) => {
                let _ = app.emit("cloud-save://synced", game_id);
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
        );
        assert!(conflict.is_some());
        assert_eq!(conflict.unwrap().remote_sha256, "remote-hash");

        // Local changed but the remote is still the uploaded hash: no conflict.
        let unchanged_remote = snapshot("uploaded", "2026-11-10T12:00:00Z");
        assert!(conflict_between(
            &local_state(2, Some("2026-11-10T13:00:00Z")),
            Some(&prefs(Some("uploaded"), Some("2026-11-10T12:00:00Z"))),
            Some(&unchanged_remote),
        )
        .is_none());

        // Remote changed but local files predate the upload: no conflict.
        assert!(conflict_between(
            &local_state(2, Some("2026-11-10T11:00:00Z")),
            Some(&prefs(Some("uploaded"), Some("2026-11-10T12:00:00Z"))),
            Some(&latest),
        )
        .is_none());

        // No local files: nothing to conflict with.
        assert!(conflict_between(
            &local_state(0, None),
            Some(&prefs(None, None)),
            Some(&latest),
        )
        .is_none());

        // Local files and a remote snapshot but no baseline: conflict.
        assert!(conflict_between(
            &local_state(1, Some("2026-11-10T13:00:00Z")),
            None,
            Some(&latest),
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
        )
        .is_none());
        let other = snapshot("another-hash", "2026-11-11T12:00:00Z");
        assert!(conflict_between(
            &local_state(2, Some("2026-11-10T13:00:00Z")),
            Some(&acknowledged),
            Some(&other),
        )
        .is_some());
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
}
