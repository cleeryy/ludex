//! Auto-managed Ludusavi manifest (#153).
//!
//! Ludusavi publishes its save-path database as a YAML `manifest.yaml` on the
//! `data` branch of `mtkennerly/ludusavi-manifest`. A fresh Ludex install has neither that file nor the
//! `ludusavi` binary, so save detection used to report "no manifest" forever.
//! This module downloads the canonical manifest into
//! `{appDataDir}/manifest/ludusavi-manifest.yaml` and keeps a small metadata
//! JSON next to it (version, ETag, last check, SHA-256), so detection works out
//! of the box without installing anything.
//!
//! Rules:
//! - the manifest is **never bundled** with Ludex (MIT data derived from
//!   PCGamingWiki, CC BY-NC-SA; see `docs/SAVES.md`);
//! - the download is data only — `saves_detect` parses the file as YAML and
//!   never executes it;
//! - nothing about the manifest is sent to the Ludex server;
//! - a failed download (offline, 404, timeout, oversize, bad checksum) keeps
//!   the previous cached file;
//! - the file is replaced atomically, bounded to 64 MiB, with a timeout;
//! - at startup Ludex checks at most once per 24 h, and only when the user has
//!   not turned automatic downloads off.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

use crate::settings::LocalSettings;
use crate::state::AppState;
use crate::util::short_id;

/// Canonical Ludusavi manifest, updated from PCGamingWiki several times a day.
///
/// The `ludusavi-manifest` repository publishes the file on its `data` branch
/// (`master` is a frozen copy) and has no GitHub release assets, so the raw
/// file is the documented source. `ludusaviManifestUrl` points elsewhere for a
/// mirror or a pinned copy.
pub const DEFAULT_MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/data/data/manifest.yaml";
/// Folder under `{appDataDir}` holding the cached copy.
pub const MANIFEST_DIR_NAME: &str = "manifest";
/// Cached manifest file name.
pub const MANIFEST_FILE_NAME: &str = "ludusavi-manifest.yaml";
/// Metadata JSON written next to the cached manifest.
pub const METADATA_FILE_NAME: &str = "ludusavi-manifest.json";

/// Automatic checks run at most once per 24 h.
pub const AUTO_CHECK_INTERVAL_HOURS: i64 = 24;
/// Hard cap on the downloaded file, matching the parser's own limit.
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
/// Whole-request timeout for the download.
pub const DOWNLOAD_TIMEOUT_SECS: u64 = 60;

/// Event emitted after an automatic update replaced the cached file.
pub const UPDATED_EVENT: &str = "save-manifest://updated";

/// Paths of the cached manifest and its metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestPaths {
    pub dir: PathBuf,
    pub manifest: PathBuf,
    pub metadata: PathBuf,
}

impl ManifestPaths {
    pub fn for_app_data(app_data: &Path) -> Self {
        let dir = app_data.join(MANIFEST_DIR_NAME);
        Self {
            manifest: dir.join(MANIFEST_FILE_NAME),
            metadata: dir.join(METADATA_FILE_NAME),
            dir,
        }
    }

    /// True when a cached manifest exists (readable or not).
    pub fn present(&self) -> bool {
        self.manifest.is_file()
    }
}

/// Metadata persisted next to the cached manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ManifestMetadata {
    /// `sha256:<12 hex>` of the cached file, or the ETag when no hash is known.
    pub version: Option<String>,
    /// HTTP ETag used for `If-None-Match` on the next check.
    pub etag: Option<String>,
    /// Last check, successful or not, RFC 3339.
    pub last_checked_at: Option<String>,
    /// Last successful download, RFC 3339.
    pub last_updated_at: Option<String>,
    /// URL the cached copy came from (mirror aware).
    pub source_url: String,
    pub size_bytes: u64,
    /// Full SHA-256 of the cached file, lowercase hex.
    pub sha256: Option<String>,
    /// Last download error, kept so the UI can explain why a check did nothing.
    pub error: Option<String>,
}

/// What the UI shows about the cached copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestStatus {
    pub present: bool,
    /// Cached file path, or null. Local only; never sent to the server.
    pub path: Option<String>,
    pub version: Option<String>,
    pub updated_at: Option<String>,
    pub checked_at: Option<String>,
    /// Effective download URL (canonical, or the user's mirror).
    pub source: String,
    pub size_bytes: u64,
    /// False when the user turned automatic downloads off.
    pub auto_update: bool,
    /// Older than the 24 h refresh interval (or never checked).
    pub stale: bool,
    pub error: Option<String>,
    pub message: String,
}

/// Result of one manual or automatic update attempt.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestUpdate {
    #[serde(flatten)]
    pub status: ManifestStatus,
    /// True when the cached file was replaced.
    pub updated: bool,
    /// True when the server answered `304 Not Modified`.
    pub not_modified: bool,
}

/// HTTP status classes the updater understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchStatus {
    Ok,
    NotModified,
    NotFound,
}

/// A download result, before checksum verification and persistence.
#[derive(Debug, Clone)]
pub struct FetchedManifest {
    pub status: FetchStatus,
    pub etag: Option<String>,
    /// SHA-256 advertised by the release (response header), lowercase hex.
    pub checksum: Option<String>,
    pub body: Vec<u8>,
}

/* -------------------------------------------------------------- pure logic */

/// Map an HTTP status to a fetch outcome. Only `2xx`, `304` and `404/410` are
/// expected; anything else is an actionable error.
pub fn classify_status(code: u16) -> Result<FetchStatus, String> {
    match code {
        200..=299 => Ok(FetchStatus::Ok),
        304 => Ok(FetchStatus::NotModified),
        404 | 410 => Ok(FetchStatus::NotFound),
        other => Err(format!(
            "The manifest server answered HTTP {other}; Ludex keeps the cached copy."
        )),
    }
}

/// Accumulates a streamed body while enforcing the size cap.
#[derive(Debug)]
pub struct CappedBody {
    bytes: Vec<u8>,
    limit: u64,
}

impl CappedBody {
    pub fn new(limit: u64) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<(), String> {
        if self.bytes.len() as u64 + chunk.len() as u64 > self.limit {
            return Err(format!(
                "The manifest is larger than {}; the download was stopped.",
                crate::util::human_bytes(self.limit)
            ));
        }
        self.bytes.extend_from_slice(chunk);
        Ok(())
    }

    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Normalise an advertised checksum: `sha256:<hex>`, `sha-256=<hex>` or a bare
/// hex digest.
fn normalise_checksum(raw: &str) -> String {
    let value = raw.trim().trim_matches(['"', '\'']).to_ascii_lowercase();
    let value = value
        .strip_prefix("sha256:")
        .or_else(|| value.strip_prefix("sha-256="))
        .or_else(|| value.strip_prefix("sha256="))
        .unwrap_or(&value);
    value.trim().to_string()
}

/// Cheap sanity check before a body replaces the cached manifest: an HTML
/// error page or a binary blob must not overwrite a working database. The full
/// YAML parse happens in `saves_detect`, where its result is actionable.
pub fn looks_like_manifest(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.contains(&0) {
        return false;
    }
    let head = &bytes[..bytes.len().min(4096)];
    let text = String::from_utf8_lossy(head);
    let trimmed = text.trim_start();
    !trimmed.starts_with('<') && trimmed.contains(':')
}

/// Verify a download against a checksum the release published.
///
/// A source without a checksum is accepted: the upstream raw file only exposes
/// an ETag. When a digest *is* advertised (a mirror, a pinned release asset) it
/// must match byte for byte.
pub fn verify_checksum(bytes: &[u8], expected: Option<&str>) -> Result<(), String> {
    let Some(expected) = expected
        .map(normalise_checksum)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    let actual = sha256_hex(bytes);
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "The manifest checksum does not match (expected {expected}, got {actual}); Ludex keeps the cached copy."
        ))
    }
}

/// A check is due when it was never done or the last one is older than
/// `AUTO_CHECK_INTERVAL_HOURS`. A malformed timestamp counts as due.
pub fn is_due(last_checked_at: Option<&str>, now: DateTime<Utc>) -> bool {
    let Some(last) = last_checked_at else {
        return true;
    };
    match DateTime::parse_from_rfc3339(last) {
        Ok(parsed) => {
            let elapsed = now.signed_duration_since(parsed.with_timezone(&Utc));
            elapsed.num_hours() >= AUTO_CHECK_INTERVAL_HOURS
        }
        Err(_) => true,
    }
}

/// Configured URL, or the canonical upstream file when the mirror is empty.
pub fn effective_url(settings: &LocalSettings) -> String {
    let configured = settings.ludusavi_manifest_url.trim();
    if configured.is_empty() {
        DEFAULT_MANIFEST_URL.to_string()
    } else {
        configured.to_string()
    }
}

/// Whether the startup hook may download without asking.
///
/// Automatic updates are on by default, which is what makes a fresh install
/// work after the very first launch. Turning the setting off disables the
/// startup hook entirely; the manual button keeps working.
pub fn should_auto_update(
    settings: &LocalSettings,
    paths: &ManifestPaths,
    now: DateTime<Utc>,
) -> bool {
    if !settings.ludusavi_manifest_auto_update {
        return false;
    }
    // The throttle is based on the last *attempt*, so a failed first download
    // is not retried on every launch either.
    let checked = read_metadata(paths).and_then(|meta| meta.last_checked_at);
    is_due(checked.as_deref(), now)
}

/// Read the metadata JSON. A corrupt or missing file is not an error: the
/// cached manifest may still be usable.
pub fn read_metadata(paths: &ManifestPaths) -> Option<ManifestMetadata> {
    let text = fs::read_to_string(&paths.metadata).ok()?;
    serde_json::from_str::<ManifestMetadata>(&text).ok()
}

/// Status shown in Settings → Saves.
pub fn status(
    paths: &ManifestPaths,
    settings: &LocalSettings,
    now: DateTime<Utc>,
) -> ManifestStatus {
    let metadata = read_metadata(paths);
    let present = paths.present();
    let version = metadata.as_ref().and_then(|meta| meta.version.clone());
    let size_bytes = metadata
        .as_ref()
        .map(|meta| meta.size_bytes)
        .filter(|size| *size > 0)
        .or_else(|| fs::metadata(&paths.manifest).ok().map(|meta| meta.len()))
        .unwrap_or(0);
    let checked_at = metadata
        .as_ref()
        .and_then(|meta| meta.last_checked_at.clone());
    let updated_at = metadata
        .as_ref()
        .and_then(|meta| meta.last_updated_at.clone())
        .or_else(|| {
            // A manifest copied in by hand has no metadata: fall back to its
            // modification time so the panel still reports something honest.
            fs::metadata(&paths.manifest)
                .ok()
                .and_then(|meta| meta.modified().ok())
                .map(|time| {
                    let stamp: DateTime<Utc> = time.into();
                    stamp.to_rfc3339()
                })
        });
    let stale = !present || is_due(checked_at.as_deref(), now);
    let error = metadata.as_ref().and_then(|meta| meta.error.clone());
    let message = if !present {
        "No save database yet. Ludex can download the official Ludusavi manifest so save detection works without installing Ludusavi.".to_string()
    } else if stale {
        "The save database is ready. It is older than 24 hours, so Ludex will refresh it on the next check.".to_string()
    } else {
        "The save database is ready.".to_string()
    };

    ManifestStatus {
        present,
        path: present.then(|| paths.manifest.to_string_lossy().to_string()),
        version,
        updated_at,
        checked_at,
        source: effective_url(settings),
        size_bytes,
        auto_update: settings.ludusavi_manifest_auto_update,
        stale,
        error,
        message,
    }
}

/* ------------------------------------------------------------- persistence */

/// Replace a file atomically: write a sibling temp file, flush it, then rename.
///
/// Windows refuses to rename over an existing file, so the fallback removes the
/// destination first. The previous manifest therefore survives every failure
/// before the rename, which is what "keep the cached copy" relies on.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Invalid manifest path: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "manifest".to_string());
    let temp = parent.join(format!(
        ".{name}.part-{}",
        short_id(&uuid::Uuid::new_v4().to_string())
    ));
    {
        let mut file = fs::File::create(&temp)
            .map_err(|error| format!("Could not write {}: {error}", temp.display()))?;
        file.write_all(bytes)
            .map_err(|error| format!("Could not write {}: {error}", temp.display()))?;
        file.sync_all()
            .map_err(|error| format!("Could not flush {}: {error}", temp.display()))?;
    }

    let replaced = match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(first) => {
            if path.exists() {
                match fs::remove_file(path) {
                    Ok(()) => fs::rename(&temp, path),
                    Err(error) => Err(std::io::Error::other(format!(
                        "could not remove the previous file: {error}"
                    ))),
                }
            } else {
                Err(first)
            }
        }
    };

    if let Err(error) = replaced {
        let _ = fs::remove_file(&temp);
        return Err(format!(
            "Could not replace {}: {error}. The previous file is unchanged.",
            path.display()
        ));
    }
    Ok(())
}

fn write_metadata(paths: &ManifestPaths, metadata: &ManifestMetadata) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(metadata)
        .map_err(|error| format!("Could not encode the manifest metadata: {error}"))?;
    write_atomic(&paths.metadata, &json)
}

/// Replace the cached manifest after verifying the download.
///
/// Every validation happens before the first write: an empty body or a checksum
/// mismatch leaves the previous file untouched.
pub fn store(
    paths: &ManifestPaths,
    fetched: &FetchedManifest,
    source_url: &str,
    now: DateTime<Utc>,
) -> Result<ManifestMetadata, String> {
    if fetched.status != FetchStatus::Ok {
        return Err("Only a 200 response can replace the manifest.".to_string());
    }
    if fetched.body.is_empty() {
        return Err("The downloaded manifest is empty; Ludex keeps the cached copy.".to_string());
    }
    if fetched.body.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(format!(
            "The manifest is larger than {}; Ludex keeps the cached copy.",
            crate::util::human_bytes(MAX_MANIFEST_BYTES)
        ));
    }
    if !looks_like_manifest(&fetched.body) {
        return Err(
            "The downloaded file is not a Ludusavi manifest; Ludex keeps the cached copy."
                .to_string(),
        );
    }
    verify_checksum(&fetched.body, fetched.checksum.as_deref())?;

    let digest = sha256_hex(&fetched.body);
    let stamp = now.to_rfc3339();
    let metadata = ManifestMetadata {
        version: Some(format!("sha256:{}", &digest[..12])),
        etag: fetched.etag.clone(),
        last_checked_at: Some(stamp.clone()),
        last_updated_at: Some(stamp),
        source_url: source_url.to_string(),
        size_bytes: fetched.body.len() as u64,
        sha256: Some(digest),
        error: None,
    };

    write_atomic(&paths.manifest, &fetched.body)?;
    if let Err(error) = write_metadata(paths, &metadata) {
        // The new file is in place; report the metadata problem but do not
        // pretend the update failed outright.
        tracing::warn!(error = %error, "the manifest was updated but its metadata was not written");
        return Ok(metadata);
    }
    tracing::info!(version = ?metadata.version, "downloaded the save-path manifest");
    Ok(metadata)
}

/// Record a failed check without touching the cached manifest.
pub fn record_failure(
    paths: &ManifestPaths,
    source_url: &str,
    now: DateTime<Utc>,
    error: &str,
) -> ManifestMetadata {
    let mut metadata = read_metadata(paths).unwrap_or_default();
    metadata.last_checked_at = Some(now.to_rfc3339());
    metadata.source_url = source_url.to_string();
    metadata.error = Some(error.to_string());
    if let Err(write_error) = write_metadata(paths, &metadata) {
        tracing::warn!(error = %write_error, "could not record the manifest check failure");
    }
    metadata
}

/// Record a `304 Not Modified` answer.
fn record_not_modified(
    paths: &ManifestPaths,
    source_url: &str,
    now: DateTime<Utc>,
) -> ManifestMetadata {
    let mut metadata = read_metadata(paths).unwrap_or_default();
    metadata.last_checked_at = Some(now.to_rfc3339());
    metadata.source_url = source_url.to_string();
    metadata.error = None;
    if let Err(error) = write_metadata(paths, &metadata) {
        tracing::warn!(error = %error, "could not record the manifest check");
    }
    metadata
}

/// Remove the cached manifest and its metadata. Missing files are not an error.
pub fn remove_cached(paths: &ManifestPaths) -> Result<(), String> {
    for path in [&paths.manifest, &paths.metadata] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("Could not delete {}: {error}", path.display()));
            }
        }
    }
    Ok(())
}

/* ---------------------------------------------------------------- updating */

/// Run one update with an injectable fetcher.
///
/// The fetcher receives the cached ETag so a conditional request can answer
/// `304`; tests substitute a mock here, so no test ever hits the network.
pub async fn update_with<F, Fut>(
    paths: &ManifestPaths,
    settings: &LocalSettings,
    now: DateTime<Utc>,
    fetcher: F,
) -> ManifestUpdate
where
    F: FnOnce(Option<String>) -> Fut,
    Fut: std::future::Future<Output = Result<FetchedManifest, String>>,
{
    let url = effective_url(settings);
    let previous = read_metadata(paths);
    let etag = previous.as_ref().and_then(|meta| meta.etag.clone());
    let cached = paths.present();
    let fetched = fetcher(etag).await;

    let checked = now.to_rfc3339();
    let mut updated = false;
    let mut not_modified = false;
    let message = match fetched {
        Err(error) => {
            record_failure(paths, &url, now, &error);
            if cached {
                format!("The save database could not be updated ({error}). Ludex keeps the cached copy.")
            } else {
                format!("The save database could not be downloaded: {error}")
            }
        }
        Ok(fetched) => match fetched.status {
            FetchStatus::NotFound => {
                let error = format!("The manifest asset was not found at {url}.");
                record_failure(paths, &url, now, &error);
                if cached {
                    format!("{error} Ludex keeps the cached copy.")
                } else {
                    error
                }
            }
            FetchStatus::NotModified => {
                record_not_modified(paths, &url, now);
                not_modified = true;
                "The save database is already up to date.".to_string()
            }
            FetchStatus::Ok => match store(paths, &fetched, &url, now) {
                Ok(metadata) => {
                    updated = true;
                    format!(
                        "Save database updated: {} ({}).",
                        metadata
                            .version
                            .clone()
                            .unwrap_or_else(|| "unknown version".to_string()),
                        crate::util::human_bytes(metadata.size_bytes)
                    )
                }
                Err(error) => {
                    record_failure(paths, &url, now, &error);
                    if cached {
                        format!("{error} Ludex keeps the cached copy.")
                    } else {
                        error
                    }
                }
            },
        },
    };

    let mut status = status(paths, settings, now);
    status.checked_at = Some(checked);
    status.message = message;
    ManifestUpdate {
        status,
        updated,
        not_modified,
    }
}

/// Download the manifest with bounded size and time.
///
/// The body is streamed so a hostile or misconfigured mirror cannot exhaust
/// memory before the cap is checked.
pub async fn fetch_remote(
    client: &reqwest::Client,
    url: &str,
    etag: Option<String>,
) -> Result<FetchedManifest, String> {
    let mut request = client
        .get(url)
        .timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS));
    if let Some(etag) = etag.as_deref().filter(|value| !value.trim().is_empty()) {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Could not reach the manifest server: {error}"))?;
    let status = classify_status(response.status().as_u16())?;
    if status != FetchStatus::Ok {
        return Ok(FetchedManifest {
            status,
            etag: None,
            checksum: None,
            body: Vec::new(),
        });
    }
    if let Some(length) = response.content_length() {
        if length > MAX_MANIFEST_BYTES {
            return Err(format!(
                "The manifest is larger than {}; the download was stopped.",
                crate::util::human_bytes(MAX_MANIFEST_BYTES)
            ));
        }
    }

    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let checksum = response
        .headers()
        .get("x-checksum-sha256")
        .and_then(|value| value.to_str().ok())
        .map(normalise_checksum)
        .filter(|value| !value.is_empty());

    let mut body = CappedBody::new(MAX_MANIFEST_BYTES);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            format!("The manifest download stopped early ({error}); Ludex keeps the cached copy.")
        })?;
        body.push(&chunk)?;
    }

    Ok(FetchedManifest {
        status,
        etag,
        checksum,
        body: body.finish(),
    })
}

/// Startup hook: check at most once per 24 h when automatic downloads are on.
pub async fn auto_update(app: AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let settings = state.settings_snapshot();
    let paths = ManifestPaths::for_app_data(&state.app_data);
    let now = Utc::now();
    if !should_auto_update(&settings, &paths, now) {
        return;
    }
    let client = state.http.clone();
    let guard = update_lock().lock().await;
    // A manual update may have run while this task waited for the lock.
    if !should_auto_update(&settings, &paths, Utc::now()) {
        drop(guard);
        return;
    }
    let url = effective_url(&settings);
    let update = update_with(&paths, &settings, Utc::now(), |etag| {
        fetch_remote(&client, &url, etag)
    })
    .await;
    drop(guard);

    if update.updated {
        tracing::info!(version = ?update.status.version, "refreshed the save-path manifest");
        let _ = app.emit(UPDATED_EVENT, &update);
    } else if let Some(error) = update.status.error.as_deref() {
        tracing::debug!(error = %error, "the automatic manifest check did nothing");
    }
}

fn update_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Manual update, serialised with the startup hook.
pub async fn update_now(app: &AppHandle) -> Result<ManifestUpdate, String> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| "Ludex is still starting; try again in a moment.".to_string())?;
    let settings = state.settings_snapshot();
    let paths = ManifestPaths::for_app_data(&state.app_data);
    let client = state.http.clone();
    let _guard = update_lock().lock().await;
    let url = effective_url(&settings);
    let update = update_with(&paths, &settings, Utc::now(), |etag| {
        fetch_remote(&client, &url, etag)
    })
    .await;
    if update.updated {
        let _ = app.emit(UPDATED_EVENT, &update);
    }
    Ok(update)
}

/// Status for the Settings panel.
pub fn current_status(app: &AppHandle) -> Result<ManifestStatus, String> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| "Ludex is still starting; try again in a moment.".to_string())?;
    let settings = state.settings_snapshot();
    let paths = ManifestPaths::for_app_data(&state.app_data);
    Ok(status(&paths, &settings, Utc::now()))
}

/// Delete the cached copy from the Settings panel.
pub fn remove_now(app: &AppHandle) -> Result<ManifestStatus, String> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| "Ludex is still starting; try again in a moment.".to_string())?;
    let settings = state.settings_snapshot();
    let paths = ManifestPaths::for_app_data(&state.app_data);
    remove_cached(&paths)?;
    Ok(status(&paths, &settings, Utc::now()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-manifest-{label}-{}",
            short_id(&uuid::Uuid::new_v4().to_string())
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    struct Fixture {
        root: PathBuf,
        paths: ManifestPaths,
        settings: LocalSettings,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn fixture(label: &str) -> Fixture {
        let root = temp_root(label);
        let paths = ManifestPaths::for_app_data(&root);
        let settings = LocalSettings {
            ludusavi_manifest_url: String::new(),
            ..LocalSettings::for_app_data(&root)
        };
        Fixture {
            root,
            paths,
            settings,
        }
    }

    fn body(text: &str) -> FetchedManifest {
        FetchedManifest {
            status: FetchStatus::Ok,
            etag: Some("\"abc\"".to_string()),
            checksum: Some(sha256_hex(text.as_bytes())),
            body: text.as_bytes().to_vec(),
        }
    }

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    const MANIFEST: &str = "\"Hades\":\n  files:\n    - <home>/.config/Hades\n";

    #[tokio::test]
    async fn a_successful_download_is_stored_atomically() {
        let fixture = fixture("store");
        let now = at("2026-11-11T10:00:00Z");
        let update = update_with(&fixture.paths, &fixture.settings, now, |etag| async move {
            assert_eq!(etag, None);
            Ok(body(MANIFEST))
        })
        .await;
        assert!(update.updated, "{update:?}");
        assert!(fixture.paths.present());
        assert_eq!(
            fs::read_to_string(&fixture.paths.manifest).unwrap(),
            MANIFEST
        );
        let metadata = read_metadata(&fixture.paths).unwrap();
        assert_eq!(metadata.size_bytes as usize, MANIFEST.len());
        assert_eq!(metadata.etag.as_deref(), Some("\"abc\""));
        assert_eq!(metadata.error, None);
        assert_eq!(
            metadata.version.as_deref(),
            Some(format!("sha256:{}", &sha256_hex(MANIFEST.as_bytes())[..12]).as_str())
        );
        // No temp file survives.
        let leftovers: Vec<_> = fs::read_dir(&fixture.paths.dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains(".part"))
            .collect();
        assert!(leftovers.is_empty());
        assert_eq!(
            update.status.checked_at.as_deref(),
            Some(now.to_rfc3339().as_str())
        );
    }

    #[tokio::test]
    async fn a_404_keeps_the_cached_copy() {
        let fixture = fixture("404");
        let now = at("2026-11-11T10:00:00Z");
        fs::create_dir_all(&fixture.paths.dir).unwrap();
        fs::write(&fixture.paths.manifest, MANIFEST).unwrap();
        let update = update_with(&fixture.paths, &fixture.settings, now, |_etag| async {
            Ok(FetchedManifest {
                status: FetchStatus::NotFound,
                etag: None,
                checksum: None,
                body: Vec::new(),
            })
        })
        .await;
        assert!(!update.updated);
        assert_eq!(
            fs::read_to_string(&fixture.paths.manifest).unwrap(),
            MANIFEST
        );
        assert!(
            update.status.message.contains("keeps the cached copy"),
            "{update:?}"
        );
        assert!(update.status.error.is_some());
    }

    #[tokio::test]
    async fn a_timeout_keeps_the_cached_copy() {
        let fixture = fixture("timeout");
        let now = at("2026-11-11T10:00:00Z");
        fs::create_dir_all(&fixture.paths.dir).unwrap();
        fs::write(&fixture.paths.manifest, MANIFEST).unwrap();
        let update = update_with(&fixture.paths, &fixture.settings, now, |_etag| async {
            Err("operation timed out after 60 s".to_string())
        })
        .await;
        assert!(!update.updated);
        assert_eq!(update.status.version, None, "no metadata to report");
        assert_eq!(
            fs::read_to_string(&fixture.paths.manifest).unwrap(),
            MANIFEST
        );
        assert!(update.status.present);
        assert!(update
            .status
            .error
            .as_deref()
            .is_some_and(|error| error.contains("timed out")));
    }

    #[tokio::test]
    async fn a_304_records_the_check_without_rewriting_the_file() {
        let fixture = fixture("304");
        let now = at("2026-11-11T10:00:00Z");
        fs::create_dir_all(&fixture.paths.dir).unwrap();
        fs::write(&fixture.paths.manifest, MANIFEST).unwrap();
        write_metadata(
            &fixture.paths,
            &ManifestMetadata {
                version: Some("sha256:aaaa".to_string()),
                etag: Some("\"abc\"".to_string()),
                last_checked_at: Some("2026-11-10T10:00:00Z".to_string()),
                last_updated_at: Some("2026-11-10T10:00:00Z".to_string()),
                source_url: DEFAULT_MANIFEST_URL.to_string(),
                size_bytes: MANIFEST.len() as u64,
                sha256: Some(sha256_hex(MANIFEST.as_bytes())),
                error: None,
            },
        )
        .unwrap();
        let update = update_with(&fixture.paths, &fixture.settings, now, |etag| async move {
            assert_eq!(etag.as_deref(), Some("\"abc\""));
            Ok(FetchedManifest {
                status: FetchStatus::NotModified,
                etag: None,
                checksum: None,
                body: Vec::new(),
            })
        })
        .await;
        assert!(update.not_modified);
        assert!(!update.updated);
        let metadata = read_metadata(&fixture.paths).unwrap();
        assert_eq!(
            metadata.last_checked_at.as_deref(),
            Some(now.to_rfc3339().as_str())
        );
        assert_eq!(metadata.version.as_deref(), Some("sha256:aaaa"));
    }

    #[tokio::test]
    async fn an_oversize_download_is_refused_before_it_replaces_the_file() {
        let fixture = fixture("oversize");
        let now = at("2026-11-11T10:00:00Z");
        fs::create_dir_all(&fixture.paths.dir).unwrap();
        fs::write(&fixture.paths.manifest, MANIFEST).unwrap();
        let update = update_with(&fixture.paths, &fixture.settings, now, |_etag| async {
            Err(format!(
                "The manifest is larger than {}; the download was stopped.",
                crate::util::human_bytes(MAX_MANIFEST_BYTES)
            ))
        })
        .await;
        assert!(!update.updated);
        assert_eq!(
            fs::read_to_string(&fixture.paths.manifest).unwrap(),
            MANIFEST
        );

        // And the cap itself, chunk by chunk:
        let mut too_big = CappedBody::new(16);
        assert!(too_big.push(&[b'x'; 16]).is_ok());
        assert!(too_big.push(b"x").is_err());
        let mut fits = CappedBody::new(16);
        fits.push(&[b'x'; 8]).unwrap();
        fits.push(&[b'x'; 8]).unwrap();
        assert_eq!(fits.finish().len(), 16);
    }

    #[tokio::test]
    async fn a_bad_checksum_keeps_the_previous_file() {
        let fixture = fixture("checksum");
        let now = at("2026-11-11T10:00:00Z");
        fs::create_dir_all(&fixture.paths.dir).unwrap();
        fs::write(&fixture.paths.manifest, MANIFEST).unwrap();
        let update = block_on_update(&fixture, now, |_etag| async {
            let mut fetched = body(MANIFEST);
            fetched.checksum = Some("0".repeat(64));
            Ok(fetched)
        })
        .await;
        assert!(!update.updated);
        assert_eq!(
            fs::read_to_string(&fixture.paths.manifest).unwrap(),
            MANIFEST
        );
        assert!(update
            .status
            .error
            .as_deref()
            .is_some_and(|error| error.contains("checksum")));

        assert!(verify_checksum(MANIFEST.as_bytes(), None).is_ok());
        assert!(verify_checksum(
            MANIFEST.as_bytes(),
            Some(&format!("sha256:{}", sha256_hex(MANIFEST.as_bytes())))
        )
        .is_ok());
        assert!(verify_checksum(MANIFEST.as_bytes(), Some("deadbeef")).is_err());
    }

    #[tokio::test]
    async fn a_malformed_download_keeps_the_previous_file() {
        let fixture = fixture("malformed");
        fs::create_dir_all(&fixture.paths.dir).unwrap();
        fs::write(&fixture.paths.manifest, MANIFEST).unwrap();
        let update = block_on_update(&fixture, at("2026-11-11T10:00:00Z"), |_etag| async {
            Ok(FetchedManifest {
                status: FetchStatus::Ok,
                etag: None,
                checksum: None,
                body: b"<html><body>captive portal</body></html>".to_vec(),
            })
        })
        .await;
        assert!(!update.updated);
        assert_eq!(
            fs::read_to_string(&fixture.paths.manifest).unwrap(),
            MANIFEST
        );
        assert!(update.status.message.contains("not a Ludusavi manifest"));
        assert!(!looks_like_manifest(b"<html></html>"));
        assert!(!looks_like_manifest(&[0u8, 1, 2]));
        assert!(looks_like_manifest(b"\"Hades\":\n  files: []\n"));
    }

    #[tokio::test]
    async fn an_empty_download_is_refused() {
        let fixture = fixture("empty");
        let update = block_on_update(&fixture, at("2026-11-11T10:00:00Z"), |_etag| async {
            Ok(FetchedManifest {
                status: FetchStatus::Ok,
                etag: None,
                checksum: None,
                body: Vec::new(),
            })
        })
        .await;
        assert!(!update.updated);
        assert!(!fixture.paths.present());
        assert!(update.status.message.contains("empty"));
    }

    async fn block_on_update<F, Fut>(
        fixture: &Fixture,
        now: DateTime<Utc>,
        fetcher: F,
    ) -> ManifestUpdate
    where
        F: FnOnce(Option<String>) -> Fut,
        Fut: std::future::Future<Output = Result<FetchedManifest, String>>,
    {
        update_with(&fixture.paths, &fixture.settings, now, fetcher).await
    }

    #[test]
    fn the_throttle_is_24_hours_and_off_when_disabled() {
        let fixture = fixture("throttle");
        let now = at("2026-11-11T10:00:00Z");
        assert!(should_auto_update(&fixture.settings, &fixture.paths, now));

        fs::create_dir_all(&fixture.paths.dir).unwrap();
        fs::write(&fixture.paths.manifest, MANIFEST).unwrap();
        write_metadata(
            &fixture.paths,
            &ManifestMetadata {
                last_checked_at: Some("2026-11-11T01:00:00Z".to_string()),
                ..ManifestMetadata::default()
            },
        )
        .unwrap();
        assert!(!should_auto_update(&fixture.settings, &fixture.paths, now));

        let later = at("2026-11-12T02:00:00Z");
        assert!(should_auto_update(&fixture.settings, &fixture.paths, later));

        // A failed first download is throttled too: the metadata exists but no
        // file does, and the check must wait for the next 24 h window.
        fs::remove_file(&fixture.paths.manifest).unwrap();
        assert!(!should_auto_update(&fixture.settings, &fixture.paths, now));
        assert!(should_auto_update(&fixture.settings, &fixture.paths, later));

        let mut disabled = fixture.settings.clone();
        disabled.ludusavi_manifest_auto_update = false;
        assert!(!should_auto_update(&disabled, &fixture.paths, later));
        assert!(is_due(None, now));
    }

    #[test]
    fn a_malformed_timestamp_is_due() {
        let now = at("2026-11-11T10:00:00Z");
        assert!(is_due(Some("not a date"), now));
        assert!(is_due(None, now));
        assert!(!is_due(Some("2026-11-11T09:00:00Z"), now));
    }

    #[test]
    fn the_url_setting_overrides_the_canonical_manifest() {
        let mut settings = LocalSettings::default();
        assert_eq!(effective_url(&settings), DEFAULT_MANIFEST_URL);
        settings.ludusavi_manifest_url = " https://mirror.example/ludusavi.yaml ".into();
        settings.normalize(Path::new("/tmp/ludex-test"));
        assert_eq!(
            effective_url(&settings),
            "https://mirror.example/ludusavi.yaml"
        );
    }

    #[tokio::test]
    async fn removing_the_cache_clears_the_status() {
        let fixture = fixture("remove");
        block_on_update(&fixture, at("2026-11-11T10:00:00Z"), |_etag| async {
            Ok(body(MANIFEST))
        })
        .await;
        assert!(fixture.paths.present());
        remove_cached(&fixture.paths).unwrap();
        assert!(!fixture.paths.present());
        let status = status(&fixture.paths, &fixture.settings, Utc::now());
        assert!(!status.present);
        assert!(status.message.contains("No save database yet"));
        assert_eq!(status.path, None);
        // Removing twice is fine.
        remove_cached(&fixture.paths).unwrap();
    }

    #[tokio::test]
    async fn status_reports_version_dates_and_source() {
        let fixture = fixture("status");
        let now = at("2026-11-11T10:00:00Z");
        block_on_update(&fixture, now, |_etag| async { Ok(body(MANIFEST)) }).await;
        let status = status(&fixture.paths, &fixture.settings, now);
        assert!(status.present);
        assert_eq!(status.source, DEFAULT_MANIFEST_URL);
        assert_eq!(
            status.updated_at.as_deref(),
            Some(now.to_rfc3339().as_str())
        );
        assert_eq!(
            status.checked_at.as_deref(),
            Some(now.to_rfc3339().as_str())
        );
        assert!(status.version.unwrap_or_default().starts_with("sha256:"));
        assert!(!status.stale);
        assert!(status.auto_update);
        assert_eq!(status.size_bytes as usize, MANIFEST.len());
        assert!(status
            .path
            .unwrap_or_default()
            .ends_with(MANIFEST_FILE_NAME));
    }

    #[test]
    fn http_statuses_are_classified() {
        assert_eq!(classify_status(200), Ok(FetchStatus::Ok));
        assert_eq!(classify_status(204), Ok(FetchStatus::Ok));
        assert_eq!(classify_status(304), Ok(FetchStatus::NotModified));
        assert_eq!(classify_status(404), Ok(FetchStatus::NotFound));
        assert_eq!(classify_status(410), Ok(FetchStatus::NotFound));
        assert!(classify_status(500).is_err());
        assert!(classify_status(429).is_err());
    }
}
