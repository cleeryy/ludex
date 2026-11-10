//! Integrity checks (#43).
//!
//! Archives are verified against the size recorded by the scanner and against
//! a SHA-256 baseline stored in `game_versions.content_hash`. Cached artwork is
//! verified against `media_manifest`. The first successful hash becomes the
//! baseline, so the check is cheap on later runs and still detects truncation
//! and bit rot.
//!
//! Findings never delete anything: a corrupt or missing file is recorded in
//! `integrity_findings` for an administrator to act on.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::watch;

use crate::db;
use crate::error::ApiError;
use crate::media::safe_media_name;
use crate::scanner::hash_file;
use crate::state::AppState;

/// Number of files between progress writes.
const PROGRESS_EVERY: i64 = 25;

/// What an integrity run verifies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IntegrityScope {
    /// Archives and cached media.
    #[default]
    All,
    /// Library archives only.
    Archives,
    /// Cached artwork only.
    Media,
}

impl IntegrityScope {
    /// Stable value stored in `integrity_reports.scope`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Archives => "archives",
            Self::Media => "media",
        }
    }

    /// Parses a scope from the API or the database.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "all" | "" => Some(Self::All),
            "archives" | "archive" => Some(Self::Archives),
            "media" => Some(Self::Media),
            _ => None,
        }
    }
}

/// Row of `integrity_reports`.
#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrityReport {
    /// Report id.
    pub id: i64,
    /// Scope that was checked.
    pub scope: String,
    /// `running`, `completed` or `failed`.
    pub status: String,
    /// Files examined.
    pub checked: i64,
    /// Findings recorded.
    pub findings_count: i64,
    /// Fatal error, if the run aborted.
    pub error: Option<String>,
    /// Start timestamp.
    pub started_at: String,
    /// End timestamp.
    pub finished_at: Option<String>,
}

/// Row of `integrity_findings`.
#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrityFinding {
    /// Finding id.
    pub id: i64,
    /// `missing`, `corrupt` or `size_mismatch`.
    pub kind: String,
    /// `archive` or `media`.
    pub target_type: String,
    /// Version id or media file name.
    pub target_id: Option<String>,
    /// Absolute path, when known.
    pub path: Option<String>,
    /// Human-readable detail.
    pub detail: Option<String>,
    /// When the finding was recorded.
    pub created_at: String,
}

/// Scheduled interval, or `None` when disabled.
#[must_use]
pub fn interval(config: &crate::config::Config) -> Option<Duration> {
    (!config.integrity_interval.is_zero()).then_some(config.integrity_interval)
}

/// Starts an asynchronous integrity run.
///
/// Returns the new report id. Only one run can be active per process.
pub async fn start(state: &AppState, scope: IntegrityScope) -> Result<i64, ApiError> {
    if state
        .integrity_busy
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_err()
    {
        return Err(ApiError::conflict(
            "integrity_running",
            "An integrity check is already running",
        ));
    }
    let report_id = match insert_report(&state.pool, scope).await {
        Ok(id) => id,
        Err(error) => {
            state
                .integrity_busy
                .store(false, std::sync::atomic::Ordering::SeqCst);
            return Err(ApiError::from(error));
        }
    };
    let state = state.clone();
    tokio::spawn(async move {
        run_report(&state, report_id, scope).await;
        state
            .integrity_busy
            .store(false, std::sync::atomic::Ordering::SeqCst);
    });
    Ok(report_id)
}

async fn insert_report(pool: &SqlitePool, scope: IntegrityScope) -> Result<i64> {
    let result = sqlx::query(
        "INSERT INTO integrity_reports (scope, status, checked, findings_count, started_at)
         VALUES (?, 'running', 0, 0, ?)",
    )
    .bind(scope.as_str())
    .bind(db::db_time(db::now()))
    .execute(pool)
    .await
    .context("insert integrity report")?;
    Ok(result.last_insert_rowid())
}

/// Runs one report. Public so tests can exercise it without the task spawner.
pub async fn run_report(state: &AppState, report_id: i64, scope: IntegrityScope) {
    let mut checked: i64 = 0;
    let mut findings: i64 = 0;
    let mut error: Option<String> = None;

    if matches!(scope, IntegrityScope::All | IntegrityScope::Archives) {
        match check_archives(state, report_id, &mut checked, &mut findings).await {
            Ok(()) => {}
            Err(failure) => error = Some(format!("archives: {failure}")),
        }
    }
    if matches!(scope, IntegrityScope::All | IntegrityScope::Media) {
        match check_media(state, report_id, &mut checked, &mut findings).await {
            Ok(()) => {}
            Err(failure) => {
                let message = format!("media: {failure}");
                error =
                    Some(error.map_or(message.clone(), |current| format!("{current}; {message}")));
            }
        }
    }

    let status = if error.is_some() {
        "failed"
    } else {
        "completed"
    };
    let result = sqlx::query(
        "UPDATE integrity_reports SET status = ?, checked = ?, findings_count = ?,
            error = ?, finished_at = ? WHERE id = ?",
    )
    .bind(status)
    .bind(checked)
    .bind(findings)
    .bind(error.as_deref())
    .bind(db::db_time(db::now()))
    .bind(report_id)
    .execute(&state.pool)
    .await;
    if let Err(failure) = result {
        tracing::error!(%failure, report_id, "could not finish integrity report");
    }
    tracing::info!(
        report_id,
        scope = scope.as_str(),
        checked,
        findings,
        status,
        "integrity check finished"
    );
}

async fn check_archives(
    state: &AppState,
    report_id: i64,
    checked: &mut i64,
    findings: &mut i64,
) -> Result<()> {
    let rows = sqlx::query_as::<_, (i64, Option<i64>, String, i64, Option<String>)>(
        "SELECT id, root_id, file_path, size, content_hash
         FROM game_versions WHERE missing = 0 AND root_unavailable = 0
         ORDER BY id",
    )
    .fetch_all(&state.pool)
    .await
    .context("list versions for integrity")?;

    for (version_id, root_id, file_path, expected_size, expected_hash) in rows {
        *checked += 1;
        let Some(path) =
            crate::roots::resolve_version_path(&state.pool, &state.config, root_id, &file_path)
                .await
        else {
            record_finding(
                &state.pool,
                report_id,
                "corrupt",
                "archive",
                Some(&version_id.to_string()),
                Some(&file_path),
                Some("path escapes the library root"),
            )
            .await?;
            *findings += 1;
            continue;
        };
        let metadata = tokio::fs::metadata(&path).await;
        let Ok(metadata) = metadata else {
            record_finding(
                &state.pool,
                report_id,
                "missing",
                "archive",
                Some(&version_id.to_string()),
                Some(&path.display().to_string()),
                Some("file is missing from disk"),
            )
            .await?;
            *findings += 1;
            continue;
        };
        if !metadata.is_file() {
            record_finding(
                &state.pool,
                report_id,
                "missing",
                "archive",
                Some(&version_id.to_string()),
                Some(&path.display().to_string()),
                Some("path is not a regular file"),
            )
            .await?;
            *findings += 1;
            continue;
        }
        if expected_size > 0 && metadata.len() != u64::try_from(expected_size).unwrap_or(0) {
            record_finding(
                &state.pool,
                report_id,
                "size_mismatch",
                "archive",
                Some(&version_id.to_string()),
                Some(&path.display().to_string()),
                Some(&format!(
                    "expected {expected_size} bytes, found {}",
                    metadata.len()
                )),
            )
            .await?;
            *findings += 1;
            continue;
        }

        let hash_path = path.clone();
        let hash = tokio::task::spawn_blocking(move || hash_file(&hash_path)).await;
        let Ok(Ok((_, digest))) = hash else {
            record_finding(
                &state.pool,
                report_id,
                "corrupt",
                "archive",
                Some(&version_id.to_string()),
                Some(&path.display().to_string()),
                Some("file could not be read for hashing"),
            )
            .await?;
            *findings += 1;
            continue;
        };
        match expected_hash {
            None => {
                sqlx::query("UPDATE game_versions SET content_hash = ? WHERE id = ?")
                    .bind(&digest)
                    .bind(version_id)
                    .execute(&state.pool)
                    .await
                    .context("record archive hash baseline")?;
            }
            Some(expected) if expected != digest => {
                record_finding(
                    &state.pool,
                    report_id,
                    "corrupt",
                    "archive",
                    Some(&version_id.to_string()),
                    Some(&path.display().to_string()),
                    Some(&format!("expected sha256 {expected}, found {digest}")),
                )
                .await?;
                *findings += 1;
            }
            Some(_) => {}
        }

        if *checked % PROGRESS_EVERY == 0 {
            update_progress(&state.pool, report_id, *checked, *findings).await?;
        }
    }
    Ok(())
}

async fn check_media(
    state: &AppState,
    report_id: i64,
    checked: &mut i64,
    findings: &mut i64,
) -> Result<()> {
    let referenced = referenced_media(&state.pool).await?;
    for name in referenced {
        if !safe_media_name(&name) {
            continue;
        }
        *checked += 1;
        let path = state.config.media_dir.join(&name);
        let metadata = tokio::fs::metadata(&path).await;
        let Ok(metadata) = metadata else {
            record_finding(
                &state.pool,
                report_id,
                "missing",
                "media",
                Some(&name),
                Some(&path.display().to_string()),
                Some("referenced media file is missing"),
            )
            .await?;
            *findings += 1;
            continue;
        };
        if !metadata.is_file() {
            record_finding(
                &state.pool,
                report_id,
                "missing",
                "media",
                Some(&name),
                Some(&path.display().to_string()),
                Some("path is not a regular file"),
            )
            .await?;
            *findings += 1;
            continue;
        }

        let hash_path = path.clone();
        let hash = tokio::task::spawn_blocking(move || hash_file(&hash_path)).await;
        let Ok(Ok((size, digest))) = hash else {
            record_finding(
                &state.pool,
                report_id,
                "corrupt",
                "media",
                Some(&name),
                Some(&path.display().to_string()),
                Some("file could not be read for hashing"),
            )
            .await?;
            *findings += 1;
            continue;
        };

        let baseline = sqlx::query_as::<_, (String, i64)>(
            "SELECT sha256, size FROM media_manifest WHERE name = ?",
        )
        .bind(&name)
        .fetch_optional(&state.pool)
        .await
        .context("read media manifest")?;
        match baseline {
            None => {
                let now = db::db_time(db::now());
                sqlx::query(
                    "INSERT INTO media_manifest (name, size, sha256, created_at, verified_at)
                     VALUES (?, ?, ?, ?, ?)
                     ON CONFLICT(name) DO UPDATE SET size = excluded.size,
                        sha256 = excluded.sha256, verified_at = excluded.verified_at",
                )
                .bind(&name)
                .bind(i64::try_from(size).unwrap_or(i64::MAX))
                .bind(&digest)
                .bind(&now)
                .bind(&now)
                .execute(&state.pool)
                .await
                .context("record media hash baseline")?;
            }
            Some((expected_hash, expected_size)) => {
                if expected_hash != digest
                    || u64::try_from(expected_size).unwrap_or(u64::MAX) != size
                {
                    record_finding(
                        &state.pool,
                        report_id,
                        "corrupt",
                        "media",
                        Some(&name),
                        Some(&path.display().to_string()),
                        Some("cached artwork differs from its manifest"),
                    )
                    .await?;
                    *findings += 1;
                } else {
                    sqlx::query("UPDATE media_manifest SET verified_at = ? WHERE name = ?")
                        .bind(db::db_time(db::now()))
                        .bind(&name)
                        .execute(&state.pool)
                        .await
                        .context("touch media manifest")?;
                }
            }
        }
    }
    Ok(())
}

/// Media file names referenced by game rows or user avatars.
pub async fn referenced_media(pool: &SqlitePool) -> Result<HashSet<String>> {
    let mut referenced: HashSet<String> = HashSet::new();
    let games = sqlx::query_as::<_, (Option<String>, Option<String>, String)>(
        "SELECT cover_path, background_path, screenshots_json FROM games",
    )
    .fetch_all(pool)
    .await
    .context("list game media references")?;
    for (cover, background, screenshots) in games {
        if let Some(name) = cover.filter(|value| !value.is_empty()) {
            referenced.insert(name);
        }
        if let Some(name) = background.filter(|value| !value.is_empty()) {
            referenced.insert(name);
        }
        referenced.extend(
            db::parse_json_list(&screenshots)
                .into_iter()
                .filter(|name| !name.is_empty()),
        );
    }
    let avatars = sqlx::query_scalar::<_, String>(
        "SELECT avatar_path FROM users WHERE avatar_path IS NOT NULL AND avatar_path != ''",
    )
    .fetch_all(pool)
    .await
    .context("list avatar references")?;
    referenced.extend(avatars);
    Ok(referenced)
}

async fn record_finding(
    pool: &SqlitePool,
    report_id: i64,
    kind: &str,
    target_type: &str,
    target_id: Option<&str>,
    path: Option<&str>,
    detail: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO integrity_findings
            (report_id, kind, target_type, target_id, path, detail, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(report_id)
    .bind(kind)
    .bind(target_type)
    .bind(target_id)
    .bind(path)
    .bind(detail)
    .bind(db::db_time(db::now()))
    .execute(pool)
    .await
    .context("insert integrity finding")?;
    Ok(())
}

async fn update_progress(
    pool: &SqlitePool,
    report_id: i64,
    checked: i64,
    findings: i64,
) -> Result<()> {
    sqlx::query("UPDATE integrity_reports SET checked = ?, findings_count = ? WHERE id = ?")
        .bind(checked)
        .bind(findings)
        .bind(report_id)
        .execute(pool)
        .await
        .context("update integrity progress")?;
    Ok(())
}

/// Runs a check on the configured interval. Disabled when the interval is zero.
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
                match start(&state, IntegrityScope::All).await {
                    Ok(report_id) => tracing::info!(report_id, "scheduled integrity check started"),
                    Err(error) => tracing::debug!(code = %error.code, "scheduled integrity check skipped"),
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

/// Lists recent reports, newest first.
pub async fn list_reports(pool: &SqlitePool, limit: i64) -> Result<Vec<IntegrityReport>> {
    sqlx::query_as::<_, IntegrityReport>(
        "SELECT id, scope, status, checked, findings_count, error, started_at, finished_at
         FROM integrity_reports ORDER BY id DESC LIMIT ?",
    )
    .bind(limit.clamp(1, 200))
    .fetch_all(pool)
    .await
    .context("list integrity reports")
}

/// Reads one report and its findings.
pub async fn read_report(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<(IntegrityReport, Vec<IntegrityFinding>)>> {
    let report = sqlx::query_as::<_, IntegrityReport>(
        "SELECT id, scope, status, checked, findings_count, error, started_at, finished_at
         FROM integrity_reports WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .context("read integrity report")?;
    let Some(report) = report else {
        return Ok(None);
    };
    let findings = sqlx::query_as::<_, IntegrityFinding>(
        "SELECT id, kind, target_type, target_id, path, detail, created_at
         FROM integrity_findings WHERE report_id = ? ORDER BY id",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .context("read integrity findings")?;
    Ok(Some((report, findings)))
}

/// Latest finished report timestamp, used by the status endpoint.
pub async fn last_finished_at(pool: &SqlitePool) -> Option<DateTime<Utc>> {
    sqlx::query_scalar::<_, String>(
        "SELECT finished_at FROM integrity_reports
         WHERE finished_at IS NOT NULL ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .as_deref()
    .and_then(db::parse_time)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::fs;
    use std::path::{Path, PathBuf};

    async fn test_state(root: &Path) -> AppState {
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
        let (scan_tx, _scan_rx) = tokio::sync::mpsc::channel(4);
        AppState::for_tests(config, scan_tx).await
    }

    async fn seed_version(state: &AppState, name: &str, size: i64, hash: Option<&str>) -> i64 {
        let root_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM library_roots ORDER BY position, id LIMIT 1",
        )
        .fetch_one(&state.pool)
        .await
        .expect("root");
        let now = db::db_time(db::now());
        let game = sqlx::query(
            "INSERT INTO games (title, sort_title, group_key, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(name)
        .bind(name)
        .bind(name)
        .bind(&now)
        .bind(&now)
        .execute(&state.pool)
        .await
        .expect("game");
        let result = sqlx::query(
            "INSERT INTO game_versions
                (game_id, root_id, file_path, file_name, size, missing, content_hash, indexed_at)
             VALUES (?, ?, ?, ?, ?, 0, ?, ?)",
        )
        .bind(game.last_insert_rowid())
        .bind(root_id)
        .bind(name)
        .bind(name)
        .bind(size)
        .bind(hash)
        .bind(&now)
        .execute(&state.pool)
        .await
        .expect("version");
        result.last_insert_rowid()
    }

    async fn run(state: &AppState) -> (IntegrityReport, Vec<IntegrityFinding>) {
        let report_id = insert_report(&state.pool, IntegrityScope::Archives)
            .await
            .expect("report");
        run_report(state, report_id, IntegrityScope::Archives).await;
        read_report(&state.pool, report_id)
            .await
            .expect("read")
            .expect("report")
    }

    #[tokio::test]
    async fn clean_run_records_a_baseline_and_no_findings() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = test_state(temp.path()).await;
        let content = vec![7_u8; 2048];
        fs::write(state.config.games_dir.join("Hades (2020).zip"), &content).expect("write");
        let (_, digest) =
            hash_file(&state.config.games_dir.join("Hades (2020).zip")).expect("hash");
        let version_id = seed_version(&state, "Hades (2020).zip", 2048, None).await;

        let (report, findings) = run(&state).await;
        assert_eq!(report.status, "completed", "{report:?}");
        assert_eq!(report.checked, 1);
        assert!(findings.is_empty(), "{findings:?}");
        let stored =
            sqlx::query_scalar::<_, String>("SELECT content_hash FROM game_versions WHERE id = ?")
                .bind(version_id)
                .fetch_one(&state.pool)
                .await
                .expect("hash");
        assert_eq!(stored, digest);
    }

    #[tokio::test]
    async fn corrupt_archive_is_reported() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = test_state(temp.path()).await;
        fs::write(state.config.games_dir.join("Celeste.zip"), vec![1_u8; 2048]).expect("write");
        seed_version(&state, "Celeste.zip", 2048, Some("deadbeef")).await;

        let (report, findings) = run(&state).await;
        assert_eq!(report.status, "completed");
        assert_eq!(report.findings_count, 1);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, "corrupt");
        assert_eq!(findings[0].target_type, "archive");
    }

    #[tokio::test]
    async fn missing_archive_is_reported_without_deleting_the_row() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = test_state(temp.path()).await;
        let version_id = seed_version(&state, "Gone.zip", 2048, None).await;

        let (report, findings) = run(&state).await;
        assert_eq!(report.findings_count, 1);
        assert_eq!(findings[0].kind, "missing");
        let missing: i64 = sqlx::query_scalar("SELECT missing FROM game_versions WHERE id = ?")
            .bind(version_id)
            .fetch_one(&state.pool)
            .await
            .expect("row");
        assert_eq!(missing, 0, "integrity checks never delete or mark rows");
    }

    #[tokio::test]
    async fn size_mismatch_is_reported() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = test_state(temp.path()).await;
        fs::write(state.config.games_dir.join("Short.zip"), vec![1_u8; 100]).expect("write");
        seed_version(&state, "Short.zip", 2048, None).await;

        let (_, findings) = run(&state).await;
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, "size_mismatch");
    }

    #[tokio::test]
    async fn media_corruption_is_detected_from_the_manifest() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = test_state(temp.path()).await;
        let media = state.config.media_dir.join("cover.jpg");
        fs::write(&media, b"good").expect("write media");
        sqlx::query(
            "INSERT INTO media_manifest (name, size, sha256, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind("cover.jpg")
        .bind(4_i64)
        .bind("not-the-real-hash")
        .bind(db::db_time(db::now()))
        .execute(&state.pool)
        .await
        .expect("manifest");
        let now = db::db_time(db::now());
        sqlx::query(
            "INSERT INTO games (title, sort_title, cover_path, created_at, updated_at)
             VALUES ('Game', 'Game', 'cover.jpg', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&state.pool)
        .await
        .expect("game");

        let report_id = insert_report(&state.pool, IntegrityScope::Media)
            .await
            .expect("report");
        run_report(&state, report_id, IntegrityScope::Media).await;
        let (report, findings) = read_report(&state.pool, report_id)
            .await
            .expect("read")
            .expect("report");
        assert_eq!(report.findings_count, 1, "{report:?}");
        assert_eq!(findings[0].kind, "corrupt");
        assert_eq!(findings[0].target_type, "media");
    }

    #[tokio::test]
    async fn media_missing_file_is_reported() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = test_state(temp.path()).await;
        let now = db::db_time(db::now());
        sqlx::query(
            "INSERT INTO games (title, sort_title, screenshots_json, created_at, updated_at)
             VALUES ('Game', 'Game', '[\"shot.jpg\"]', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&state.pool)
        .await
        .expect("game");

        let report_id = insert_report(&state.pool, IntegrityScope::Media)
            .await
            .expect("report");
        run_report(&state, report_id, IntegrityScope::Media).await;
        let (_, findings) = read_report(&state.pool, report_id)
            .await
            .expect("read")
            .expect("report");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, "missing");
        assert_eq!(findings[0].target_id.as_deref(), Some("shot.jpg"));
    }

    #[test]
    fn scopes_parse_and_intervals_are_configurable() {
        assert_eq!(IntegrityScope::parse("ALL"), Some(IntegrityScope::All));
        assert_eq!(
            IntegrityScope::parse("archives"),
            Some(IntegrityScope::Archives)
        );
        assert_eq!(IntegrityScope::parse("nope"), None);

        let mut config = Config::for_local(
            PathBuf::from("/tmp/games"),
            PathBuf::from("/tmp/data"),
            PathBuf::from("/tmp/media"),
        );
        assert_eq!(interval(&config), Some(Duration::from_secs(168 * 3_600)));
        config.integrity_interval = Duration::ZERO;
        assert_eq!(interval(&config), None);
    }
}
