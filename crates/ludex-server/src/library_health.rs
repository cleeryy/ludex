//! Library Health Center (#165).
//!
//! One cached report answers "is my library healthy?" instead of leaving an
//! administrator to browse games one by one. The report walks the configured
//! roots, the indexed rows, the media cache and the last integrity run, then
//! stores typed findings with a severity, plain-language detail, evidence and
//! the safe actions the web UI may offer.
//!
//! Guarantees:
//!
//! * Nothing here deletes an archive file. The only file deletion a finding can
//!   trigger is the media cache collector, which only removes unreferenced
//!   artwork older than its grace period.
//! * Removing a database row requires an explicit `confirm: true` and is
//!   restricted to the administrator role by the HTTP layer.
//! * Findings are computed without hashing archives: the deep checks stay in
//!   [`crate::integrity`] and are surfaced here as cached findings.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::db::{self, Database};
use crate::error::ApiError;
use crate::media::safe_media_name;
use crate::state::AppState;

/// Number of reports kept in the cache.
const KEEP_REPORTS: i64 = 20;

/// Maximum number of selected findings one action may process.
const MAX_ACTION_FINDINGS: usize = 200;

/// Maximum number of games one metadata refresh action may enqueue.
const MAX_METADATA_REFRESH: usize = 50;

/// Stable trigger names stored in `library_health_reports.trigger`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthTrigger {
    /// Started from the admin page.
    Manual,
    /// Started by the nightly worker.
    Scheduled,
    /// Started automatically after a repair action.
    Action,
}

impl HealthTrigger {
    /// Value stored in the database.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Scheduled => "scheduled",
            Self::Action => "action",
        }
    }
}

/// Severities, ordered by urgency.
pub mod severity {
    /// Broken: data is missing or unreadable.
    pub const ERROR: &str = "error";
    /// Degraded: likely a problem, or a repair is pending.
    pub const WARNING: &str = "warning";
    /// Informational: cleanup or metadata opportunity.
    pub const INFO: &str = "info";
}

/// Finding categories used by the summary cards and filters.
pub mod category {
    /// Referenced data that is missing.
    pub const MISSING: &str = "missing";
    /// Roots or games that cannot serve a file.
    pub const UNAVAILABLE: &str = "unavailable";
    /// Rows or files that are no longer needed.
    pub const ORPHAN: &str = "orphan";
    /// Possible duplicate games or archives.
    pub const DUPLICATE: &str = "duplicate";
    /// Incomplete metadata.
    pub const METADATA: &str = "metadata";
    /// Integrity and manifest findings.
    pub const INTEGRITY: &str = "integrity";
    /// Index/disk count differences.
    pub const COUNTS: &str = "counts";
}

/// Safe action identifiers returned with findings.
pub mod action {
    /// Scan the finding's root (or every root).
    pub const RESCAN_ROOT: &str = "rescan_root";
    /// Refresh metadata for the finding's game.
    pub const REFRESH_METADATA: &str = "refresh_metadata";
    /// Mark a version row missing after re-checking the file (scanner behaviour).
    pub const MARK_MISSING: &str = "mark_missing";
    /// Delete a version row; never the archive file.
    pub const REMOVE_VERSION: &str = "remove_version";
    /// Delete a game row that has no versions and no user data.
    pub const REMOVE_ORPHAN_GAME: &str = "remove_orphan_game";
    /// Compute the media GC plan without deleting anything.
    pub const MEDIA_GC_DRY_RUN: &str = "media_gc_dry_run";
    /// Run the media GC, deleting only unreferenced artwork past its grace period.
    pub const MEDIA_GC_APPLY: &str = "media_gc_apply";
    /// Client-side: open the related game or finding for a human decision.
    pub const REVIEW: &str = "review";
}

/// Every action the HTTP layer accepts.
pub const ACTIONS: &[&str] = &[
    action::RESCAN_ROOT,
    action::REFRESH_METADATA,
    action::MARK_MISSING,
    action::REMOVE_VERSION,
    action::REMOVE_ORPHAN_GAME,
    action::MEDIA_GC_DRY_RUN,
    action::MEDIA_GC_APPLY,
];

/// Actions that delete a file or a database row.
pub const DESTRUCTIVE_ACTIONS: &[&str] = &[
    action::REMOVE_VERSION,
    action::REMOVE_ORPHAN_GAME,
    action::MEDIA_GC_APPLY,
];

/// Counts by severity.
#[derive(Debug, Clone, Copy, Default, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LibraryHealthCounts {
    /// Findings that need attention.
    pub error: i64,
    /// Degraded or pending findings.
    pub warning: i64,
    /// Informational findings.
    pub info: i64,
    /// Total findings in the report.
    pub total: i64,
}

/// Count of findings per category.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LibraryHealthCategoryCount {
    /// Category id (see the module `category` constants).
    pub category: String,
    /// Findings in that category.
    pub count: i64,
}

/// Cached report summary.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LibraryHealthReport {
    /// Report id.
    pub id: i64,
    /// `running`, `completed` or `failed`.
    pub status: String,
    /// `manual`, `scheduled` or `action`.
    pub trigger: String,
    /// Progress percentage, 0–100.
    pub progress: i64,
    /// Rows and files considered in this run.
    pub total: i64,
    /// Number of findings recorded.
    pub findings_count: i64,
    /// Counts by severity.
    pub counts: LibraryHealthCounts,
    /// Counts by category.
    pub by_category: Vec<LibraryHealthCategoryCount>,
    /// Fatal error, when the run aborted.
    pub error: Option<String>,
    /// Start timestamp.
    pub started_at: DateTime<Utc>,
    /// End timestamp.
    pub finished_at: Option<DateTime<Utc>>,
}

/// One typed finding.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LibraryHealthFinding {
    /// Finding id.
    pub id: i64,
    /// Report this finding belongs to.
    pub report_id: i64,
    /// Category id.
    pub category: String,
    /// Specific kind (`missing_archive`, `duplicate_game`, …).
    pub kind: String,
    /// `error`, `warning` or `info`.
    pub severity: String,
    /// Related game, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_id: Option<i64>,
    /// Related version, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_id: Option<i64>,
    /// Related root, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_id: Option<i64>,
    /// Absolute path or object key, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Short human-readable title.
    pub title: String,
    /// Plain-language explanation.
    pub detail: String,
    /// Structured evidence for the UI.
    pub evidence: serde_json::Value,
    /// Safe action identifiers.
    pub actions: Vec<String>,
    /// When the finding was recorded.
    pub created_at: DateTime<Utc>,
}

/// `GET /api/admin/health/library` response.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LibraryHealthResponse {
    /// Latest cached report, if one has ever run.
    pub report: Option<LibraryHealthReport>,
    /// Findings matching the request filters.
    pub findings: Vec<LibraryHealthFinding>,
    /// Total findings matching the filters, before pagination.
    pub findings_total: i64,
}

/// `POST /api/admin/health/library/scan` response.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LibraryHealthStartResponse {
    /// Report id to poll with `GET /api/admin/health/library`.
    pub report_id: i64,
    /// Always `running` at creation time.
    pub status: String,
}

/// `POST /api/admin/health/library/actions` request.
#[derive(Debug, Clone, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LibraryHealthActionRequest {
    /// One of [`ACTIONS`].
    pub action: String,
    /// Findings to act on. Empty for the global media GC actions.
    #[serde(default)]
    pub finding_ids: Vec<i64>,
    /// Required for destructive actions.
    #[serde(default)]
    pub confirm: bool,
}

/// Media garbage collection summary returned by an action.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LibraryHealthMediaGc {
    /// Whether files were actually removed.
    pub dry_run: bool,
    /// Files present in the media directory.
    pub scanned: usize,
    /// Files referenced by the database.
    pub referenced: usize,
    /// Unreferenced files old enough to be collected.
    pub orphan_files: usize,
    /// Bytes that are (or would be) collected.
    pub orphan_bytes: u64,
    /// Files removed (zero on a dry run).
    pub deleted_files: usize,
    /// Bytes removed (zero on a dry run).
    pub deleted_bytes: u64,
    /// Up to 100 candidate names.
    pub candidates: Vec<String>,
}

/// `POST /api/admin/health/library/actions` response.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LibraryHealthActionResult {
    /// Action that ran.
    pub action: String,
    /// Findings or global operations that succeeded.
    pub applied: usize,
    /// Findings skipped because they did not match or were unsafe.
    pub skipped: usize,
    /// Human-readable reasons for skipped findings.
    pub errors: Vec<String>,
    /// Media GC result, when the action was a GC action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_gc: Option<LibraryHealthMediaGc>,
    /// Whether a fresh report was queued after the action.
    pub report_queued: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ReportRow {
    id: i64,
    status: String,
    trigger: String,
    progress: i64,
    total: i64,
    findings_count: i64,
    errors_count: i64,
    warnings_count: i64,
    info_count: i64,
    error: Option<String>,
    started_at: String,
    finished_at: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct FindingRow {
    id: i64,
    report_id: i64,
    category: String,
    kind: String,
    severity: String,
    game_id: Option<i64>,
    version_id: Option<i64>,
    root_id: Option<i64>,
    path: Option<String>,
    title: String,
    detail: String,
    evidence_json: String,
    actions_json: String,
    created_at: String,
}

impl FindingRow {
    fn into_view(self) -> LibraryHealthFinding {
        LibraryHealthFinding {
            id: self.id,
            report_id: self.report_id,
            category: self.category,
            kind: self.kind,
            severity: self.severity,
            game_id: self.game_id,
            version_id: self.version_id,
            root_id: self.root_id,
            path: self.path,
            title: self.title,
            detail: self.detail,
            evidence: serde_json::from_str(&self.evidence_json)
                .unwrap_or_else(|_| serde_json::json!({})),
            actions: db::parse_json_list(&self.actions_json),
            created_at: db::parse_time(&self.created_at).unwrap_or_else(Utc::now),
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct GameRow {
    id: i64,
    title: String,
    release_date: Option<String>,
    provider_slug: Option<String>,
    provider_id: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct RootRow {
    id: i64,
    label: String,
    path: String,
    enabled: i64,
    available: i64,
    last_error: Option<String>,
    kind: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct VersionHealthRow {
    id: i64,
    game_id: i64,
    root_id: Option<i64>,
    file_path: String,
    size: i64,
    missing: i64,
    root_unavailable: i64,
    storage_backend: Option<String>,
    object_key: Option<String>,
    title: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct GameAvailabilityRow {
    id: i64,
    title: String,
    versions: i64,
    available: i64,
    progress_rows: i64,
    bookmark_rows: i64,
    save_rows: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ArchiveHashRow {
    id: i64,
    game_id: i64,
    file_path: String,
    content_hash: Option<String>,
    archive_hash: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct IntegrityHealthRow {
    kind: String,
    target_type: String,
    target_id: Option<String>,
    path: Option<String>,
    detail: Option<String>,
    report_id: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ActionFindingRow {
    game_id: Option<i64>,
    version_id: Option<i64>,
    root_id: Option<i64>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ActionVersionRow {
    missing: i64,
    root_id: Option<i64>,
    file_path: String,
    storage_backend: Option<String>,
}

/// New finding built by one check phase.
#[derive(Debug, Clone)]
struct NewFinding {
    category: &'static str,
    kind: &'static str,
    severity: &'static str,
    game_id: Option<i64>,
    version_id: Option<i64>,
    root_id: Option<i64>,
    path: Option<String>,
    title: String,
    detail: String,
    evidence: serde_json::Value,
    actions: Vec<&'static str>,
}

impl NewFinding {
    fn new(
        category: &'static str,
        kind: &'static str,
        severity: &'static str,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            category,
            kind,
            severity,
            game_id: None,
            version_id: None,
            root_id: None,
            path: None,
            title: title.into(),
            detail: detail.into(),
            evidence: serde_json::json!({}),
            actions: Vec::new(),
        }
    }

    fn game(mut self, game_id: i64) -> Self {
        self.game_id = Some(game_id);
        self
    }

    fn version(mut self, version_id: i64) -> Self {
        self.version_id = Some(version_id);
        self
    }

    fn root(mut self, root_id: i64) -> Self {
        self.root_id = Some(root_id);
        self
    }

    fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    fn evidence(mut self, evidence: serde_json::Value) -> Self {
        self.evidence = evidence;
        self
    }

    fn actions(mut self, actions: &[&'static str]) -> Self {
        self.actions = actions.to_vec();
        self
    }
}

/// Scheduled interval, or `None` when disabled.
#[must_use]
pub fn interval(config: &crate::config::Config) -> Option<Duration> {
    (!config.library_health_interval.is_zero()).then_some(config.library_health_interval)
}

/// Starts a report in the background.
///
/// Only one report can run at a time per process; starting a second one
/// returns 409 `health_running`.
pub async fn start(state: &AppState, trigger: HealthTrigger) -> Result<i64, ApiError> {
    if state
        .library_health_busy
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_err()
    {
        return Err(ApiError::conflict(
            "health_running",
            "A library health report is already running",
        ));
    }
    let report_id = match insert_report(&state.pool, trigger).await {
        Ok(id) => id,
        Err(error) => {
            state
                .library_health_busy
                .store(false, std::sync::atomic::Ordering::SeqCst);
            return Err(ApiError::from(error));
        }
    };
    let state = state.clone();
    tokio::spawn(async move {
        run_report(&state, report_id).await;
        state
            .library_health_busy
            .store(false, std::sync::atomic::Ordering::SeqCst);
    });
    Ok(report_id)
}

async fn insert_report(pool: &Database, trigger: HealthTrigger) -> Result<i64> {
    db::query(
        "INSERT INTO library_health_reports (status, trigger, progress, started_at)
         VALUES ('running', ?, 0, ?)",
    )
    .bind(trigger.as_str())
    .bind(db::db_time(db::now()))
    .returning_id(pool)
    .await
    .context("insert library health report")
}

/// Runs one report. Public so tests can exercise it without the task spawner.
pub async fn run_report(state: &AppState, report_id: i64) {
    let failure = generate(state, report_id).await.err();
    let (status, error) = match failure {
        Some(error) => ("failed", Some(format!("{error:#}"))),
        None => ("completed", None),
    };
    if let Err(error) = finish_report(&state.pool, report_id, status, error.as_deref()).await {
        tracing::error!(%error, report_id, "could not finish library health report");
    }
    tracing::info!(report_id, status, error = ?error, "library health report finished");
}

async fn generate(state: &AppState, report_id: i64) -> Result<()> {
    let games = count(&state.pool, "SELECT COUNT(*) FROM games").await?;
    let versions = count(&state.pool, "SELECT COUNT(*) FROM game_versions").await?;
    let roots = count(&state.pool, "SELECT COUNT(*) FROM library_roots").await?;
    let total = games.saturating_add(versions).saturating_add(roots);
    update_progress(&state.pool, report_id, 2, total).await?;

    let root_findings = check_roots(state).await?;
    store_phase(state, report_id, 10, &root_findings).await?;

    let archive_findings = check_archives(state).await?;
    store_phase(state, report_id, 38, &archive_findings).await?;

    let game_findings = check_games(state).await?;
    store_phase(state, report_id, 50, &game_findings).await?;

    let media_findings = check_media(state).await?;
    store_phase(state, report_id, 62, &media_findings).await?;

    let duplicate_findings = check_duplicates(state).await?;
    store_phase(state, report_id, 72, &duplicate_findings).await?;

    let metadata_findings = check_metadata(state).await?;
    store_phase(state, report_id, 85, &metadata_findings).await?;

    let count_findings = check_counts(state).await?;
    store_phase(state, report_id, 92, &count_findings).await?;

    let mut seen_paths = HashSet::new();
    for finding in archive_findings
        .iter()
        .chain(media_findings.iter())
        .chain(count_findings.iter())
    {
        if let Some(path) = &finding.path {
            seen_paths.insert(path.clone());
        }
    }
    let integrity_findings = check_integrity(state, &seen_paths).await?;
    store_phase(state, report_id, 98, &integrity_findings).await?;

    Ok(())
}

async fn count(pool: &Database, sql: &str) -> Result<i64> {
    db::query_scalar::<i64>(sql)
        .fetch_one(pool)
        .await
        .context("count library health rows")
}

async fn update_progress(pool: &Database, report_id: i64, progress: i64, total: i64) -> Result<()> {
    db::query("UPDATE library_health_reports SET progress = ?, total = ? WHERE id = ?")
        .bind(progress)
        .bind(total)
        .bind(report_id)
        .execute(pool)
        .await
        .context("update library health progress")?;
    Ok(())
}

async fn store_phase(
    state: &AppState,
    report_id: i64,
    progress: i64,
    findings: &[NewFinding],
) -> Result<()> {
    if !findings.is_empty() {
        insert_findings(&state.pool, report_id, findings).await?;
    }
    db::query(
        "UPDATE library_health_reports
         SET progress = ?, findings_count = findings_count + ? WHERE id = ?",
    )
    .bind(progress)
    .bind(i64::try_from(findings.len()).unwrap_or(i64::MAX))
    .bind(report_id)
    .execute(&state.pool)
    .await
    .context("update library health phase")?;
    Ok(())
}

async fn insert_findings(pool: &Database, report_id: i64, findings: &[NewFinding]) -> Result<()> {
    let now = db::db_time(db::now());
    let mut transaction = pool.begin().await.context("begin health findings")?;
    for finding in findings {
        let evidence =
            serde_json::to_string(&finding.evidence).unwrap_or_else(|_| "{}".to_string());
        let actions = serde_json::to_string(&finding.actions).unwrap_or_else(|_| "[]".to_string());
        db::query(
            "INSERT INTO library_health_findings
                (report_id, category, kind, severity, game_id, version_id, root_id, path,
                 title, detail, evidence_json, actions_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(report_id)
        .bind(finding.category)
        .bind(finding.kind)
        .bind(finding.severity)
        .bind(finding.game_id)
        .bind(finding.version_id)
        .bind(finding.root_id)
        .bind(finding.path.as_deref())
        .bind(&finding.title)
        .bind(&finding.detail)
        .bind(evidence)
        .bind(actions)
        .bind(&now)
        .execute(&mut transaction)
        .await
        .context("insert library health finding")?;
    }
    transaction
        .commit()
        .await
        .context("commit health findings")?;
    Ok(())
}

async fn finish_report(
    pool: &Database,
    report_id: i64,
    status: &str,
    error: Option<&str>,
) -> Result<()> {
    let now = db::db_time(db::now());
    db::query(
        "UPDATE library_health_reports SET
            status = ?,
            progress = CASE WHEN ? = 'completed' THEN 100 ELSE progress END,
            findings_count = (SELECT COUNT(*) FROM library_health_findings WHERE report_id = ?),
            errors_count = (SELECT COUNT(*) FROM library_health_findings
                WHERE report_id = ? AND severity = 'error'),
            warnings_count = (SELECT COUNT(*) FROM library_health_findings
                WHERE report_id = ? AND severity = 'warning'),
            info_count = (SELECT COUNT(*) FROM library_health_findings
                WHERE report_id = ? AND severity = 'info'),
            error = ?,
            finished_at = ?
         WHERE id = ?",
    )
    .bind(status)
    .bind(status)
    .bind(report_id)
    .bind(report_id)
    .bind(report_id)
    .bind(report_id)
    .bind(error)
    .bind(&now)
    .bind(report_id)
    .execute(pool)
    .await
    .context("finish library health report")?;
    let _ = db::query(
        "DELETE FROM library_health_reports WHERE id NOT IN
            (SELECT id FROM library_health_reports ORDER BY id DESC LIMIT ?)",
    )
    .bind(KEEP_REPORTS)
    .execute(pool)
    .await;
    Ok(())
}

async fn check_roots(state: &AppState) -> Result<Vec<NewFinding>> {
    let roots = db::query_as::<RootRow>(
        "SELECT id, label, path, enabled, available, last_error, kind
         FROM library_roots ORDER BY position, id",
    )
    .fetch_all(&state.pool)
    .await
    .context("list roots for health")?;
    let mut findings = Vec::new();
    for root in roots {
        if root.enabled == 0 {
            continue;
        }
        let exists = tokio::fs::metadata(Path::new(&root.path))
            .await
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false);
        if exists {
            continue;
        }
        let versions =
            db::query_scalar::<i64>("SELECT COUNT(*) FROM game_versions WHERE root_id = ?")
                .bind(root.id)
                .fetch_one(&state.pool)
                .await
                .context("count root versions")?;
        let detail = match &root.last_error {
            Some(error) if !error.trim().is_empty() => {
                format!("The folder {} is not readable: {error}", root.path)
            }
            _ => format!(
                "The folder {} is missing or not mounted. {versions} indexed file(s) cannot be served until it is back.",
                root.path
            ),
        };
        findings.push(
            NewFinding::new(
                category::UNAVAILABLE,
                "root_unavailable",
                severity::ERROR,
                format!("Library root “{}” is unavailable", root.label),
                detail,
            )
            .root(root.id)
            .path(root.path.clone())
            .evidence(serde_json::json!({
                "rootId": root.id,
                "label": root.label,
                "path": root.path,
                "versions": versions,
                "available": root.available != 0,
                "lastError": root.last_error,
                "kind": root.kind,
            }))
            .actions(&[action::RESCAN_ROOT]),
        );
    }
    Ok(findings)
}

async fn check_archives(state: &AppState) -> Result<Vec<NewFinding>> {
    let rows = db::query_as::<VersionHealthRow>(
        "SELECT v.id, v.game_id, v.root_id, v.file_path, v.size, v.missing,
                v.root_unavailable, v.storage_backend, v.object_key, g.title
         FROM game_versions v
         JOIN games g ON g.id = v.game_id
         ORDER BY v.id",
    )
    .fetch_all(&state.pool)
    .await
    .context("list versions for health")?;
    let mut findings = Vec::new();
    for row in rows {
        if row.root_unavailable != 0 {
            // The root-level finding already explains why every file is
            // unreachable; a per-version finding would only add noise.
            continue;
        }
        let placeholder = crate::roots::resolve_version_path(
            &state.pool,
            &state.config,
            row.root_id,
            &row.file_path,
        )
        .await
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| row.file_path.clone());
        if row.missing != 0 {
            findings.push(missing_archive(
                &row,
                &placeholder,
                "The archive file is gone from disk. The database row and any save data were kept.",
            ));
            continue;
        }
        let remote = row.storage_backend.as_deref() == Some("s3") && state.storage.is_remote();
        if remote {
            let Some(key) = row.object_key.as_deref().filter(|key| !key.is_empty()) else {
                findings.push(
                    NewFinding::new(
                        category::MISSING,
                        "missing_archive",
                        severity::ERROR,
                        format!("{} has no S3 object key", row.title),
                        "This version is marked as stored in object storage but no object key was recorded.",
                    )
                    .game(row.game_id)
                    .version(row.id)
                    .path(&row.file_path)
                    .evidence(serde_json::json!({
                        "filePath": row.file_path,
                        "storageBackend": "s3",
                        "size": row.size,
                    }))
                    .actions(&[action::RESCAN_ROOT, action::REVIEW]),
                );
                continue;
            };
            match state.storage.head(key).await {
                Ok(0) => findings.push(empty_archive(&row, key)),
                Ok(size) => {
                    if row.size > 0 && size != u64::try_from(row.size).unwrap_or(0) {
                        findings.push(size_mismatch(&row, key, size));
                    }
                }
                Err(crate::storage::StorageError::NotFound) => findings.push(missing_archive(
                    &row,
                    key,
                    "The object was not found in the bucket.",
                )),
                Err(error) => findings.push(
                    NewFinding::new(
                        category::INTEGRITY,
                        "storage_unreachable",
                        severity::WARNING,
                        format!("Object storage could not be checked for {}", row.title),
                        format!("The bucket did not answer for this object: {error}"),
                    )
                    .game(row.game_id)
                    .version(row.id)
                    .path(key)
                    .evidence(serde_json::json!({
                        "filePath": row.file_path,
                        "objectKey": key,
                        "storageBackend": "s3",
                    }))
                    .actions(&[action::REVIEW]),
                ),
            }
            continue;
        }

        let path = crate::roots::resolve_version_path(
            &state.pool,
            &state.config,
            row.root_id,
            &row.file_path,
        )
        .await;
        let Some(path) = path else {
            findings.push(
                NewFinding::new(
                    category::INTEGRITY,
                    "unsafe_path",
                    severity::ERROR,
                    format!("{} points outside its library root", row.title),
                    "The indexed path contains a traversal or a root that no longer resolves. The scanner will re-index the file.",
                )
                .game(row.game_id)
                .version(row.id)
                .root_opt(row.root_id)
                .path(&row.file_path)
                .evidence(serde_json::json!({ "filePath": row.file_path }))
                .actions(&[action::RESCAN_ROOT]),
            );
            continue;
        };
        let absolute = path.display().to_string();
        let metadata = tokio::fs::metadata(&path).await;
        match metadata {
            Ok(metadata) if metadata.is_file() => {
                if metadata.len() == 0 {
                    findings.push(empty_archive(&row, &absolute));
                } else if row.size > 0 && metadata.len() != u64::try_from(row.size).unwrap_or(0) {
                    findings.push(size_mismatch(&row, &absolute, metadata.len()));
                }
            }
            Ok(_) => findings.push(missing_archive(
                &row,
                &absolute,
                "The indexed path is not a regular file.",
            )),
            Err(_) => findings.push(missing_archive(
                &row,
                &absolute,
                "The indexed file no longer exists. A rescan will mark it missing.",
            )),
        }
    }
    Ok(findings)
}

fn missing_archive(row: &VersionHealthRow, path: &str, detail: &str) -> NewFinding {
    let remote = row.storage_backend.as_deref() == Some("s3");
    let detail = if remote {
        format!("{detail} The archive may still exist in object storage; run an integrity check or rescan before removing anything.")
    } else {
        detail.to_string()
    };
    let actions: &[&'static str] = if remote {
        &[action::RESCAN_ROOT, action::REVIEW]
    } else {
        &[
            action::RESCAN_ROOT,
            action::MARK_MISSING,
            action::REMOVE_VERSION,
        ]
    };
    NewFinding::new(
        category::MISSING,
        "missing_archive",
        severity::ERROR,
        format!("{} has a missing archive", row.title),
        detail,
    )
    .game(row.game_id)
    .version(row.id)
    .root_opt(row.root_id)
    .path(path)
    .evidence(serde_json::json!({
        "filePath": row.file_path,
        "size": row.size,
        "missing": row.missing != 0,
        "storageBackend": row.storage_backend.clone().unwrap_or_else(|| "local".to_string()),
    }))
    .actions(actions)
}

fn empty_archive(row: &VersionHealthRow, path: &str) -> NewFinding {
    NewFinding::new(
        category::INTEGRITY,
        "empty_archive",
        severity::WARNING,
        format!("{} has an empty archive", row.title),
        "The file is zero bytes. It cannot be installed or streamed.",
    )
    .game(row.game_id)
    .version(row.id)
    .root_opt(row.root_id)
    .path(path)
    .evidence(serde_json::json!({
        "filePath": row.file_path,
        "size": 0,
    }))
    .actions(&[action::RESCAN_ROOT, action::REVIEW])
}

fn size_mismatch(row: &VersionHealthRow, path: &str, actual: u64) -> NewFinding {
    NewFinding::new(
        category::INTEGRITY,
        "size_mismatch",
        severity::WARNING,
        format!("{} changed on disk", row.title),
        format!(
            "The index recorded {} bytes but the file now has {actual} bytes. A rescan refreshes the row.",
            row.size
        ),
    )
    .game(row.game_id)
    .version(row.id)
    .root_opt(row.root_id)
    .path(path)
    .evidence(serde_json::json!({
        "filePath": row.file_path,
        "expectedSize": row.size,
        "actualSize": actual,
    }))
    .actions(&[action::RESCAN_ROOT, action::REVIEW])
}

impl NewFinding {
    fn root_opt(mut self, root_id: Option<i64>) -> Self {
        self.root_id = root_id;
        self
    }
}

async fn check_games(state: &AppState) -> Result<Vec<NewFinding>> {
    let rows = db::query_as::<GameAvailabilityRow>(
        "SELECT g.id, g.title,
                COUNT(v.id) AS versions,
                COALESCE(SUM(CASE WHEN v.missing = 0 AND v.root_unavailable = 0
                    THEN 1 ELSE 0 END), 0) AS available,
                (SELECT COUNT(*) FROM progress p WHERE p.game_id = g.id) AS progress_rows,
                (SELECT COUNT(*) FROM bookmarks b WHERE b.game_id = g.id) AS bookmark_rows,
                (SELECT COUNT(*) FROM save_snapshots s WHERE s.game_id = g.id) AS save_rows
         FROM games g
         LEFT JOIN game_versions v ON v.game_id = g.id
         GROUP BY g.id, g.title
         ORDER BY g.id",
    )
    .fetch_all(&state.pool)
    .await
    .context("list game availability")?;
    let mut findings = Vec::new();
    for row in rows {
        if row.versions == 0 {
            let user_data = row.progress_rows + row.bookmark_rows + row.save_rows;
            let detail = if user_data == 0 {
                "This game has no library file and no progress, bookmark or save data. Removing the row is safe."
            } else {
                "This game has no library file but still has user data; review it before removing anything."
            };
            let mut finding = NewFinding::new(
                category::ORPHAN,
                "orphan_game",
                if user_data == 0 {
                    severity::INFO
                } else {
                    severity::WARNING
                },
                format!("{} has no library file", row.title),
                detail,
            )
            .game(row.id)
            .evidence(serde_json::json!({
                "versions": 0,
                "progressRows": row.progress_rows,
                "bookmarkRows": row.bookmark_rows,
                "saveRows": row.save_rows,
            }));
            finding = if user_data == 0 {
                finding.actions(&[action::REMOVE_ORPHAN_GAME, action::REVIEW])
            } else {
                finding.actions(&[action::REVIEW])
            };
            findings.push(finding);
        } else if row.available == 0 {
            findings.push(
                NewFinding::new(
                    category::UNAVAILABLE,
                    "game_without_version",
                    severity::ERROR,
                    format!("{} has no available file", row.title),
                    format!(
                        "All {} indexed version(s) are missing or on an unavailable root, so this game cannot be downloaded or streamed.",
                        row.versions
                    ),
                )
                .game(row.id)
                .evidence(serde_json::json!({
                    "versions": row.versions,
                    "available": 0,
                }))
                .actions(&[action::RESCAN_ROOT, action::REVIEW]),
            );
        }
    }
    Ok(findings)
}

async fn check_media(state: &AppState) -> Result<Vec<NewFinding>> {
    let referenced = crate::integrity::referenced_media(&state.pool)
        .await
        .context("list referenced media")?;
    let mut names: Vec<&String> = referenced.iter().collect();
    names.sort();
    let mut findings = Vec::new();
    for name in names {
        if !safe_media_name(name) {
            findings.push(
                NewFinding::new(
                    category::INTEGRITY,
                    "unsafe_media_name",
                    severity::WARNING,
                    format!("Artwork reference “{name}” is invalid"),
                    "The database references a media name that cannot be served safely. Re-upload or clear the artwork.",
                )
                .evidence(serde_json::json!({ "name": name }))
                .actions(&[action::REVIEW]),
            );
            continue;
        }
        let path = state.config.media_dir.join(name);
        let exists = tokio::fs::metadata(&path)
            .await
            .map(|metadata| metadata.is_file())
            .unwrap_or(false);
        if !exists {
            findings.push(
                NewFinding::new(
                    category::MISSING,
                    "media_missing",
                    severity::WARNING,
                    "A referenced cover or screenshot is missing",
                    format!(
                        "The database references {name}, but the file is not in the media folder."
                    ),
                )
                .path(path.display().to_string())
                .evidence(serde_json::json!({ "name": name }))
                .actions(&[action::MEDIA_GC_DRY_RUN, action::REVIEW]),
            );
        }
    }

    if let Ok(mut entries) = tokio::fs::read_dir(&state.config.media_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if !safe_media_name(&name) || referenced.contains(&name) {
                continue;
            }
            let Ok(metadata) = entry.metadata().await else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            findings.push(
                NewFinding::new(
                    category::ORPHAN,
                    "media_orphan",
                    severity::INFO,
                    "An unreferenced artwork file can be cleaned up",
                    format!(
                        "{name} is not referenced by any game or account and is {} bytes.",
                        metadata.len()
                    ),
                )
                .path(entry.path().display().to_string())
                .evidence(serde_json::json!({
                    "name": name,
                    "size": metadata.len(),
                }))
                .actions(&[action::MEDIA_GC_DRY_RUN, action::MEDIA_GC_APPLY]),
            );
        }
    }
    Ok(findings)
}

async fn check_duplicates(state: &AppState) -> Result<Vec<NewFinding>> {
    let games = db::query_as::<GameRow>(
        "SELECT id, title, release_date, provider_slug, provider_id FROM games ORDER BY id",
    )
    .fetch_all(&state.pool)
    .await
    .context("list games for duplicates")?;
    let titles: HashMap<i64, String> = games
        .iter()
        .map(|game| (game.id, game.title.clone()))
        .collect();
    let mut findings = Vec::new();
    let mut emitted: HashSet<Vec<i64>> = HashSet::new();

    let mut title_groups: HashMap<(String, Option<String>), Vec<&GameRow>> = HashMap::new();
    let mut provider_groups: HashMap<(String, String), Vec<&GameRow>> = HashMap::new();
    for game in &games {
        let key = (
            normalized_title(&game.title),
            release_year(&game.release_date),
        );
        title_groups.entry(key).or_default().push(game);
        if let (Some(slug), Some(provider_id)) = (&game.provider_slug, &game.provider_id) {
            if !slug.trim().is_empty() && !provider_id.trim().is_empty() {
                provider_groups
                    .entry((slug.clone(), provider_id.clone()))
                    .or_default()
                    .push(game);
            }
        }
    }
    for ((title, year), group) in title_groups {
        if group.len() < 2 {
            continue;
        }
        let mut ids: Vec<i64> = group.iter().map(|game| game.id).collect();
        ids.sort_unstable();
        if !emitted.insert(ids.clone()) {
            continue;
        }
        findings.push(
            NewFinding::new(
                category::DUPLICATE,
                "duplicate_game",
                severity::WARNING,
                format!("Possible duplicate games: {title}"),
                format!(
                    "{} games share the same title{}.",
                    group.len(),
                    year.as_ref()
                        .map_or_else(String::new, |year| format!(" and {year} release year"))
                ),
            )
            .game(group[0].id)
            .evidence(serde_json::json!({
                "gameIds": ids,
                "titles": group.iter().map(|game| game.title.clone()).collect::<Vec<_>>(),
                "year": year,
            }))
            .actions(&[action::REVIEW]),
        );
    }
    for ((slug, provider_id), group) in provider_groups {
        if group.len() < 2 {
            continue;
        }
        let mut ids: Vec<i64> = group.iter().map(|game| game.id).collect();
        ids.sort_unstable();
        if !emitted.insert(ids.clone()) {
            continue;
        }
        findings.push(
            NewFinding::new(
                category::DUPLICATE,
                "duplicate_game",
                severity::WARNING,
                format!("Several games claim the same provider record ({slug})"),
                format!(
                    "{} games point at provider id {provider_id}. One of them is probably a duplicate.",
                    group.len()
                ),
            )
            .game(group[0].id)
            .evidence(serde_json::json!({
                "gameIds": ids,
                "providerSlug": slug,
                "providerId": provider_id,
                "titles": group.iter().map(|game| game.title.clone()).collect::<Vec<_>>(),
            }))
            .actions(&[action::REVIEW]),
        );
    }

    let versions = db::query_as::<ArchiveHashRow>(
        "SELECT id, game_id, file_path, content_hash, archive_hash
         FROM game_versions
         WHERE missing = 0 AND (content_hash IS NOT NULL OR archive_hash IS NOT NULL)
         ORDER BY id",
    )
    .fetch_all(&state.pool)
    .await
    .context("list archive hashes")?;
    let mut hashes: HashMap<String, Vec<&ArchiveHashRow>> = HashMap::new();
    for version in &versions {
        if let Some(hash) = version
            .content_hash
            .as_deref()
            .filter(|hash| !hash.is_empty())
            .or_else(|| {
                version
                    .archive_hash
                    .as_deref()
                    .filter(|hash| !hash.is_empty())
            })
        {
            hashes.entry(hash.to_string()).or_default().push(version);
        }
    }
    for (hash, group) in hashes {
        if group.len() < 2 {
            continue;
        }
        let version_ids: Vec<i64> = group.iter().map(|version| version.id).collect();
        let game_ids: Vec<i64> = {
            let mut ids: Vec<i64> = group.iter().map(|version| version.game_id).collect();
            ids.sort_unstable();
            ids.dedup();
            ids
        };
        let mut titles: Vec<String> = game_ids
            .iter()
            .filter_map(|id| titles.get(id).cloned())
            .collect();
        titles.sort();
        titles.dedup();
        findings.push(
            NewFinding::new(
                category::DUPLICATE,
                "duplicate_archive",
                severity::WARNING,
                "Several indexed archives are byte-identical",
                format!(
                    "{} versions share the same content hash ({}). One copy is usually enough.",
                    group.len(),
                    &hash[..hash.len().min(16)]
                ),
            )
            .game(game_ids.first().copied().unwrap_or_default())
            .evidence(serde_json::json!({
                "hash": hash,
                "versionIds": version_ids,
                "gameIds": game_ids,
                "titles": titles,
                "paths": group.iter().map(|version| version.file_path.clone()).collect::<Vec<_>>(),
            }))
            .actions(&[action::REVIEW]),
        );
    }
    Ok(findings)
}

async fn check_metadata(state: &AppState) -> Result<Vec<NewFinding>> {
    let games = db::query_as::<(
        i64,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )>(
        "SELECT id, title, cover_path, description, release_date, provider_slug
         FROM games ORDER BY id",
    )
    .fetch_all(&state.pool)
    .await
    .context("list games for metadata")?;
    let mut findings = Vec::new();
    for (id, title, cover, description, release_date, provider_slug) in games {
        let mut missing: Vec<&str> = Vec::new();
        if cover.as_deref().is_none_or(|cover| cover.trim().is_empty()) {
            missing.push("cover");
        }
        if description
            .as_deref()
            .is_none_or(|description| description.trim().is_empty())
        {
            missing.push("description");
        }
        if release_date
            .as_deref()
            .is_none_or(|date| date.trim().is_empty())
        {
            missing.push("releaseDate");
        }
        if provider_slug
            .as_deref()
            .is_none_or(|slug| slug.trim().is_empty())
        {
            missing.push("provider");
        }
        if missing.is_empty() {
            continue;
        }
        findings.push(
            NewFinding::new(
                category::METADATA,
                "metadata_gap",
                severity::INFO,
                format!("{title} has incomplete metadata"),
                format!("Missing: {}.", missing.join(", ")),
            )
            .game(id)
            .evidence(serde_json::json!({ "missingFields": missing }))
            .actions(&[action::REFRESH_METADATA, action::REVIEW]),
        );
    }
    Ok(findings)
}

async fn check_counts(state: &AppState) -> Result<Vec<NewFinding>> {
    let roots = db::query_as::<RootRow>(
        "SELECT id, label, path, enabled, available, last_error, kind
         FROM library_roots WHERE enabled = 1 ORDER BY position, id",
    )
    .fetch_all(&state.pool)
    .await
    .context("list roots for counts")?;
    let mut findings = Vec::new();
    for root in roots {
        let path = std::path::PathBuf::from(&root.path);
        let kind = root.kind.clone();
        let on_disk = tokio::task::spawn_blocking(move || {
            crate::scanner::count_indexable_files(&path, &kind)
        })
        .await
        .unwrap_or(0);
        if on_disk == 0 && root.available == 0 {
            continue;
        }
        let indexed = db::query_scalar::<i64>(
            "SELECT COUNT(*) FROM game_versions
             WHERE root_id = ? AND missing = 0 AND root_unavailable = 0",
        )
        .bind(root.id)
        .fetch_one(&state.pool)
        .await
        .context("count indexed root versions")?;
        let on_disk = i64::try_from(on_disk).unwrap_or(i64::MAX);
        if on_disk == indexed {
            continue;
        }
        let (severity, detail) = if indexed > on_disk {
            (
                severity::WARNING,
                format!(
                    "The index lists {indexed} file(s) but only {on_disk} matching file(s) exist on disk."
                ),
            )
        } else {
            (
                severity::INFO,
                format!(
                    "{on_disk} matching file(s) are on disk and {indexed} are indexed. A rescan indexes the new files (mirrored archives are indexed once)."
                ),
            )
        };
        findings.push(
            NewFinding::new(
                category::COUNTS,
                "count_mismatch",
                severity,
                format!("“{}” does not match the index", root.label),
                detail,
            )
            .root(root.id)
            .path(root.path.clone())
            .evidence(serde_json::json!({
                "rootId": root.id,
                "path": root.path,
                "indexed": indexed,
                "onDisk": on_disk,
            }))
            .actions(&[action::RESCAN_ROOT]),
        );
    }
    Ok(findings)
}

async fn check_integrity(
    state: &AppState,
    seen_paths: &HashSet<String>,
) -> Result<Vec<NewFinding>> {
    let rows = db::query_as::<IntegrityHealthRow>(
        "SELECT f.kind, f.target_type, f.target_id, f.path, f.detail, f.report_id
         FROM integrity_findings f
         WHERE f.report_id = (SELECT id FROM integrity_reports
             WHERE status = 'completed' ORDER BY id DESC LIMIT 1)
         ORDER BY f.id",
    )
    .fetch_all(&state.pool)
    .await
    .context("list integrity findings")?;
    let mut findings = Vec::new();
    let mut seen: HashSet<(String, Option<String>)> = HashSet::new();
    for row in rows {
        let path_key = row.path.clone();
        if path_key
            .as_ref()
            .is_some_and(|path| seen_paths.contains(path))
        {
            continue;
        }
        if !seen.insert((row.kind.clone(), row.path.clone())) {
            continue;
        }
        let (severity, title): (&str, String) = match row.kind.as_str() {
            "missing" => (
                severity::ERROR,
                format!("Integrity check: a {} is missing", row.target_type),
            ),
            "corrupt" => (
                severity::ERROR,
                format!("Integrity check: a {} failed verification", row.target_type),
            ),
            "size_mismatch" => (
                severity::WARNING,
                format!("Integrity check: a {} changed size", row.target_type),
            ),
            other => (
                severity::WARNING,
                format!("Integrity check reported “{other}”"),
            ),
        };
        let actions: &[&'static str] = if row.target_type == "media" {
            &[action::MEDIA_GC_DRY_RUN, action::REVIEW]
        } else {
            &[action::RESCAN_ROOT, action::REVIEW]
        };
        findings.push(
            NewFinding::new(
                category::INTEGRITY,
                "integrity_finding",
                severity,
                title,
                row.detail
                    .clone()
                    .unwrap_or_else(|| "See the integrity report for details.".to_string()),
            )
            .path_opt(row.path.clone())
            .evidence(serde_json::json!({
                "integrityReportId": row.report_id,
                "kind": row.kind,
                "targetType": row.target_type,
                "targetId": row.target_id,
                "path": row.path,
            }))
            .actions(actions),
        );
    }
    Ok(findings)
}

impl NewFinding {
    fn path_opt(mut self, path: Option<String>) -> Self {
        self.path = path;
        self
    }
}

fn normalized_title(title: &str) -> String {
    let mut output = String::new();
    let mut space = false;
    for character in title.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            output.push(character);
            space = false;
        } else if !space && !output.is_empty() {
            output.push(' ');
            space = true;
        }
    }
    output.trim().to_string()
}

fn release_year(date: &Option<String>) -> Option<String> {
    date.as_deref()
        .and_then(|date| date.get(0..4))
        .filter(|year| year.chars().all(|character| character.is_ascii_digit()))
        .map(str::to_string)
}

/// Reads the latest cached report and a filtered page of findings.
pub async fn read_latest(
    pool: &Database,
    severity_filter: Option<&str>,
    category_filter: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<LibraryHealthResponse> {
    let report = latest_report(pool).await?;
    let Some(report) = report else {
        return Ok(LibraryHealthResponse {
            report: None,
            findings: Vec::new(),
            findings_total: 0,
        });
    };
    let mut where_sql = String::from(" WHERE report_id = ?");
    if severity_filter.is_some() {
        where_sql.push_str(" AND severity = ?");
    }
    if category_filter.is_some() {
        where_sql.push_str(" AND category = ?");
    }
    let total_sql = format!("SELECT COUNT(*) FROM library_health_findings{where_sql}");
    let mut total_query = db::query_scalar::<i64>(&total_sql).bind(report.id);
    if let Some(severity) = severity_filter {
        total_query = total_query.bind(severity);
    }
    if let Some(category) = category_filter {
        total_query = total_query.bind(category);
    }
    let total = total_query
        .fetch_one(pool)
        .await
        .context("count library health findings")?;

    let list_sql = format!(
        "SELECT id, report_id, category, kind, severity, game_id, version_id, root_id, path,
                title, detail, evidence_json, actions_json, created_at
         FROM library_health_findings{where_sql}
         ORDER BY CASE severity WHEN 'error' THEN 0 WHEN 'warning' THEN 1 ELSE 2 END, id
         LIMIT ? OFFSET ?"
    );
    let mut list_query = db::query_as::<FindingRow>(&list_sql).bind(report.id);
    if let Some(severity) = severity_filter {
        list_query = list_query.bind(severity);
    }
    if let Some(category) = category_filter {
        list_query = list_query.bind(category);
    }
    let findings = list_query
        .bind(limit.clamp(1, 500))
        .bind(offset.max(0))
        .fetch_all(pool)
        .await
        .context("list library health findings")?
        .into_iter()
        .map(FindingRow::into_view)
        .collect();
    Ok(LibraryHealthResponse {
        report: Some(report),
        findings,
        findings_total: total,
    })
}

async fn latest_report(pool: &Database) -> Result<Option<LibraryHealthReport>> {
    let row = db::query_as::<ReportRow>(
        "SELECT id, status, trigger, progress, total, findings_count, errors_count,
                warnings_count, info_count, error, started_at, finished_at
         FROM library_health_reports ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .context("read latest library health report")?;
    let Some(row) = row else {
        return Ok(None);
    };
    let by_category = db::query_as::<(String, i64)>(
        "SELECT category, COUNT(*) FROM library_health_findings
         WHERE report_id = ? GROUP BY category ORDER BY COUNT(*) DESC, category",
    )
    .bind(row.id)
    .fetch_all(pool)
    .await
    .context("count library health categories")?
    .into_iter()
    .map(|(category, count)| LibraryHealthCategoryCount { category, count })
    .collect();
    Ok(Some(LibraryHealthReport {
        id: row.id,
        status: row.status,
        trigger: row.trigger,
        progress: row.progress,
        total: row.total,
        findings_count: row.findings_count,
        counts: LibraryHealthCounts {
            error: row.errors_count,
            warning: row.warnings_count,
            info: row.info_count,
            total: row.findings_count,
        },
        by_category,
        error: row.error,
        started_at: db::parse_time(&row.started_at).unwrap_or_else(Utc::now),
        finished_at: row.finished_at.as_deref().and_then(db::parse_time),
    }))
}

/// Applies one repair action to the selected findings.
///
/// The HTTP layer enforces the editor/administrator split and confirmation.
/// This function re-checks confirmation for destructive actions so a future
/// caller cannot bypass it.
pub async fn apply_action(
    state: &AppState,
    request: &LibraryHealthActionRequest,
) -> Result<LibraryHealthActionResult, ApiError> {
    let action = request.action.trim();
    if !ACTIONS.contains(&action) {
        return Err(ApiError::bad_request(
            "unknown_health_action",
            format!("Unknown health action `{action}`"),
        ));
    }
    let destructive = DESTRUCTIVE_ACTIONS.contains(&action);
    if destructive && !request.confirm {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "This action needs an explicit confirmation",
        ));
    }

    if action == self::action::MEDIA_GC_DRY_RUN || action == self::action::MEDIA_GC_APPLY {
        let dry_run = action == self::action::MEDIA_GC_DRY_RUN;
        let result = crate::media_gc::run(state, dry_run).await?;
        return Ok(LibraryHealthActionResult {
            action: action.to_string(),
            applied: 1,
            skipped: 0,
            errors: Vec::new(),
            media_gc: Some(LibraryHealthMediaGc {
                dry_run: result.dry_run,
                scanned: result.scanned,
                referenced: result.referenced,
                orphan_files: result.orphan_files,
                orphan_bytes: result.orphan_bytes,
                deleted_files: result.deleted_files,
                deleted_bytes: result.deleted_bytes,
                candidates: result.candidates,
            }),
            report_queued: !dry_run && start(state, HealthTrigger::Action).await.is_ok(),
        });
    }

    let findings = load_action_findings(state, &request.finding_ids).await?;
    if !request.finding_ids.is_empty() && findings.is_empty() {
        return Err(ApiError::not_found(
            "finding_not_found",
            "None of the selected findings belongs to the latest report",
        ));
    }

    let mut result = LibraryHealthActionResult {
        action: action.to_string(),
        applied: 0,
        skipped: 0,
        errors: Vec::new(),
        media_gc: None,
        report_queued: false,
    };

    match action {
        self::action::RESCAN_ROOT => {
            let mut roots: Vec<i64> = findings
                .iter()
                .filter_map(|finding| finding.root_id)
                .collect();
            roots.sort_unstable();
            roots.dedup();
            if roots.is_empty() {
                if crate::scanner::trigger(state).await {
                    result.applied = 1;
                } else {
                    result.skipped = 1;
                    result.errors.push("A scan is already running".to_string());
                }
            } else {
                for root_id in roots {
                    if crate::scanner::trigger_root(state, root_id).await {
                        result.applied += 1;
                    } else {
                        result.skipped += 1;
                        result
                            .errors
                            .push(format!("Root {root_id}: a scan is already running"));
                    }
                }
            }
        }
        self::action::REFRESH_METADATA => {
            let mut games: Vec<i64> = findings
                .iter()
                .filter_map(|finding| finding.game_id)
                .collect();
            games.sort_unstable();
            games.dedup();
            games.truncate(MAX_METADATA_REFRESH);
            for game_id in &games {
                let state = state.clone();
                let game_id = *game_id;
                tokio::spawn(async move {
                    crate::metadata::enrich_game(&state, game_id).await;
                });
            }
            result.applied = games.len();
        }
        self::action::MARK_MISSING => {
            for finding in &findings {
                let Some(version_id) = finding.version_id else {
                    result.skipped += 1;
                    continue;
                };
                match mark_version_missing(state, version_id).await {
                    Ok(true) => result.applied += 1,
                    Ok(false) => result.skipped += 1,
                    Err(error) => {
                        result.skipped += 1;
                        result.errors.push(error);
                    }
                }
            }
        }
        self::action::REMOVE_VERSION => {
            for finding in &findings {
                let Some(version_id) = finding.version_id else {
                    result.skipped += 1;
                    continue;
                };
                match remove_version(state, version_id).await {
                    Ok(true) => result.applied += 1,
                    Ok(false) => result.skipped += 1,
                    Err(error) => {
                        result.skipped += 1;
                        result.errors.push(error);
                    }
                }
            }
        }
        self::action::REMOVE_ORPHAN_GAME => {
            for finding in &findings {
                let Some(game_id) = finding.game_id else {
                    result.skipped += 1;
                    continue;
                };
                match remove_orphan_game(state, game_id).await {
                    Ok(true) => result.applied += 1,
                    Ok(false) => result.skipped += 1,
                    Err(error) => {
                        result.skipped += 1;
                        result.errors.push(error);
                    }
                }
            }
        }
        _ => {
            result.skipped = findings.len();
        }
    }

    if result.applied > 0 && action != self::action::REFRESH_METADATA {
        result.report_queued = start(state, HealthTrigger::Action).await.is_ok();
    }
    Ok(result)
}

async fn load_action_findings(
    state: &AppState,
    ids: &[i64],
) -> Result<Vec<ActionFindingRow>, ApiError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = ids.iter().copied().take(MAX_ACTION_FINDINGS).collect();
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT game_id, version_id, root_id
         FROM library_health_findings
         WHERE report_id = (SELECT id FROM library_health_reports ORDER BY id DESC LIMIT 1)
           AND id IN ({placeholders})"
    );
    let mut query = db::query_as::<ActionFindingRow>(&sql);
    for id in &ids {
        query = query.bind(*id);
    }
    query.fetch_all(&state.pool).await.map_err(ApiError::from)
}

async fn mark_version_missing(state: &AppState, version_id: i64) -> Result<bool, String> {
    let row = db::query_as::<ActionVersionRow>(
        "SELECT missing, root_id, file_path, storage_backend
         FROM game_versions WHERE id = ?",
    )
    .bind(version_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| format!("Version {version_id} no longer exists"))?;
    if row.missing != 0 {
        return Ok(false);
    }
    if row.storage_backend.as_deref() == Some("s3") && state.storage.is_remote() {
        return Err(format!(
            "Version {version_id} lives in object storage; run an integrity check instead"
        ));
    }
    let path =
        crate::roots::resolve_version_path(&state.pool, &state.config, row.root_id, &row.file_path)
            .await
            .ok_or_else(|| format!("Version {version_id} has an unsafe path"))?;
    if tokio::fs::try_exists(&path).await.unwrap_or(true) {
        return Err(format!("Version {version_id}: the file still exists"));
    }
    db::query("UPDATE game_versions SET missing = 1 WHERE id = ?")
        .bind(version_id)
        .execute(&state.pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(true)
}

async fn remove_version(state: &AppState, version_id: i64) -> Result<bool, String> {
    let row = db::query_as::<ActionVersionRow>(
        "SELECT missing, root_id, file_path, storage_backend
         FROM game_versions WHERE id = ?",
    )
    .bind(version_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| format!("Version {version_id} no longer exists"))?;
    if row.storage_backend.as_deref() == Some("s3") {
        return Err(format!(
            "Version {version_id} lives in object storage; remove it from the storage tools"
        ));
    }
    if row.missing == 0 {
        let exists = match crate::roots::resolve_version_path(
            &state.pool,
            &state.config,
            row.root_id,
            &row.file_path,
        )
        .await
        {
            Some(path) => tokio::fs::try_exists(&path).await.unwrap_or(true),
            None => false,
        };
        if exists {
            return Err(format!(
                "Version {version_id}: the archive file still exists, refusing to drop the row"
            ));
        }
    }
    db::query("DELETE FROM game_versions WHERE id = ?")
        .bind(version_id)
        .execute(&state.pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(true)
}

async fn remove_orphan_game(state: &AppState, game_id: i64) -> Result<bool, String> {
    let versions = db::query_scalar::<i64>("SELECT COUNT(*) FROM game_versions WHERE game_id = ?")
        .bind(game_id)
        .fetch_one(&state.pool)
        .await
        .map_err(|error| error.to_string())?;
    if versions > 0 {
        return Err(format!(
            "Game {game_id} still has {versions} version(s); remove the files first"
        ));
    }
    let progress = db::query_scalar::<i64>("SELECT COUNT(*) FROM progress WHERE game_id = ?")
        .bind(game_id)
        .fetch_one(&state.pool)
        .await
        .map_err(|error| error.to_string())?;
    let bookmarks = db::query_scalar::<i64>("SELECT COUNT(*) FROM bookmarks WHERE game_id = ?")
        .bind(game_id)
        .fetch_one(&state.pool)
        .await
        .map_err(|error| error.to_string())?;
    let saves = db::query_scalar::<i64>("SELECT COUNT(*) FROM save_snapshots WHERE game_id = ?")
        .bind(game_id)
        .fetch_one(&state.pool)
        .await
        .map_err(|error| error.to_string())?;
    if progress + bookmarks + saves > 0 {
        return Err(format!(
            "Game {game_id} still has user data (progress, bookmarks or saves)"
        ));
    }
    db::query("DELETE FROM games WHERE id = ?")
        .bind(game_id)
        .execute(&state.pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(true)
}

/// Runs a report on the configured interval. Disabled when the interval is zero.
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
                match start(&state, HealthTrigger::Scheduled).await {
                    Ok(report_id) => tracing::info!(report_id, "scheduled library health report started"),
                    Err(error) => tracing::debug!(code = %error.code, "scheduled library health report skipped"),
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
        config.media_gc_grace = Duration::ZERO;
        let (scan_tx, _scan_rx) = tokio::sync::mpsc::channel(4);
        AppState::for_tests(config, scan_tx).await
    }

    async fn seed_game(state: &AppState, title: &str, year: Option<&str>) -> i64 {
        let now = db::db_time(db::now());
        let release_date = year.map(|year| format!("{year}-01-01T00:00:00Z"));
        db::query(
            "INSERT INTO games (title, sort_title, release_date, cover_path, description,
                provider_slug, created_at, updated_at)
             VALUES (?, ?, ?, NULL, NULL, NULL, ?, ?)",
        )
        .bind(title)
        .bind(title)
        .bind(release_date)
        .bind(&now)
        .bind(&now)
        .returning_id(&state.pool)
        .await
        .expect("game")
    }

    async fn seed_version(
        state: &AppState,
        game_id: i64,
        file_name: &str,
        size: i64,
        missing: bool,
        content_hash: Option<&str>,
    ) -> i64 {
        let root_id =
            db::query_scalar::<i64>("SELECT id FROM library_roots ORDER BY position, id LIMIT 1")
                .fetch_one(&state.pool)
                .await
                .expect("root");
        let now = db::db_time(db::now());
        db::query(
            "INSERT INTO game_versions
                (game_id, root_id, file_path, file_name, size, missing, content_hash, indexed_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(game_id)
        .bind(root_id)
        .bind(file_name)
        .bind(file_name)
        .bind(size)
        .bind(i64::from(missing))
        .bind(content_hash)
        .bind(&now)
        .returning_id(&state.pool)
        .await
        .expect("version")
    }

    async fn run(state: &AppState) -> LibraryHealthResponse {
        let report_id = insert_report(&state.pool, HealthTrigger::Manual)
            .await
            .expect("report");
        run_report(state, report_id).await;
        read_latest(&state.pool, None, None, 500, 0)
            .await
            .expect("read report")
    }

    fn kinds(response: &LibraryHealthResponse) -> Vec<String> {
        response
            .findings
            .iter()
            .map(|finding| finding.kind.clone())
            .collect()
    }

    #[tokio::test]
    async fn report_finds_missing_archives_without_deleting_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = test_state(temp.path()).await;
        fs::write(
            state.config.games_dir.join("Hades (2020).zip"),
            vec![7_u8; 2048],
        )
        .expect("write archive");
        let hades = seed_game(&state, "Hades", Some("2020")).await;
        let hades_version =
            seed_version(&state, hades, "Hades (2020).zip", 2048, false, None).await;
        let gone = seed_game(&state, "Gone", Some("2019")).await;
        let gone_version = seed_version(&state, gone, "Gone (2019).zip", 4096, true, None).await;

        let response = run(&state).await;
        let report = response.report.expect("report");
        assert_eq!(report.status, "completed", "{report:?}");
        assert_eq!(report.progress, 100);
        assert!(report.findings_count > 0);

        let missing = response
            .findings
            .iter()
            .find(|finding| finding.kind == "missing_archive")
            .expect("missing archive finding");
        assert_eq!(missing.severity, severity::ERROR);
        assert_eq!(missing.version_id, Some(gone_version));
        assert!(missing
            .actions
            .iter()
            .any(|action| action == action::REMOVE_VERSION));

        let healthy = response
            .findings
            .iter()
            .filter(|finding| finding.version_id == Some(hades_version))
            .collect::<Vec<_>>();
        assert!(
            healthy
                .iter()
                .all(|finding| finding.kind != "missing_archive"),
            "{healthy:?}"
        );
        assert!(
            state.config.games_dir.join("Hades (2020).zip").exists(),
            "health checks never delete archive files"
        );
    }

    #[tokio::test]
    async fn report_finds_orphaned_media_and_duplicate_games() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = test_state(temp.path()).await;
        fs::write(state.config.media_dir.join("orphan.jpg"), vec![0_u8; 128]).expect("orphan");
        let first = seed_game(&state, "Celeste", Some("2018")).await;
        seed_version(&state, first, "Celeste (2018).zip", 2048, true, None).await;
        let second = seed_game(&state, "celeste!", Some("2018")).await;
        seed_version(&state, second, "Celeste alt (2018).zip", 2048, true, None).await;
        let now = db::db_time(db::now());
        db::query("UPDATE games SET cover_path = 'missing-cover.jpg', updated_at = ? WHERE id = ?")
            .bind(&now)
            .bind(first)
            .execute(&state.pool)
            .await
            .expect("cover");

        let response = run(&state).await;
        let found = kinds(&response);
        assert!(
            found.iter().any(|kind| kind == "duplicate_game"),
            "near-identical titles are a duplicate candidate: {found:?}"
        );
        assert!(
            found.iter().any(|kind| kind == "game_without_version"),
            "both versions are missing: {found:?}"
        );
        assert!(found.iter().any(|kind| kind == "media_orphan"), "{found:?}");
        assert!(
            found.iter().any(|kind| kind == "media_missing"),
            "{found:?}"
        );
        assert!(found.iter().any(|kind| kind == "metadata_gap"), "{found:?}");
        assert!(state.config.media_dir.join("orphan.jpg").exists());
    }

    #[tokio::test]
    async fn duplicate_titles_and_hashes_are_reported() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = test_state(temp.path()).await;
        let first = seed_game(&state, "Hollow Knight", Some("2017")).await;
        let second = seed_game(&state, "hollow-knight", Some("2017")).await;
        fs::write(
            state.config.games_dir.join("Hollow Knight (2017).zip"),
            vec![1_u8; 2048],
        )
        .expect("one");
        fs::write(
            state.config.games_dir.join("Hollow Knight copy.zip"),
            vec![2_u8; 2048],
        )
        .expect("two");
        seed_version(
            &state,
            first,
            "Hollow Knight (2017).zip",
            2048,
            false,
            Some("same-hash"),
        )
        .await;
        seed_version(
            &state,
            second,
            "Hollow Knight copy.zip",
            2048,
            false,
            Some("same-hash"),
        )
        .await;

        let response = run(&state).await;
        let found = kinds(&response);
        assert!(
            found
                .iter()
                .filter(|kind| *kind == "duplicate_game")
                .count()
                >= 1,
            "{found:?}"
        );
        assert!(
            found.iter().any(|kind| kind == "duplicate_archive"),
            "{found:?}"
        );
        let duplicate = response
            .findings
            .iter()
            .find(|finding| finding.kind == "duplicate_archive")
            .expect("archive duplicate");
        assert_eq!(
            duplicate.evidence["versionIds"].as_array().map(Vec::len),
            Some(2)
        );
    }

    #[tokio::test]
    async fn media_gc_dry_run_and_apply_require_confirmation_and_keep_archives() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = test_state(temp.path()).await;
        let archive = state.config.games_dir.join("Keep (2020).zip");
        fs::write(&archive, vec![3_u8; 2048]).expect("archive");
        let game = seed_game(&state, "Keep", Some("2020")).await;
        seed_version(&state, game, "Keep (2020).zip", 2048, false, None).await;
        let orphan = state.config.media_dir.join("stale.jpg");
        fs::write(&orphan, vec![0_u8; 64]).expect("orphan");

        let dry_run = apply_action(
            &state,
            &LibraryHealthActionRequest {
                action: action::MEDIA_GC_DRY_RUN.to_string(),
                finding_ids: Vec::new(),
                confirm: false,
            },
        )
        .await
        .expect("dry run");
        let gc = dry_run.media_gc.expect("media gc");
        assert!(gc.dry_run);
        assert_eq!(gc.deleted_files, 0);
        assert!(orphan.exists());

        let refused = apply_action(
            &state,
            &LibraryHealthActionRequest {
                action: action::MEDIA_GC_APPLY.to_string(),
                finding_ids: Vec::new(),
                confirm: false,
            },
        )
        .await;
        assert!(refused.is_err(), "apply must require confirmation");
        assert!(orphan.exists());

        let applied = apply_action(
            &state,
            &LibraryHealthActionRequest {
                action: action::MEDIA_GC_APPLY.to_string(),
                finding_ids: Vec::new(),
                confirm: true,
            },
        )
        .await
        .expect("apply");
        assert_eq!(applied.media_gc.expect("media gc").deleted_files, 1);
        assert!(!orphan.exists(), "unreferenced artwork is removed");
        assert!(archive.exists(), "media GC never deletes archive files");
    }

    #[test]
    fn titles_are_normalised_and_years_extracted() {
        assert_eq!(
            normalized_title("  Hollow-Knight: 2017 "),
            "hollow knight 2017"
        );
        assert_eq!(
            release_year(&Some("2017-02-24T00:00:00Z".to_string())),
            Some("2017".to_string())
        );
        assert_eq!(release_year(&Some("unknown".to_string())), None);
    }
}
