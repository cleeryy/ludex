//! Admin observability dashboard (#170).
//!
//! One bounded snapshot for the in-product dashboard: windowed request
//! statistics from the in-process registry, background job counts, database
//! size/pool health, storage usage, recent audit events, webhook failures and
//! integrity findings. Every query is capped and every collection is a fixed
//! size, so the endpoint stays cheap on a server with a large library.
//!
//! Nothing here exposes a secret. Library root paths are collapsed to labels,
//! integrity paths are omitted, and the Prometheus section reports whether a
//! scrape token is configured without ever returning it.

use std::path::Path;
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use crate::db;
use crate::metrics::{self, IntegritySummary};
use crate::state::AppState;

/// Recent audit rows returned by the dashboard.
const RECENT_EVENTS_LIMIT: i64 = 12;
/// Undelivered webhook rows returned by the dashboard.
const RECENT_WEBHOOK_FAILURES_LIMIT: i64 = 8;
/// Findings from the latest integrity report.
const RECENT_FINDINGS_LIMIT: i64 = 8;
/// Error series shown by the dashboard.
const TOP_ERRORS_LIMIT: usize = 5;
/// Maximum backup archives listed while computing usage.
const MAX_BACKUP_ENTRIES: usize = 512;

/// Time window a dashboard query covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservabilityWindow {
    /// Last hour.
    OneHour,
    /// Last 24 hours.
    Day,
}

impl ObservabilityWindow {
    /// Window used when the query string omits `window`.
    pub const DEFAULT: Self = Self::OneHour;

    /// Parses the `window` query value.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "1h" => Some(Self::OneHour),
            "24h" => Some(Self::Day),
            _ => None,
        }
    }

    /// Canonical query value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OneHour => "1h",
            Self::Day => "24h",
        }
    }

    /// Window length in seconds.
    #[must_use]
    pub fn seconds(self) -> i64 {
        match self {
            Self::OneHour => 3_600,
            Self::Day => 86_400,
        }
    }
}

/// Server identity and uptime.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityServer {
    /// Crate version.
    pub version: String,
    /// Build commit, or `unknown`.
    pub commit: String,
    /// Seconds since the process started.
    pub uptime_seconds: u64,
    /// Process start timestamp.
    pub started_at: DateTime<Utc>,
}

/// Request totals for the selected window.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityRequests {
    /// Requests seen in the window.
    pub total: u64,
    /// Average requests per second over the window.
    pub per_second: f64,
    /// 2xx responses.
    pub success: u64,
    /// 3xx responses.
    pub redirects: u64,
    /// 4xx responses.
    pub client_errors: u64,
    /// 5xx responses.
    pub server_errors: u64,
    /// Share of responses that were 4xx or 5xx, `0..=1`.
    pub error_rate: f64,
    /// Mean response time in milliseconds.
    pub average_ms: f64,
    /// 50th percentile response time in milliseconds.
    pub p50_ms: f64,
    /// 95th percentile response time in milliseconds.
    pub p95_ms: f64,
    /// 99th percentile response time in milliseconds.
    pub p99_ms: f64,
}

/// One point of the request series.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityPoint {
    /// Bucket start.
    pub at: DateTime<Utc>,
    /// Requests in the bucket.
    pub requests: u64,
    /// 2xx responses.
    pub success: u64,
    /// 3xx responses.
    pub redirects: u64,
    /// 4xx responses.
    pub client_errors: u64,
    /// 5xx responses.
    pub server_errors: u64,
    /// Mean response time in milliseconds.
    pub average_ms: f64,
}

/// Background work currently in flight.
#[derive(Debug, Clone, Copy, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityActive {
    /// Library scans running (0 or 1).
    pub scans: i64,
    /// Ingest jobs running (downloading or verifying).
    pub ingest_running: i64,
    /// Ingest jobs waiting to start.
    pub ingest_queued: i64,
    /// Archive transfers streaming to a client.
    pub downloads: u64,
    /// Whether an integrity report is running.
    pub integrity_running: bool,
}

/// Database health for the dashboard.
#[derive(Debug, Clone, Copy, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityDatabase {
    /// `SELECT 1` succeeded.
    pub ok: bool,
    /// `sqlite` or `postgres`.
    pub backend: &'static str,
    /// Database size in bytes.
    pub size_bytes: u64,
    /// Round-trip time of the health probe, milliseconds.
    pub latency_ms: f64,
    /// Live connections including idle ones.
    pub connections: u32,
    /// Idle connections ready for reuse.
    pub idle_connections: u32,
    /// Configured connection ceiling.
    pub max_connections: u32,
}

/// Free space on the configured volumes.
#[derive(Debug, Clone, Copy, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityDisk {
    /// Free bytes under the data directory.
    pub data_free_bytes: u64,
    /// Free bytes under the games directory.
    pub games_free_bytes: u64,
    /// Free bytes under the media directory.
    pub media_free_bytes: u64,
}

/// Cached artwork.
#[derive(Debug, Clone, Copy, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityMedia {
    /// Cached files.
    pub files: u64,
    /// Cached bytes.
    pub bytes: u64,
}

/// Cloud save snapshots stored by the server.
#[derive(Debug, Clone, Copy, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilitySaves {
    /// Snapshot count.
    pub snapshots: i64,
    /// Total bytes recorded for snapshots.
    pub bytes: i64,
}

/// Backup archives found under `{data_dir}/backups`.
#[derive(Debug, Clone, Copy, Default, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityBackups {
    /// Archive count, capped at 512 files.
    pub archives: u64,
    /// Total archive bytes.
    pub bytes: u64,
    /// Modification time of the newest archive.
    pub latest_at: Option<DateTime<Utc>>,
}

/// One library root, without its filesystem path.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityRoot {
    /// Human-readable label.
    pub label: String,
    /// Whether the scanner walks this root.
    pub enabled: bool,
    /// Whether the path was readable at the last scan.
    pub available: bool,
    /// Indexed versions stored under this root.
    pub versions: i64,
    /// Why the root was marked unavailable.
    pub last_error: Option<String>,
}

/// Storage usage for the dashboard.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityStorage {
    /// `local` or `s3`.
    pub backend: String,
    /// Free bytes per volume.
    pub disk: ObservabilityDisk,
    /// Media cache usage.
    pub media: ObservabilityMedia,
    /// Cloud save usage.
    pub saves: ObservabilitySaves,
    /// Backup archives.
    pub backups: ObservabilityBackups,
    /// Library roots, path-free.
    pub roots: Vec<ObservabilityRoot>,
}

/// One error series from the cumulative request counters.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityErrorRoute {
    /// HTTP method.
    pub method: String,
    /// Route template, never a concrete id.
    pub route: String,
    /// Response status.
    pub status: u16,
    /// Responses seen since process start.
    pub count: u64,
}

/// One recent audit entry.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityEvent {
    /// Audit row id.
    pub id: i64,
    /// Stable action name.
    pub action: String,
    /// `user`, `api_key` or `system`.
    pub actor_kind: String,
    /// Target family.
    pub target_type: Option<String>,
    /// Target identifier.
    pub target_id: Option<String>,
    /// When the event was recorded.
    pub created_at: Option<DateTime<Utc>>,
}

/// One undelivered webhook attempt.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityWebhookFailure {
    /// Delivery row id.
    pub id: i64,
    /// Event name that triggered the delivery.
    pub event: String,
    /// Last HTTP status, when the endpoint answered.
    pub status_code: Option<i64>,
    /// Attempts made so far.
    pub attempts: i64,
    /// Last transport or HTTP error.
    pub last_error: Option<String>,
    /// Next scheduled attempt.
    pub next_attempt_at: Option<DateTime<Utc>>,
}

/// Webhook delivery health.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityWebhooks {
    /// Deliveries waiting for a retry.
    pub pending: i64,
    /// Deliveries abandoned after the last attempt.
    pub failed: i64,
    /// Oldest-first list of undelivered attempts, newest first.
    pub recent_failures: Vec<ObservabilityWebhookFailure>,
}

/// One integrity finding.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityFinding {
    /// `missing`, `corrupt` or `size_mismatch`.
    pub kind: String,
    /// `archive` or `media`.
    pub target_type: String,
    /// Version id or media file name.
    pub target_id: Option<String>,
    /// Human-readable detail.
    pub detail: Option<String>,
    /// When the finding was recorded.
    pub created_at: Option<DateTime<Utc>>,
}

/// Integrity state.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityIntegrity {
    /// Latest report, if any.
    pub last: Option<IntegritySummary>,
    /// Newest findings from that report, capped at 8.
    pub recent_findings: Vec<ObservabilityFinding>,
}

/// Prometheus scrape configuration state. The token itself is never returned.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityPrometheus {
    /// Scrape path.
    pub endpoint: String,
    /// Whether `LUDEX_METRICS_TOKEN` is configured.
    pub token_protected: bool,
}

/// Complete admin dashboard snapshot.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilitySnapshot {
    /// When the snapshot was assembled.
    pub generated_at: DateTime<Utc>,
    /// `1h` or `24h`.
    pub window: String,
    /// Version, commit and uptime.
    pub server: ObservabilityServer,
    /// Windowed request statistics.
    pub requests: ObservabilityRequests,
    /// Bounded request series, oldest first.
    pub series: Vec<ObservabilityPoint>,
    /// Most frequent 4xx/5xx series since process start, capped at 5.
    pub top_errors: Vec<ObservabilityErrorRoute>,
    /// Jobs in flight.
    pub active: ObservabilityActive,
    /// Database health.
    pub database: ObservabilityDatabase,
    /// Storage usage.
    pub storage: ObservabilityStorage,
    /// Recent audit entries, newest first.
    pub recent_events: Vec<ObservabilityEvent>,
    /// Webhook delivery health.
    pub webhooks: ObservabilityWebhooks,
    /// Integrity state.
    pub integrity: ObservabilityIntegrity,
    /// Prometheus scrape state.
    pub prometheus: ObservabilityPrometheus,
}

/// Collects the dashboard snapshot. Database failures degrade individual
/// sections instead of failing the request.
pub async fn snapshot(state: &AppState, window: ObservabilityWindow) -> ObservabilitySnapshot {
    let generated_at = Utc::now();
    let started = Instant::now();
    let db_ok = state.pool.ping().await;
    let db_latency_ms = started.elapsed().as_secs_f64() * 1000.0;
    let pool = state.pool.pool_stats();
    let scan = state.scan.read().await.clone();
    let ingest = state.ingest.as_ref().map(|handle| handle.counts());
    let window_stats = state.metrics.window(window.seconds());
    let latency = state.metrics.latency();

    let error_count = window_stats.client_errors + window_stats.server_errors;
    let total = window_stats.total;
    let seconds = window.seconds().max(1) as f64;

    let (saves_snapshots, saves_bytes) = save_usage(state).await;
    let backups = backup_usage(&state.config.data_dir).await;
    let (media_files, media_bytes) = crate::metrics::media_usage(&state.config.media_dir).await;
    let roots = crate::roots::list(&state.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|root| ObservabilityRoot {
            label: root.label,
            enabled: root.enabled,
            available: root.available,
            versions: root.version_count,
            last_error: root.last_error,
        })
        .collect();
    let last_integrity = metrics::last_integrity(state).await;
    let recent_findings = match &last_integrity {
        Some(report) => integrity_findings(state, report.id).await,
        None => Vec::new(),
    };
    let (webhook_pending, webhook_failed, recent_failures) = webhook_health(state).await;
    let uptime_seconds = state.uptime_seconds();

    ObservabilitySnapshot {
        generated_at,
        window: window.as_str().to_string(),
        server: ObservabilityServer {
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit: metrics::commit(),
            uptime_seconds,
            started_at: generated_at - Duration::seconds(uptime_seconds as i64),
        },
        requests: ObservabilityRequests {
            total,
            per_second: total as f64 / seconds,
            success: window_stats.success,
            redirects: window_stats.redirects,
            client_errors: window_stats.client_errors,
            server_errors: window_stats.server_errors,
            error_rate: if total == 0 {
                0.0
            } else {
                error_count as f64 / total as f64
            },
            average_ms: window_stats.average_ms,
            p50_ms: latency.p50_ms,
            p95_ms: latency.p95_ms,
            p99_ms: latency.p99_ms,
        },
        series: window_stats
            .samples
            .iter()
            .map(|point| ObservabilityPoint {
                at: metrics::slot_time(point.at),
                requests: point.requests,
                success: point.success,
                redirects: point.redirects,
                client_errors: point.client_errors,
                server_errors: point.server_errors,
                average_ms: point.average_ms,
            })
            .collect(),
        top_errors: state
            .metrics
            .top_errors(TOP_ERRORS_LIMIT)
            .into_iter()
            .map(|row| ObservabilityErrorRoute {
                method: row.method,
                route: row.route,
                status: row.status,
                count: row.count,
            })
            .collect(),
        active: ObservabilityActive {
            scans: i64::from(scan.running),
            ingest_running: ingest.map_or(0, |counts| counts.running),
            ingest_queued: ingest.map_or(0, |counts| counts.queued),
            downloads: state.metrics.active_downloads(),
            integrity_running: state
                .integrity_busy
                .load(std::sync::atomic::Ordering::Relaxed),
        },
        database: ObservabilityDatabase {
            ok: db_ok,
            backend: state.pool.backend().as_str(),
            size_bytes: match state.pool.backend() {
                crate::db::Backend::Sqlite => {
                    crate::metrics::database_size(&state.config.database_url).await
                }
                crate::db::Backend::Postgres => state.pool.size_bytes().await,
            },
            latency_ms: db_latency_ms,
            connections: pool.size,
            idle_connections: pool.idle,
            max_connections: pool.max_connections,
        },
        storage: ObservabilityStorage {
            backend: state.storage.backend().as_str().to_string(),
            disk: ObservabilityDisk {
                data_free_bytes: crate::metrics::free_space(&state.config.data_dir),
                games_free_bytes: crate::metrics::free_space(&state.config.games_dir),
                media_free_bytes: crate::metrics::free_space(&state.config.media_dir),
            },
            media: ObservabilityMedia {
                files: media_files,
                bytes: media_bytes,
            },
            saves: ObservabilitySaves {
                snapshots: saves_snapshots,
                bytes: saves_bytes,
            },
            backups,
            roots,
        },
        recent_events: recent_events(state).await,
        webhooks: ObservabilityWebhooks {
            pending: webhook_pending,
            failed: webhook_failed,
            recent_failures,
        },
        integrity: ObservabilityIntegrity {
            last: last_integrity,
            recent_findings,
        },
        prometheus: ObservabilityPrometheus {
            endpoint: "/metrics".to_string(),
            token_protected: state.config.metrics_token.is_some(),
        },
    }
}

async fn recent_events(state: &AppState) -> Vec<ObservabilityEvent> {
    let rows = db::query_as::<(i64, String, String, Option<String>, Option<String>, String)>(
        "SELECT id, action, actor_kind, target_type, target_id, created_at
         FROM audit_logs ORDER BY id DESC LIMIT ?",
    )
    .bind(RECENT_EVENTS_LIMIT)
    .fetch_all(&state.pool)
    .await;
    let Ok(rows) = rows else {
        return Vec::new();
    };
    rows.into_iter()
        .map(
            |(id, action, actor_kind, target_type, target_id, created_at)| ObservabilityEvent {
                id,
                action,
                actor_kind,
                target_type,
                target_id,
                created_at: db::parse_time(&created_at),
            },
        )
        .collect()
}

async fn webhook_health(state: &AppState) -> (i64, i64, Vec<ObservabilityWebhookFailure>) {
    let counts = db::query_as::<(i64, i64)>(
        "SELECT
            CAST(COALESCE(SUM(CASE WHEN delivered_at IS NULL AND next_attempt_at IS NOT NULL THEN 1 ELSE 0 END), 0) AS BIGINT),
            CAST(COALESCE(SUM(CASE WHEN delivered_at IS NULL AND next_attempt_at IS NULL THEN 1 ELSE 0 END), 0) AS BIGINT)
         FROM webhook_deliveries",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or((0, 0));

    let rows = db::query_as::<(
        i64,
        String,
        Option<i64>,
        i64,
        Option<String>,
        Option<String>,
    )>(
        "SELECT id, event, status_code, attempts, last_error, next_attempt_at
         FROM webhook_deliveries
         WHERE delivered_at IS NULL
         ORDER BY id DESC LIMIT ?",
    )
    .bind(RECENT_WEBHOOK_FAILURES_LIMIT)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    let failures = rows
        .into_iter()
        .map(
            |(id, event, status_code, attempts, last_error, next_attempt_at)| {
                ObservabilityWebhookFailure {
                    id,
                    event,
                    status_code,
                    attempts,
                    last_error,
                    next_attempt_at: next_attempt_at.as_deref().and_then(db::parse_time),
                }
            },
        )
        .collect();
    (counts.0, counts.1, failures)
}

async fn integrity_findings(state: &AppState, report_id: i64) -> Vec<ObservabilityFinding> {
    let rows = db::query_as::<(String, String, Option<String>, Option<String>, String)>(
        "SELECT kind, target_type, target_id, detail, created_at
         FROM integrity_findings
         WHERE report_id = ?
         ORDER BY id DESC LIMIT ?",
    )
    .bind(report_id)
    .bind(RECENT_FINDINGS_LIMIT)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(
            |(kind, target_type, target_id, detail, created_at)| ObservabilityFinding {
                kind,
                target_type,
                target_id,
                detail,
                created_at: db::parse_time(&created_at),
            },
        )
        .collect()
}

async fn save_usage(state: &AppState) -> (i64, i64) {
    db::query_as::<(i64, i64)>(
        "SELECT CAST(COUNT(*) AS BIGINT), CAST(COALESCE(SUM(size), 0) AS BIGINT)
         FROM save_snapshots",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or((0, 0))
}

/// Aggregates archives under `{data_dir}/backups` without ever listing a path
/// in the response. The directory is created by `ludex-backup` and may not
/// exist on a server that backs up elsewhere.
async fn backup_usage(data_dir: &Path) -> ObservabilityBackups {
    let directory = data_dir.join("backups");
    tokio::task::spawn_blocking(move || {
        let mut usage = ObservabilityBackups::default();
        let Ok(entries) = std::fs::read_dir(&directory) else {
            return usage;
        };
        for entry in entries.flatten().take(MAX_BACKUP_ENTRIES) {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let is_archive = entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(".tar.gz") || name.ends_with(".tgz"));
            if !is_archive {
                continue;
            }
            usage.archives += 1;
            usage.bytes += metadata.len();
            let modified = metadata.modified().ok().map(DateTime::<Utc>::from);
            if let Some(modified) = modified {
                usage.latest_at = Some(usage.latest_at.map_or(modified, |last| last.max(modified)));
            }
        }
        usage
    })
    .await
    .unwrap_or_default()
}
