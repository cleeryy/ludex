//! Library scanner: walks every enabled library root, groups files into logical
//! games and keeps the database in sync without ever deleting user progress.
//!
//! Roots are ordered (`library_roots.position`). Each version records its
//! `root_id`; a root that is missing or unmounted is marked unavailable and its
//! versions are flagged `root_unavailable` instead of `missing`, so an
//! unplugged disk never looks like a deleted game. Identical archives mirrored
//! in several roots (same file name, same size, same SHA-256) are indexed once.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use ludex_core::archive::is_archive_path;
use ludex_core::naming::{grouping_key, parse_filename, ParsedName};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, watch};
use walkdir::{DirEntry, WalkDir};

use crate::audit::actions;
use crate::db;
use crate::events::{Actor, Event};
use crate::roots::{self, LibraryRoot};
use crate::state::AppState;

/// Directory names that never contain library files.
const BLOCKED_DIRECTORIES: &[&str] = &[
    "redist",
    "_commonredist",
    "support",
    "tools",
    "redistributables",
    "directx",
    "dotnet",
];

/// Files below this size are ignored unless they are portable executables.
const MIN_ARCHIVE_BYTES: u64 = 1024 * 1024;

/// Maximum directory depth walked from the library root.
const MAX_DEPTH: usize = 3;

/// Buffer used when hashing archives.
const HASH_BUFFER_BYTES: usize = 64 * 1024;

/// One scan request. `root_id = None` scans every enabled root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanRequest {
    /// Restrict the scan to a single root.
    pub root_id: Option<i64>,
}

impl ScanRequest {
    /// Full scan over every enabled root.
    #[must_use]
    pub const fn all() -> Self {
        Self { root_id: None }
    }

    /// Scan one root only.
    #[must_use]
    pub const fn root(root_id: i64) -> Self {
        Self {
            root_id: Some(root_id),
        }
    }

    /// Merges two queued requests: a full scan wins over a targeted one, and
    /// two different roots are merged into a full scan so neither is skipped.
    fn merge(self, other: Self) -> Self {
        match (self.root_id, other.root_id) {
            (None, _) | (_, None) => Self::all(),
            (Some(left), Some(right)) if left == right => self,
            (Some(_), Some(_)) => Self::all(),
        }
    }
}

struct Candidate {
    root_id: i64,
    root_path: PathBuf,
    relative: String,
    file_name: String,
    size: i64,
    /// Archive mtime in Unix nanoseconds, used to detect same-size replacements
    /// within the same second.
    modified: i64,
    parsed: ParsedName,
    content_hash: Option<String>,
}

struct Discovered {
    available: bool,
    error: Option<String>,
    candidates: Vec<Candidate>,
}

/// Triggers a full scan when none is running.
pub async fn trigger(state: &AppState) -> bool {
    trigger_request(state, ScanRequest::all()).await
}

/// Triggers a scan of one root when none is running.
pub async fn trigger_root(state: &AppState, root_id: i64) -> bool {
    trigger_request(state, ScanRequest::root(root_id)).await
}

async fn trigger_request(state: &AppState, request: ScanRequest) -> bool {
    if state.scan.read().await.running {
        return false;
    }
    state.scan_tx.try_send(request).is_ok()
}

/// Background worker: consumes scan requests and the periodic timer.
pub async fn worker(
    state: AppState,
    mut rx: mpsc::Receiver<ScanRequest>,
    mut cancel: watch::Receiver<bool>,
) {
    let seconds = state.config.scan_interval.as_secs();
    let mut ticker = if seconds == 0 {
        None
    } else {
        let mut interval = tokio::time::interval(Duration::from_secs(seconds));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;
        Some(interval)
    };

    loop {
        let tick = async {
            match ticker.as_mut() {
                Some(ticker) => {
                    ticker.tick().await;
                }
                None => std::future::pending::<()>().await,
            }
        };

        tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    break;
                }
            }
            message = rx.recv() => {
                let Some(mut request) = message else {
                    break;
                };
                // Coalesce burst requests into a single pass.
                while let Ok(next) = rx.try_recv() {
                    request = request.merge(next);
                }
                run_scan(&state, request).await;
            }
            () = tick => {
                run_scan(&state, ScanRequest::all()).await;
            }
        }
    }
    tracing::debug!("library scan worker stopped");
}

/// Runs one complete scan. Errors are recorded in the status, never returned.
pub async fn run_scan(state: &AppState, request: ScanRequest) {
    let scan_started = std::time::Instant::now();
    {
        let mut status = state.scan.write().await;
        status.running = true;
        status.started_at = Some(db::now());
        status.finished_at = None;
        status.scanned = 0;
        status.added = 0;
        status.updated = 0;
        status.removed = 0;
        status.metadata_pending = 0;
        status.errors.clear();
    }

    let roots = match selected_roots(state, request.root_id).await {
        Ok(roots) => roots,
        Err(error) => {
            finish_with_error(state, format!("scan failed: {error}")).await;
            return;
        }
    };

    let mut errors: Vec<String> = Vec::new();
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut scanned_roots: Vec<i64> = Vec::new();
    let mut unavailable_roots: Vec<i64> = Vec::new();

    for root in &roots {
        let path = PathBuf::from(&root.path);
        let root_id = root.id;
        let discovered = tokio::task::spawn_blocking(move || discover_root(&path, root_id)).await;
        match discovered {
            Ok(Ok(discovered)) => {
                if discovered.available {
                    let count = discovered.candidates.len();
                    candidates.extend(discovered.candidates);
                    scanned_roots.push(root.id);
                    tracing::debug!(root = %root.path, files = count, "root scanned");
                } else {
                    unavailable_roots.push(root.id);
                    if errors.len() < 20 {
                        errors.push(format!(
                            "{}: {}",
                            root.path,
                            discovered
                                .error
                                .unwrap_or_else(|| "root is unavailable".to_string())
                        ));
                    }
                }
            }
            Ok(Err(error)) => {
                unavailable_roots.push(root.id);
                if errors.len() < 20 {
                    errors.push(format!("{}: {error}", root.path));
                }
            }
            Err(error) => {
                unavailable_roots.push(root.id);
                if errors.len() < 20 {
                    errors.push(format!("{}: scan task failed: {error}", root.path));
                }
            }
        }
    }

    for root in &roots {
        let available = scanned_roots.contains(&root.id);
        let error = if available {
            None
        } else {
            Some("root is missing or not a directory")
        };
        if let Err(error) = roots::record_scan(&state.pool, root.id, available, error).await {
            tracing::warn!(root_id = root.id, %error, "could not record root scan state");
        }
    }
    mark_root_availability(state, &scanned_roots, &unavailable_roots).await;

    let scanned = i64::try_from(candidates.len()).unwrap_or(i64::MAX);
    state.scan.write().await.scanned = scanned;

    let (candidates, skipped) =
        tokio::task::spawn_blocking(move || dedupe_across_roots(candidates))
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "dedupe task failed; indexing every candidate");
                (Vec::new(), Vec::new())
            });
    if !skipped.is_empty() {
        tracing::info!(
            duplicates = skipped.len(),
            "skipped mirrored archives present in several roots"
        );
    }

    let mut seen: HashSet<(i64, String)> = HashSet::with_capacity(candidates.len() + skipped.len());
    // A mirrored archive that is skipped is still on disk: keep any row that
    // was indexed for it before it became a duplicate.
    seen.extend(skipped);
    let mut new_games: Vec<i64> = Vec::new();
    let mut new_game_ids = HashSet::new();
    let mut added: i64 = 0;
    let mut updated: i64 = 0;

    for candidate in &candidates {
        // Record the file as present before indexing so a transient database
        // error never marks an existing file as missing.
        seen.insert((candidate.root_id, candidate.relative.clone()));
        match process_candidate(state, candidate).await {
            Ok(outcome) => {
                if outcome.version_created {
                    added += 1;
                } else {
                    updated += 1;
                }
                if outcome.game_created && new_game_ids.insert(outcome.game_id) {
                    new_games.push(outcome.game_id);
                }
            }
            Err(error) => {
                if errors.len() < 20 {
                    errors.push(format!("{}: {error}", candidate.relative));
                }
                tracing::warn!(
                    path = %candidate.relative,
                    error = %error,
                    "failed to index library file"
                );
            }
        }
    }

    let removed = match mark_missing(state, &seen, &scanned_roots).await {
        Ok(removed) => removed,
        Err(error) => {
            if errors.len() < 20 {
                errors.push(format!("mark missing: {error}"));
            }
            0
        }
    };

    let fingerprinted = backfill_fingerprints(state).await;

    {
        let mut status = state.scan.write().await;
        status.running = false;
        status.finished_at = Some(db::now());
        status.added = added;
        status.updated = updated;
        status.removed = removed;
        status.metadata_pending = i64::try_from(new_games.len()).unwrap_or(0);
        status.errors = errors;
    }

    state.metrics.record_scan(scan_started.elapsed());
    tracing::info!(
        scanned,
        added,
        updated,
        removed,
        roots = scanned_roots.len(),
        unavailable_roots = unavailable_roots.len(),
        fingerprinted,
        new_games = new_games.len(),
        "library scan finished"
    );

    if !new_games.is_empty() {
        for game_id in &new_games {
            let title = sqlx::query_scalar::<_, String>("SELECT title FROM games WHERE id = ?")
                .bind(game_id)
                .fetch_optional(&state.pool)
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            state.events.emit(
                Event::new(actions::GAME_INGESTED, Actor::System)
                    .target("game", game_id.to_string())
                    .metadata(serde_json::json!({ "title": title, "source": "scan" })),
            );
        }
        tokio::spawn(enrich_batch(state.clone(), new_games));
    }
}

async fn selected_roots(state: &AppState, root_id: Option<i64>) -> Result<Vec<LibraryRoot>> {
    match root_id {
        Some(id) => Ok(roots::get(&state.pool, id).await?.into_iter().collect()),
        None => roots::enabled(&state.pool).await,
    }
}

async fn mark_root_availability(state: &AppState, available: &[i64], unavailable: &[i64]) {
    for id in available {
        let _ = sqlx::query(
            "UPDATE game_versions SET root_unavailable = 0
             WHERE root_id = ? AND root_unavailable = 1",
        )
        .bind(id)
        .execute(&state.pool)
        .await;
    }
    for id in unavailable {
        let _ = sqlx::query(
            "UPDATE game_versions SET root_unavailable = 1, missing = 0 WHERE root_id = ?",
        )
        .bind(id)
        .execute(&state.pool)
        .await;
    }
}

async fn finish_with_error(state: &AppState, message: String) {
    tracing::error!(error = %message, "library scan failed");
    let mut status = state.scan.write().await;
    status.running = false;
    status.finished_at = Some(db::now());
    status.errors = vec![message];
}

struct ProcessOutcome {
    game_id: i64,
    version_created: bool,
    game_created: bool,
}

async fn process_candidate(state: &AppState, candidate: &Candidate) -> Result<ProcessOutcome> {
    let key = grouping_key(&candidate.parsed);
    let (game_id, game_created) = ensure_game(state, candidate, &key).await?;
    let version_created = upsert_version(state, game_id, candidate).await?;
    Ok(ProcessOutcome {
        game_id,
        version_created,
        game_created,
    })
}

async fn ensure_game(state: &AppState, candidate: &Candidate, key: &str) -> Result<(i64, bool)> {
    if let Some(id) =
        sqlx::query_scalar::<_, i64>("SELECT id FROM games WHERE group_key = ? ORDER BY id LIMIT 1")
            .bind(key)
            .fetch_optional(&state.pool)
            .await
            .context("find grouped game")?
    {
        return Ok((id, false));
    }

    let title = if candidate.parsed.title.trim().is_empty() {
        candidate.file_name.clone()
    } else {
        candidate.parsed.title.clone()
    };
    let sort_title = if candidate.parsed.sort_title.trim().is_empty() {
        title.clone()
    } else {
        candidate.parsed.sort_title.clone()
    };
    let release_date = candidate
        .parsed
        .year
        .and_then(db::date_from_year)
        .map(db::db_time);
    let tags_json = db::dump_json_list(&candidate.parsed.tags);
    let early_access = i64::from(looks_early_access(&candidate.file_name));
    let now = db::db_time(db::now());

    let result = sqlx::query(
        "INSERT INTO games
            (title, sort_title, group_key, release_date, tags_json, early_access,
             root_id, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&title)
    .bind(&sort_title)
    .bind(key)
    .bind(release_date)
    .bind(tags_json)
    .bind(early_access)
    .bind(candidate.root_id)
    .bind(&now)
    .bind(&now)
    .execute(&state.pool)
    .await
    .context("insert game")?;

    Ok((result.last_insert_rowid(), true))
}

async fn upsert_version(state: &AppState, game_id: i64, candidate: &Candidate) -> Result<bool> {
    let release_date = candidate
        .parsed
        .year
        .and_then(db::date_from_year)
        .map(db::db_time);
    let now = db::db_time(db::now());
    let version = candidate.parsed.version.clone();
    let early_access = i64::from(looks_early_access(&candidate.file_name));

    let existing = sqlx::query_as::<_, (i64, i64, Option<String>, Option<i64>)>(
        "SELECT id, size, archive_hash, file_modified FROM game_versions
         WHERE root_id = ? AND file_path = ? LIMIT 1",
    )
    .bind(candidate.root_id)
    .bind(&candidate.relative)
    .fetch_optional(&state.pool)
    .await
    .context("find indexed version")?;

    if let Some((id, previous_size, previous_hash, previous_modified)) = existing {
        // Re-fingerprint when the archive was replaced (size or mtime changed)
        // or when the row predates the fingerprint column.
        let replaced = previous_size != candidate.size
            || previous_modified.unwrap_or(-1) != candidate.modified;
        let fingerprint = if previous_hash.is_none() || replaced {
            fingerprint_for(candidate).await
        } else {
            previous_hash
        };
        sqlx::query(
            "UPDATE game_versions SET
                game_id = ?, file_name = ?, size = ?, version = ?,
                release_date = COALESCE(?, release_date), early_access = ?,
                missing = 0, root_unavailable = 0,
                content_hash = COALESCE(?, content_hash),
                archive_hash = ?, file_modified = ?, updated_at = ?,
                indexed_at = ?
             WHERE id = ?",
        )
        .bind(game_id)
        .bind(&candidate.file_name)
        .bind(candidate.size)
        .bind(version)
        .bind(release_date)
        .bind(early_access)
        .bind(candidate.content_hash.as_deref())
        .bind(fingerprint)
        .bind(candidate.modified)
        .bind(&now)
        .bind(&now)
        .bind(id)
        .execute(&state.pool)
        .await
        .context("update game version")?;
        Ok(false)
    } else {
        // Index the row first so the version is downloadable immediately; the
        // fingerprint is filled in right after (it reads up to 2 MiB).
        sqlx::query(
            "INSERT INTO game_versions
                (game_id, root_id, file_path, file_name, size, version, release_date,
                 early_access, missing, root_unavailable, content_hash, indexed_at,
                 archive_hash, file_modified, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, 0, ?, ?, NULL, ?, ?)",
        )
        .bind(game_id)
        .bind(candidate.root_id)
        .bind(&candidate.relative)
        .bind(&candidate.file_name)
        .bind(candidate.size)
        .bind(version)
        .bind(release_date)
        .bind(early_access)
        .bind(candidate.content_hash.as_deref())
        .bind(&now)
        .bind(candidate.modified)
        .bind(&now)
        .execute(&state.pool)
        .await
        .context("insert game version")?;

        if let Some(fingerprint) = fingerprint_for(candidate).await {
            sqlx::query(
                "UPDATE game_versions SET archive_hash = ?, updated_at = ?
                 WHERE root_id = ? AND file_path = ?",
            )
            .bind(fingerprint)
            .bind(db::db_time(db::now()))
            .bind(candidate.root_id)
            .bind(&candidate.relative)
            .execute(&state.pool)
            .await
            .context("store the archive fingerprint")?;
        }
        Ok(true)
    }
}

/// Cheap archive fingerprint for a discovered file, computed off the async
/// runtime. A failure is logged and leaves the row without a fingerprint: the
/// file is still indexed and update detection falls back to version ids.
async fn fingerprint_for(candidate: &Candidate) -> Option<String> {
    let path = candidate.root_path.join(&candidate.relative);
    let relative = candidate.relative.clone();
    match tokio::task::spawn_blocking(move || ludex_core::archive_fingerprint(&path)).await {
        Ok(Ok(value)) => Some(value),
        Ok(Err(error)) => {
            tracing::debug!(path = %relative, error = %error, "could not fingerprint archive");
            None
        }
        Err(error) => {
            tracing::debug!(path = %relative, error = %error, "fingerprint task failed");
            None
        }
    }
}

/// Fill fingerprints for rows indexed before the column existed (or whose
/// archive was unreadable during a previous scan). Bounded per scan so a huge
/// library does not turn every scan into a full read pass.
async fn backfill_fingerprints(state: &AppState) -> i64 {
    let rows = sqlx::query_as::<_, (i64, String, String)>(
        "SELECT v.id, v.file_path, r.path
         FROM game_versions v
         JOIN library_roots r ON r.id = v.root_id
         WHERE v.archive_hash IS NULL AND v.missing = 0 AND v.root_unavailable = 0
           AND r.available = 1
         LIMIT 200",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let mut updated = 0;
    for (id, relative, root_path) in rows {
        let path = PathBuf::from(root_path).join(&relative);
        let relative_for_log = relative.clone();
        let fingerprint = match tokio::task::spawn_blocking(move || {
            ludex_core::archive_fingerprint(&path)
        })
        .await
        {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => {
                tracing::debug!(path = %relative_for_log, error = %error, "could not fingerprint archive");
                continue;
            }
            Err(error) => {
                tracing::debug!(path = %relative_for_log, error = %error, "fingerprint task failed");
                continue;
            }
        };
        let result = sqlx::query(
            "UPDATE game_versions SET archive_hash = ?, updated_at = COALESCE(updated_at, indexed_at) WHERE id = ?",
        )
        .bind(fingerprint)
        .bind(id)
        .execute(&state.pool)
        .await;
        match result {
            Ok(_) => updated += 1,
            Err(error) => tracing::debug!(error = %error, "could not store archive fingerprint"),
        }
    }
    updated
}

/// Marks versions of scanned, available roots as missing when they vanished.
///
/// Versions under an unavailable root are never touched: the disk is gone, not
/// the game. A targeted scan only considers the roots it walked.
async fn mark_missing(
    state: &AppState,
    seen: &HashSet<(i64, String)>,
    scanned_roots: &[i64],
) -> Result<i64> {
    if scanned_roots.is_empty() {
        return Ok(0);
    }
    let placeholders = std::iter::repeat_n("?", scanned_roots.len())
        .collect::<Vec<_>>()
        .join(",");
    let query = format!(
        "SELECT v.id, v.game_id, v.file_path, v.root_id
         FROM game_versions v
         JOIN library_roots r ON r.id = v.root_id
         WHERE v.missing = 0 AND v.root_unavailable = 0 AND r.available = 1
           AND v.root_id IN ({placeholders})"
    );
    let mut request = sqlx::query_as::<_, (i64, i64, String, Option<i64>)>(&query);
    for id in scanned_roots {
        request = request.bind(id);
    }
    let rows = request
        .fetch_all(&state.pool)
        .await
        .context("list indexed versions")?;

    let mut removed = 0;
    for (id, game_id, file_path, root_id) in rows {
        let Some(root_id) = root_id else {
            continue;
        };
        if seen.contains(&(root_id, file_path.clone())) {
            continue;
        }
        sqlx::query("UPDATE game_versions SET missing = 1 WHERE id = ?")
            .bind(id)
            .execute(&state.pool)
            .await
            .context("mark version missing")?;
        state.events.emit(
            Event::new(actions::GAME_DELETED, Actor::System)
                .target("game", game_id.to_string())
                .metadata(serde_json::json!({
                    "filePath": file_path,
                    "rootId": root_id,
                    "source": "scan",
                })),
        );
        removed += 1;
    }
    Ok(removed)
}

async fn enrich_batch(state: AppState, game_ids: Vec<i64>) {
    use futures_util::StreamExt;
    futures_util::stream::iter(game_ids)
        .for_each_concurrent(2, |game_id| {
            let state = state.clone();
            async move {
                crate::metadata::enrich_game(&state, game_id).await;
                let mut status = state.scan.write().await;
                status.metadata_pending = (status.metadata_pending - 1).max(0);
            }
        })
        .await;
}

fn discover_root(root: &Path, root_id: i64) -> Result<Discovered> {
    if !root.is_dir() {
        return Ok(Discovered {
            available: false,
            error: Some("path is missing or not a directory".to_string()),
            candidates: Vec::new(),
        });
    }
    let mut candidates = Vec::new();
    for entry in WalkDir::new(root)
        .max_depth(MAX_DEPTH)
        .follow_links(false)
        .into_iter()
        .filter_entry(keep_entry)
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(mut candidate) = candidate_from_entry(root, &entry) else {
            continue;
        };
        candidate.root_id = root_id;
        candidates.push(candidate);
    }
    candidates.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(Discovered {
        available: true,
        error: None,
        candidates,
    })
}

/// Test-only convenience wrapper returning relative candidates.
#[cfg(test)]
fn discover(root: &Path) -> Result<Vec<Candidate>> {
    Ok(discover_root(root, 0)?.candidates)
}

/// Drops archives that are byte-identical mirrors in several roots.
///
/// Hashing every archive on every scan would be prohibitively expensive on a
/// multi-terabyte library, so only candidates that share a file name and size
/// across roots are hashed. Returns the kept candidates and how many were
/// skipped.
fn dedupe_across_roots(candidates: Vec<Candidate>) -> (Vec<Candidate>, Vec<(i64, String)>) {
    let mut groups: HashMap<(String, i64), Vec<usize>> = HashMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        groups
            .entry((candidate.file_name.to_ascii_lowercase(), candidate.size))
            .or_default()
            .push(index);
    }

    let mut dropped: HashSet<usize> = HashSet::new();
    for indexes in groups.values() {
        let roots: HashSet<i64> = indexes
            .iter()
            .map(|index| candidates[*index].root_id)
            .collect();
        if roots.len() < 2 {
            continue;
        }
        let mut kept_hash: Option<String> = None;
        for index in indexes {
            if dropped.contains(index) {
                continue;
            }
            let candidate = &candidates[*index];
            let Some(hash) = candidate.content_hash.clone().or_else(|| {
                let path = candidate.root_path.join(&candidate.relative);
                hash_file(&path).ok().map(|(_, hash)| hash)
            }) else {
                continue;
            };
            match &kept_hash {
                None => kept_hash = Some(hash),
                Some(known) if *known == hash => {
                    dropped.insert(*index);
                    tracing::debug!(
                        path = %candidate.relative,
                        root_id = candidate.root_id,
                        "skipping mirrored archive"
                    );
                }
                Some(_) => {}
            }
        }
    }

    let mut kept = Vec::with_capacity(candidates.len() - dropped.len());
    let mut skipped = Vec::with_capacity(dropped.len());
    for (index, candidate) in candidates.into_iter().enumerate() {
        if dropped.contains(&index) {
            skipped.push((candidate.root_id, candidate.relative));
        } else {
            kept.push(candidate);
        }
    }
    (kept, skipped)
}

/// Streaming SHA-256 of a file, with its size.
pub fn hash_file(path: &Path) -> std::io::Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    let mut size = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((size, hex::encode(hasher.finalize())))
}

/// Best-effort early-access detection from the file name.
fn looks_early_access(file_name: &str) -> bool {
    let lowered = file_name.to_ascii_lowercase();
    lowered.contains("(ea)")
        || lowered.contains("early access")
        || lowered.contains("(earlyaccess)")
}

fn keep_entry(entry: &DirEntry) -> bool {
    if entry.depth() == 0 {
        return true;
    }
    let name = entry.file_name().to_string_lossy();
    if name.starts_with('.') {
        return false;
    }
    if entry.file_type().is_dir() {
        let lowered = name.to_ascii_lowercase();
        if BLOCKED_DIRECTORIES.contains(&lowered.as_str()) {
            return false;
        }
    }
    true
}

fn candidate_from_entry(root: &Path, entry: &DirEntry) -> Option<Candidate> {
    let path = entry.path();
    let file_name = entry.file_name().to_str()?.to_string();
    let is_executable = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"));
    if !is_executable && !is_archive_path(path) {
        return None;
    }
    let lowered = file_name.to_ascii_lowercase();
    if lowered.ends_with(".part") || lowered.ends_with(".tmp") {
        return None;
    }
    if is_executable && (lowered.starts_with("unins") || lowered.starts_with("uninstall")) {
        return None;
    }
    let metadata = entry.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    let size = metadata.len();
    if !is_executable && size < MIN_ARCHIVE_BYTES {
        return None;
    }
    let relative = relative_path(root, path)?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    Some(Candidate {
        root_id: 0,
        root_path: root.to_path_buf(),
        relative,
        file_name: file_name.clone(),
        size: i64::try_from(size).unwrap_or(i64::MAX),
        modified,
        parsed: parse_filename(&file_name),
        content_hash: None,
    })
}

/// Returns the path relative to the library root, using `/` separators.
#[must_use]
pub fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for component in relative.components() {
        parts.push(component.as_os_str().to_str()?.to_string());
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Resolves a database `file_path` inside the library root, rejecting traversal.
#[must_use]
pub fn resolve_relative(root: &Path, relative: &str) -> Option<PathBuf> {
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        path.push(part);
    }
    path.starts_with(root).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path, size: u64) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        let file = fs::File::create(path).expect("create");
        file.set_len(size).expect("size");
    }

    #[test]
    fn discovery_skips_junk_and_respects_depth() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        touch(&root.join("Hades (2020).zip"), MIN_ARCHIVE_BYTES + 1);
        touch(&root.join("Hades (2020) v1.1.zip"), MIN_ARCHIVE_BYTES + 1);
        touch(&root.join("tiny.zip"), 1024);
        touch(&root.join("notes.txt"), MIN_ARCHIVE_BYTES * 2);
        touch(&root.join("Portable.exe"), 1024);
        touch(&root.join("unins000.exe"), 1024);
        touch(&root.join("Redist/dependency.zip"), MIN_ARCHIVE_BYTES * 2);
        touch(&root.join(".hidden/secret.zip"), MIN_ARCHIVE_BYTES * 2);
        touch(
            &root.join("Game/too/deep/nested.zip"),
            MIN_ARCHIVE_BYTES + 1,
        );

        let candidates = discover(root).expect("discover");
        let names: Vec<&str> = candidates
            .iter()
            .map(|candidate| candidate.file_name.as_str())
            .collect();
        assert!(names.contains(&"Hades (2020).zip"), "{names:?}");
        assert!(names.contains(&"Hades (2020) v1.1.zip"), "{names:?}");
        assert!(names.contains(&"Portable.exe"), "{names:?}");
        assert!(!names.contains(&"tiny.zip"), "{names:?}");
        assert!(!names.contains(&"notes.txt"), "{names:?}");
        assert!(!names.contains(&"unins000.exe"), "{names:?}");
        assert!(!names.contains(&"dependency.zip"), "{names:?}");
        assert!(!names.contains(&"secret.zip"), "{names:?}");
        assert!(!names.contains(&"nested.zip"), "{names:?}");
    }

    #[test]
    fn missing_roots_are_reported_unavailable() {
        let temp = tempfile::tempdir().expect("tempdir");
        let discovered = discover_root(&temp.path().join("gone"), 1).expect("discover");
        assert!(!discovered.available);
        assert!(discovered.candidates.is_empty());
        assert!(discovered.error.is_some());
    }

    #[test]
    fn mirrored_archives_are_deduped_by_content_hash() {
        let first = tempfile::tempdir().expect("tempdir");
        let second = tempfile::tempdir().expect("tempdir");
        touch(
            &first.path().join("Hades (2020).zip"),
            MIN_ARCHIVE_BYTES + 1,
        );
        touch(
            &second.path().join("Hades (2020).zip"),
            MIN_ARCHIVE_BYTES + 1,
        );
        let mut candidates = discover(first.path()).expect("first");
        candidates.extend(discover(second.path()).expect("second"));
        candidates[0].root_id = 1;
        candidates[1].root_id = 2;

        let (kept, skipped) = dedupe_across_roots(candidates);
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].0, 2);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].root_id, 1);
    }

    #[test]
    fn same_name_different_content_is_kept_twice() {
        let first = tempfile::tempdir().expect("tempdir");
        let second = tempfile::tempdir().expect("tempdir");
        let left = first.path().join("Hades (2020).zip");
        let right = second.path().join("Hades (2020).zip");
        touch(&left, MIN_ARCHIVE_BYTES + 1);
        touch(&right, MIN_ARCHIVE_BYTES + 2);
        // Same size, different bytes: same size is required to trigger hashing.
        let size = MIN_ARCHIVE_BYTES as usize + 1;
        fs::write(&left, vec![1_u8; size]).expect("write left");
        fs::write(&right, vec![2_u8; size]).expect("write right");
        let mut candidates = discover(first.path()).expect("first");
        candidates.extend(discover(second.path()).expect("second"));
        candidates[0].root_id = 1;
        candidates[1].root_id = 2;
        assert_eq!(candidates[0].size, candidates[1].size);

        let (kept, skipped) = dedupe_across_roots(candidates);
        assert!(skipped.is_empty());
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn early_access_is_detected_from_file_names() {
        assert!(looks_early_access("Upcoming (2024) (EA).zip"));
        assert!(looks_early_access("Game (Early Access).zip"));
        assert!(!looks_early_access("Hades (2020).zip"));
    }

    #[test]
    fn versions_of_the_same_title_share_a_group() {
        let first = parse_filename("Hades (2020).zip");
        let second = parse_filename("Hades (2020) v1.1.zip");
        assert_eq!(grouping_key(&first), grouping_key(&second));
        assert_eq!(first.year, Some(2020));
        assert_eq!(second.version.as_deref(), Some("1.1"));
    }

    #[test]
    fn relative_paths_roundtrip_and_reject_traversal() {
        let root = Path::new("/library");
        let relative = relative_path(root, &root.join("Game/Sub/file.zip")).expect("relative");
        assert_eq!(relative, "Game/Sub/file.zip");
        assert_eq!(
            resolve_relative(root, &relative).as_deref(),
            Some(root.join("Game/Sub/file.zip").as_path())
        );
        assert!(resolve_relative(root, "../etc/passwd").is_none());
        assert!(resolve_relative(root, "/etc/passwd").is_none());
        assert!(resolve_relative(root, "Game//file.zip").is_none());
    }

    #[test]
    fn scan_requests_merge_to_the_broadest_scope() {
        assert_eq!(
            ScanRequest::root(3).merge(ScanRequest::root(3)).root_id,
            Some(3)
        );
        assert_eq!(
            ScanRequest::root(3).merge(ScanRequest::root(4)).root_id,
            None
        );
        assert_eq!(ScanRequest::root(3).merge(ScanRequest::all()).root_id, None);
        assert_eq!(ScanRequest::all().merge(ScanRequest::root(3)).root_id, None);
    }

    #[test]
    fn hash_file_is_stable() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("file.bin");
        fs::write(&path, b"ludex").expect("write");
        let (size, hash) = hash_file(&path).expect("hash");
        assert_eq!(size, 5);
        assert_eq!(
            hash,
            "44e01fcd380808bc2738f7bfa8bd7b5ef9f387536083fa67215a3f171c9b702e".to_string()
        );
    }
}
