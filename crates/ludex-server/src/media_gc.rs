//! Media cache garbage collection (#18).
//!
//! The media directory accumulates covers, backgrounds and screenshots. When a
//! game is removed or its artwork is replaced, the old file stays behind.
//!
//! The collector computes the set of files referenced by the database
//! (`games.cover_path`, `games.background_path`, `games.screenshots_json`,
//! `users.avatar_path`) and removes only unreferenced files older than a grace
//! period (`LUDEX_MEDIA_GC_GRACE_HOURS`, default 24 h). The grace period
//! protects files written by an enrichment pass that has not committed its
//! database row yet, and a dry-run mode reports what would be deleted.

use std::time::{Duration, SystemTime};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::watch;

use crate::db;
use crate::error::ApiError;
use crate::integrity::referenced_media;
use crate::media::safe_media_name;
use crate::state::AppState;

/// Maximum number of orphan names returned in one response.
const MAX_REPORTED_CANDIDATES: usize = 100;

/// Result of one garbage collection pass.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaGcResult {
    /// Whether files were actually removed.
    pub dry_run: bool,
    /// Grace period applied, in hours.
    pub grace_hours: u64,
    /// Files present in the media directory.
    pub scanned: usize,
    /// Distinct media names referenced by the database.
    pub referenced: usize,
    /// Unreferenced files old enough to be collected.
    pub orphan_files: usize,
    /// Total size of the orphans, in bytes.
    pub orphan_bytes: u64,
    /// Files removed (zero on a dry run).
    pub deleted_files: usize,
    /// Bytes removed (zero on a dry run).
    pub deleted_bytes: u64,
    /// Up to 100 orphan file names, for confirmation.
    pub candidates: Vec<String>,
    /// Start timestamp.
    pub started_at: DateTime<Utc>,
    /// End timestamp.
    pub finished_at: DateTime<Utc>,
}

/// Scheduled interval, or `None` when disabled.
#[must_use]
pub fn interval(config: &crate::config::Config) -> Option<Duration> {
    (!config.media_gc_interval.is_zero()).then_some(config.media_gc_interval)
}

/// Collects unreferenced media files.
///
/// `dry_run = true` computes the plan without touching the filesystem. The
/// function never deletes a file whose name is referenced by any game or user
/// row, even when it is old.
pub async fn run(state: &AppState, dry_run: bool) -> Result<MediaGcResult, ApiError> {
    if state
        .media_gc_busy
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_err()
    {
        return Err(ApiError::conflict(
            "media_gc_running",
            "A media garbage collection is already running",
        ));
    }
    let result = collect(state, dry_run).await;
    state
        .media_gc_busy
        .store(false, std::sync::atomic::Ordering::SeqCst);
    result
}

async fn collect(state: &AppState, dry_run: bool) -> Result<MediaGcResult, ApiError> {
    let started_at = Utc::now();
    let started = db::db_time(started_at);
    let run_id = db::query("INSERT INTO media_gc_runs (dry_run, started_at) VALUES (?, ?)")
        .bind(i64::from(dry_run))
        .bind(&started)
        .returning_id(&state.pool)
        .await
        .map_err(ApiError::from)?;

    let referenced = referenced_media(&state.pool)
        .await
        .map_err(ApiError::from)?;
    let grace = state.config.media_gc_grace;
    let files = tokio::fs::read_dir(&state.config.media_dir).await;
    let mut scanned = 0_usize;
    let mut orphan_files = 0_usize;
    let mut orphan_bytes = 0_u64;
    let mut deleted_files = 0_usize;
    let mut deleted_bytes = 0_u64;
    let mut candidates: Vec<String> = Vec::new();

    if let Ok(mut entries) = files {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if !safe_media_name(&name) {
                continue;
            }
            let metadata = match entry.metadata().await {
                Ok(metadata) if metadata.is_file() => metadata,
                _ => continue,
            };
            scanned += 1;
            if referenced.contains(&name) {
                continue;
            }
            let age = metadata
                .modified()
                .ok()
                .and_then(|modified| SystemTime::now().duration_since(modified).ok());
            let Some(age) = age else {
                // Unknown age: never delete.
                continue;
            };
            if age < grace {
                continue;
            }
            orphan_files += 1;
            orphan_bytes += metadata.len();
            if candidates.len() < MAX_REPORTED_CANDIDATES {
                candidates.push(name.clone());
            }
            if dry_run {
                continue;
            }
            match tokio::fs::remove_file(entry.path()).await {
                Ok(()) => {
                    deleted_files += 1;
                    deleted_bytes += metadata.len();
                    let _ = db::query("DELETE FROM media_manifest WHERE name = ?")
                        .bind(&name)
                        .execute(&state.pool)
                        .await;
                }
                Err(error) => {
                    tracing::warn!(file = %name, %error, "could not delete orphaned media file");
                }
            }
        }
    }

    let finished_at = Utc::now();
    let _ = db::query(
        "UPDATE media_gc_runs SET scanned = ?, referenced = ?, orphan_files = ?,
            orphan_bytes = ?, deleted_files = ?, deleted_bytes = ?, finished_at = ?
         WHERE id = ?",
    )
    .bind(i64::try_from(scanned).unwrap_or(i64::MAX))
    .bind(i64::try_from(referenced.len()).unwrap_or(i64::MAX))
    .bind(i64::try_from(orphan_files).unwrap_or(i64::MAX))
    .bind(i64::try_from(orphan_bytes).unwrap_or(i64::MAX))
    .bind(i64::try_from(deleted_files).unwrap_or(i64::MAX))
    .bind(i64::try_from(deleted_bytes).unwrap_or(i64::MAX))
    .bind(db::db_time(finished_at))
    .bind(run_id)
    .execute(&state.pool)
    .await;

    tracing::info!(
        dry_run,
        scanned,
        referenced = referenced.len(),
        orphan_files,
        orphan_bytes,
        deleted_files,
        deleted_bytes,
        "media cache garbage collection finished"
    );

    Ok(MediaGcResult {
        dry_run,
        grace_hours: grace.as_secs() / 3_600,
        scanned,
        referenced: referenced.len(),
        orphan_files,
        orphan_bytes,
        deleted_files,
        deleted_bytes,
        candidates,
        started_at,
        finished_at,
    })
}

/// Runs a real collection on the configured interval. Disabled when zero.
pub async fn scheduled_worker(state: AppState, mut cancel: watch::Receiver<bool>) {
    let Some(interval) = interval(&state.config) else {
        return;
    };
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                // Never collect while a scan is running: the scan may be
                // enriching new games whose artwork is not referenced yet.
                if state.scan.read().await.running {
                    tracing::debug!("skipping scheduled media GC: a scan is running");
                    continue;
                }
                match run(&state, false).await {
                    Ok(result) => tracing::info!(
                        deleted = result.deleted_files,
                        bytes = result.deleted_bytes,
                        "scheduled media GC finished"
                    ),
                    Err(error) => tracing::debug!(code = %error.code, "scheduled media GC skipped"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::fs;
    use std::path::{Path, PathBuf};

    async fn test_state(root: &Path, grace: Duration) -> AppState {
        let data = root.join("data");
        let media = root.join("media");
        let games = root.join("games");
        fs::create_dir_all(&data).expect("data");
        fs::create_dir_all(&media).expect("media");
        fs::create_dir_all(&games).expect("games");
        let mut config = Config::for_local(games, data, media);
        config.database_url = format!(
            "sqlite://{}?mode=rwc",
            root.join("data").join("ludex.db").display()
        );
        config.media_gc_grace = grace;
        let (scan_tx, _scan_rx) = tokio::sync::mpsc::channel(4);
        AppState::for_tests(config, scan_tx).await
    }

    async fn reference_cover(state: &AppState, name: &str) {
        let now = db::db_time(db::now());
        db::query(
            "INSERT INTO games (title, sort_title, cover_path, screenshots_json,
                created_at, updated_at)
             VALUES ('Game', 'Game', ?, '[]', ?, ?)",
        )
        .bind(name)
        .bind(&now)
        .bind(&now)
        .execute(&state.pool)
        .await
        .expect("game");
    }

    #[tokio::test]
    async fn orphan_is_removed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = test_state(temp.path(), Duration::ZERO).await;
        let orphan = state.config.media_dir.join("orphan.jpg");
        fs::write(&orphan, vec![0_u8; 512]).expect("write");
        let result = run(&state, false).await.expect("gc");
        assert_eq!(result.orphan_files, 1);
        assert_eq!(result.deleted_files, 1);
        assert_eq!(result.deleted_bytes, 512);
        assert!(!orphan.exists());
    }

    #[tokio::test]
    async fn referenced_media_is_kept() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = test_state(temp.path(), Duration::ZERO).await;
        let cover = state.config.media_dir.join("cover.jpg");
        fs::write(&cover, vec![0_u8; 512]).expect("write");
        reference_cover(&state, "cover.jpg").await;
        let result = run(&state, false).await.expect("gc");
        assert_eq!(result.orphan_files, 0);
        assert_eq!(result.deleted_files, 0);
        assert!(cover.exists());
    }

    #[tokio::test]
    async fn screenshots_are_referenced() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = test_state(temp.path(), Duration::ZERO).await;
        let shot = state.config.media_dir.join("shot.jpg");
        fs::write(&shot, vec![0_u8; 64]).expect("write");
        let now = db::db_time(db::now());
        db::query(
            "INSERT INTO games (title, sort_title, screenshots_json, created_at, updated_at)
             VALUES ('Game', 'Game', '[\"shot.jpg\"]', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&state.pool)
        .await
        .expect("game");
        let result = run(&state, false).await.expect("gc");
        assert_eq!(result.deleted_files, 0);
        assert!(shot.exists());
    }

    #[tokio::test]
    async fn grace_period_protects_fresh_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = test_state(temp.path(), Duration::from_secs(24 * 3_600)).await;
        let fresh = state.config.media_dir.join("fresh.jpg");
        fs::write(&fresh, vec![0_u8; 32]).expect("write");

        let result = run(&state, false).await.expect("gc");
        assert_eq!(result.grace_hours, 24);
        assert_eq!(result.orphan_files, 0);
        assert!(fresh.exists());
    }

    #[tokio::test]
    async fn dry_run_reports_without_deleting() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = test_state(temp.path(), Duration::ZERO).await;
        let orphan = state.config.media_dir.join("orphan.jpg");
        fs::write(&orphan, vec![0_u8; 128]).expect("write");
        let result = run(&state, true).await.expect("gc");
        assert!(result.dry_run);
        assert_eq!(result.orphan_files, 1);
        assert_eq!(result.orphan_bytes, 128);
        assert_eq!(result.deleted_files, 0);
        assert!(orphan.exists());
        assert_eq!(result.candidates, vec!["orphan.jpg".to_string()]);
    }

    #[test]
    fn intervals_are_configurable() {
        let mut config = Config::for_local(
            PathBuf::from("/tmp/games"),
            PathBuf::from("/tmp/data"),
            PathBuf::from("/tmp/media"),
        );
        assert_eq!(interval(&config), None);
        config.media_gc_interval = Duration::from_secs(3_600);
        assert_eq!(interval(&config), Some(Duration::from_secs(3_600)));
    }
}
