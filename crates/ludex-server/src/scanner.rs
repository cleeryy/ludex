//! Library scanner: walks the games volume, groups files into logical games and
//! keeps the database in sync without ever deleting user progress.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use ludex_core::archive::is_archive_path;
use ludex_core::naming::{grouping_key, parse_filename, ParsedName};
use tokio::sync::{mpsc, watch};
use walkdir::{DirEntry, WalkDir};

use crate::audit::actions;
use crate::db;
use crate::events::{Actor, Event};
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

struct Candidate {
    relative: String,
    file_name: String,
    size: i64,
    parsed: ParsedName,
}

/// Triggers a scan when none is running.
pub async fn trigger(state: &AppState) -> bool {
    if state.scan.read().await.running {
        return false;
    }
    state.scan_tx.try_send(()).is_ok()
}

/// Background worker: consumes scan requests and the periodic timer.
pub async fn worker(
    state: AppState,
    mut rx: mpsc::Receiver<()>,
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
                if message.is_none() {
                    break;
                }
                // Coalesce burst requests into a single pass.
                while rx.try_recv().is_ok() {}
                run_scan(&state).await;
            }
            () = tick => {
                run_scan(&state).await;
            }
        }
    }
    tracing::debug!("library scan worker stopped");
}

/// Runs one complete scan. Errors are recorded in the status, never returned.
pub async fn run_scan(state: &AppState) {
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

    let root = state.config.games_dir.clone();
    let discovery = tokio::task::spawn_blocking(move || discover(&root)).await;
    let candidates = match discovery {
        Ok(Ok(candidates)) => candidates,
        Ok(Err(error)) => {
            finish_with_error(state, format!("scan failed: {error}")).await;
            return;
        }
        Err(error) => {
            finish_with_error(state, format!("scan task failed: {error}")).await;
            return;
        }
    };

    let scanned = i64::try_from(candidates.len()).unwrap_or(i64::MAX);
    state.scan.write().await.scanned = scanned;

    let mut seen = HashSet::with_capacity(candidates.len());
    let mut new_games: Vec<i64> = Vec::new();
    let mut new_game_ids = HashSet::new();
    let mut errors: Vec<String> = Vec::new();
    let mut added: i64 = 0;
    let mut updated: i64 = 0;

    for candidate in &candidates {
        // Record the file as present before indexing so a transient database
        // error never marks an existing file as missing.
        seen.insert(candidate.relative.clone());
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

    let removed = match mark_missing(state, &seen).await {
        Ok(removed) => removed,
        Err(error) => {
            if errors.len() < 20 {
                errors.push(format!("mark missing: {error}"));
            }
            0
        }
    };

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

    tracing::info!(
        scanned,
        added,
        updated,
        removed,
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
             created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&title)
    .bind(&sort_title)
    .bind(key)
    .bind(release_date)
    .bind(tags_json)
    .bind(early_access)
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

    let existing =
        sqlx::query_scalar::<_, i64>("SELECT id FROM game_versions WHERE file_path = ? LIMIT 1")
            .bind(&candidate.relative)
            .fetch_optional(&state.pool)
            .await
            .context("find indexed version")?;

    if let Some(id) = existing {
        sqlx::query(
            "UPDATE game_versions SET
                game_id = ?, file_name = ?, size = ?, version = ?,
                release_date = COALESCE(?, release_date), early_access = ?,
                missing = 0, indexed_at = ?
             WHERE id = ?",
        )
        .bind(game_id)
        .bind(&candidate.file_name)
        .bind(candidate.size)
        .bind(version)
        .bind(release_date)
        .bind(early_access)
        .bind(now)
        .bind(id)
        .execute(&state.pool)
        .await
        .context("update game version")?;
        Ok(false)
    } else {
        sqlx::query(
            "INSERT INTO game_versions
                (game_id, file_path, file_name, size, version, release_date,
                 early_access, missing, indexed_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?)",
        )
        .bind(game_id)
        .bind(&candidate.relative)
        .bind(&candidate.file_name)
        .bind(candidate.size)
        .bind(version)
        .bind(release_date)
        .bind(early_access)
        .bind(now)
        .execute(&state.pool)
        .await
        .context("insert game version")?;
        Ok(true)
    }
}

async fn mark_missing(state: &AppState, seen: &HashSet<String>) -> Result<i64> {
    let rows = sqlx::query_as::<_, (i64, i64, String)>(
        "SELECT id, game_id, file_path FROM game_versions WHERE missing = 0",
    )
    .fetch_all(&state.pool)
    .await
    .context("list indexed versions")?;

    let mut removed = 0;
    for (id, game_id, file_path) in rows {
        if seen.contains(&file_path) {
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
                .metadata(serde_json::json!({ "filePath": file_path, "source": "scan" })),
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

fn discover(root: &Path) -> Result<Vec<Candidate>> {
    if !root.is_dir() {
        return Ok(Vec::new());
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
        let Some(candidate) = candidate_from_entry(root, &entry) else {
            continue;
        };
        candidates.push(candidate);
    }
    candidates.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(candidates)
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
    Some(Candidate {
        relative,
        file_name: file_name.clone(),
        size: i64::try_from(size).unwrap_or(i64::MAX),
        parsed: parse_filename(&file_name),
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
}
