//! Prometheus metrics and server health details (#48).
//!
//! The registry is intentionally dependency-free: fixed-bucket histograms and
//! counters keyed by route template, method and status. Route templates come
//! from axum's `MatchedPath`, so `/api/games/42` and `/api/games/7` share one
//! series and metric cardinality stays bounded.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{MatchedPath, State};
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use chrono::{DateTime, TimeZone, Utc};
use serde::Serialize;

use crate::db;
use crate::db::Database;
use crate::state::AppState;

/// Histogram upper bounds, in seconds.
const BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// Public `/metrics` scrapes allowed per IP in [`RATE_WINDOW`].
const PUBLIC_SCRAPES_PER_WINDOW: u32 = 30;
/// Sliding window for the public scrape limit.
const RATE_WINDOW: Duration = Duration::from_secs(60);

/// Seconds covered by one in-memory timeline slot.
const TIMELINE_SLOT_SECONDS: i64 = 300;
/// Slots needed for the largest supported window (24 h at 5 minutes).
const TIMELINE_WINDOW_SLOTS: i64 = 86_400 / TIMELINE_SLOT_SECONDS;
/// Hard cap on retained slots; older slots are dropped before the map grows.
const TIMELINE_MAX_SLOTS: usize = 512;
/// Maximum number of points a windowed series returns.
pub const MAX_SERIES_POINTS: usize = 96;

#[derive(Debug, Default, Clone)]
struct Histogram {
    buckets: [u64; BUCKETS.len()],
    sum: f64,
    count: u64,
}

impl Histogram {
    fn observe(&mut self, value: f64) {
        for (index, bound) in BUCKETS.iter().enumerate() {
            if value <= *bound {
                self.buckets[index] += 1;
            }
        }
        self.sum += value;
        self.count += 1;
    }
}

/// Counters for one 5-minute timeline slot.
#[derive(Debug, Default, Clone, Copy)]
struct SlotCounters {
    success: u64,
    redirect: u64,
    client_error: u64,
    server_error: u64,
    other: u64,
    duration_sum: f64,
    duration_count: u64,
}

impl SlotCounters {
    fn observe(&mut self, status: u16, seconds: f64) {
        match status {
            200..=299 => self.success += 1,
            300..=399 => self.redirect += 1,
            400..=499 => self.client_error += 1,
            500..=599 => self.server_error += 1,
            _ => self.other += 1,
        }
        self.duration_sum += seconds;
        self.duration_count += 1;
    }
}

/// One point in a windowed request series.
#[derive(Debug, Clone, Copy, Default)]
pub struct SeriesPoint {
    /// Bucket start, Unix seconds.
    pub at: i64,
    /// Requests seen in the bucket.
    pub requests: u64,
    /// 2xx responses.
    pub success: u64,
    /// 3xx responses.
    pub redirects: u64,
    /// 4xx responses.
    pub client_errors: u64,
    /// 5xx responses.
    pub server_errors: u64,
    /// Average response time in milliseconds for the bucket.
    pub average_ms: f64,
}

/// Aggregate request statistics for a bounded time window.
#[derive(Debug, Clone, Default)]
pub struct WindowStats {
    /// Requests in the window.
    pub total: u64,
    /// 2xx responses.
    pub success: u64,
    /// 3xx responses.
    pub redirects: u64,
    /// 4xx responses.
    pub client_errors: u64,
    /// 5xx responses.
    pub server_errors: u64,
    /// 1xx or unknown status responses.
    pub other: u64,
    /// Average response time in milliseconds.
    pub average_ms: f64,
    /// Windowed series, at most [`MAX_SERIES_POINTS`] points, oldest first.
    pub samples: Vec<SeriesPoint>,
}

impl WindowStats {
    fn add(&mut self, counters: &SlotCounters) {
        self.total += counters.success
            + counters.redirect
            + counters.client_error
            + counters.server_error
            + counters.other;
        self.success += counters.success;
        self.redirects += counters.redirect;
        self.client_errors += counters.client_error;
        self.server_errors += counters.server_error;
        self.other += counters.other;
    }
}

/// Latency percentiles derived from every recorded histogram.
#[derive(Debug, Clone, Copy, Default)]
pub struct LatencyStats {
    /// 50th percentile, milliseconds.
    pub p50_ms: f64,
    /// 95th percentile, milliseconds.
    pub p95_ms: f64,
    /// 99th percentile, milliseconds.
    pub p99_ms: f64,
    /// Mean latency, milliseconds.
    pub average_ms: f64,
}

/// One error series from the cumulative request counters.
#[derive(Debug, Clone)]
pub struct ErrorRoute {
    /// HTTP method.
    pub method: String,
    /// Route template.
    pub route: String,
    /// Response status.
    pub status: u16,
    /// Requests seen since process start.
    pub count: u64,
}

#[derive(Debug, Default)]
struct Window {
    started: Option<Instant>,
    count: u32,
}

/// In-process metrics registry.
#[derive(Debug, Default)]
pub struct Metrics {
    requests: Mutex<HashMap<(String, String, String), u64>>,
    durations: Mutex<HashMap<(String, String), Histogram>>,
    scrape_windows: Mutex<HashMap<IpAddr, Window>>,
    /// Bounded 5-minute timeline, keyed by slot index. Never grows past
    /// [`TIMELINE_MAX_SLOTS`] entries.
    timeline: Mutex<HashMap<i64, SlotCounters>>,
    /// Archive transfers currently streaming to a client.
    active_downloads: AtomicU64,
}

impl Metrics {
    /// Records one HTTP request.
    pub fn record(&self, method: &str, route: &str, status: u16, seconds: f64) {
        let method = normalize_method(method);
        let route = route.to_string();
        let status_label = status.to_string();
        if let Ok(mut requests) = self.requests.lock() {
            *requests
                .entry((method.clone(), route.clone(), status_label))
                .or_insert(0) += 1;
        }
        if let Ok(mut durations) = self.durations.lock() {
            durations
                .entry((method, route))
                .or_default()
                .observe(seconds);
        }
        if let Ok(mut timeline) = self.timeline.lock() {
            let slot = Utc::now().timestamp().div_euclid(TIMELINE_SLOT_SECONDS);
            if timeline.len() > TIMELINE_MAX_SLOTS {
                let cutoff = slot - TIMELINE_WINDOW_SLOTS;
                timeline.retain(|key, _| *key >= cutoff);
            }
            timeline.entry(slot).or_default().observe(status, seconds);
        }
    }

    /// Marks an archive transfer as in flight until the returned guard drops.
    pub fn download_started(&self) -> DownloadGuard<'_> {
        self.active_downloads.fetch_add(1, Ordering::Relaxed);
        DownloadGuard { metrics: self }
    }

    /// Archive transfers currently streaming.
    #[must_use]
    pub fn active_downloads(&self) -> u64 {
        self.active_downloads.load(Ordering::Relaxed)
    }

    /// Request counts and a bounded series for the last `seconds`.
    ///
    /// The series is downsampled so a response never carries more than
    /// [`MAX_SERIES_POINTS`] points, whatever the window.
    #[must_use]
    pub fn window(&self, seconds: i64) -> WindowStats {
        let slots = (seconds.div_euclid(TIMELINE_SLOT_SECONDS)).clamp(1, TIMELINE_WINDOW_SLOTS);
        let now_slot = Utc::now().timestamp().div_euclid(TIMELINE_SLOT_SECONDS);
        let first_slot = now_slot - slots + 1;
        let Ok(timeline) = self.timeline.lock() else {
            return WindowStats::default();
        };
        let mut stats = WindowStats::default();
        let mut raw: Vec<(i64, SlotCounters)> = Vec::with_capacity(slots as usize);
        let mut weighted_seconds = 0.0;
        let mut timed_requests = 0_u64;
        for slot in first_slot..=now_slot {
            let counters = timeline.get(&slot).copied().unwrap_or_default();
            stats.add(&counters);
            weighted_seconds += counters.duration_sum;
            timed_requests += counters.duration_count;
            raw.push((slot, counters));
        }
        if timed_requests > 0 {
            stats.average_ms = (weighted_seconds / timed_requests as f64) * 1000.0;
        }
        stats.samples = downsample(&raw, MAX_SERIES_POINTS);
        stats
    }

    /// Latency percentiles over every request recorded since start.
    #[must_use]
    pub fn latency(&self) -> LatencyStats {
        let Ok(durations) = self.durations.lock() else {
            return LatencyStats::default();
        };
        let mut buckets = [0_u64; BUCKETS.len()];
        let mut count = 0_u64;
        let mut sum = 0.0;
        for histogram in durations.values() {
            for (index, value) in histogram.buckets.iter().enumerate() {
                buckets[index] += value;
            }
            count += histogram.count;
            sum += histogram.sum;
        }
        if count == 0 {
            return LatencyStats::default();
        }
        let percentile = |fraction: f64| -> f64 {
            let rank = ((count as f64) * fraction).ceil().max(1.0) as u64;
            for (index, bound) in BUCKETS.iter().enumerate() {
                if buckets[index] >= rank {
                    return bound * 1000.0;
                }
            }
            BUCKETS[BUCKETS.len() - 1] * 1000.0
        };
        LatencyStats {
            p50_ms: percentile(0.50),
            p95_ms: percentile(0.95),
            p99_ms: percentile(0.99),
            average_ms: (sum / count as f64) * 1000.0,
        }
    }

    /// Most frequent 4xx/5xx series since process start, capped at `limit`.
    ///
    /// Route labels come from axum templates, so the list stays small even on a
    /// busy server.
    #[must_use]
    pub fn top_errors(&self, limit: usize) -> Vec<ErrorRoute> {
        let Ok(requests) = self.requests.lock() else {
            return Vec::new();
        };
        let mut rows: Vec<ErrorRoute> = requests
            .iter()
            .filter_map(|((method, route, status), count)| {
                let status = status.parse::<u16>().ok()?;
                (status >= 400).then(|| ErrorRoute {
                    method: method.clone(),
                    route: route.clone(),
                    status,
                    count: *count,
                })
            })
            .collect();
        rows.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.route.cmp(&right.route))
                .then_with(|| left.status.cmp(&right.status))
        });
        rows.truncate(limit.clamp(1, 20));
        rows
    }

    /// Whether a public scrape from `ip` is allowed. Rotating windows: at most
    /// [`PUBLIC_SCRAPES_PER_WINDOW`] requests per [`RATE_WINDOW`].
    #[must_use]
    pub fn allow_scrape(&self, ip: IpAddr) -> bool {
        let Ok(mut scrapes) = self.scrape_windows.lock() else {
            return true;
        };
        let now = Instant::now();
        let window = scrapes.entry(ip).or_default();
        match window.started {
            Some(started) if now.duration_since(started) < RATE_WINDOW => {
                if window.count >= PUBLIC_SCRAPES_PER_WINDOW {
                    return false;
                }
                window.count += 1;
            }
            _ => {
                window.started = Some(now);
                window.count = 1;
            }
        }
        true
    }

    /// Renders the counters and histograms in Prometheus text format.
    #[must_use]
    pub fn render(&self) -> String {
        let mut output = String::with_capacity(4096);
        output.push_str(
            "# HELP ludex_http_requests_total HTTP requests by route, method and status.\n",
        );
        output.push_str("# TYPE ludex_http_requests_total counter\n");
        if let Ok(requests) = self.requests.lock() {
            let mut rows: Vec<_> = requests.iter().collect();
            rows.sort_by(|left, right| left.0.cmp(right.0));
            for ((method, route, status), count) in rows {
                output.push_str(&format!(
                    "ludex_http_requests_total{{method=\"{}\",route=\"{}\",status=\"{}\"}} {count}\n",
                    escape(method),
                    escape(route),
                    escape(status)
                ));
            }
        }

        output.push_str("# HELP ludex_http_request_duration_seconds HTTP request latency.\n");
        output.push_str("# TYPE ludex_http_request_duration_seconds histogram\n");
        if let Ok(durations) = self.durations.lock() {
            let mut rows: Vec<_> = durations.iter().collect();
            rows.sort_by(|left, right| left.0.cmp(right.0));
            for ((method, route), histogram) in rows {
                let labels = format!("method=\"{}\",route=\"{}\"", escape(method), escape(route));
                for (index, bound) in BUCKETS.iter().enumerate() {
                    output.push_str(&format!(
                        "ludex_http_request_duration_seconds_bucket{{{labels},le=\"{bound}\"}} {}\n",
                        histogram.buckets[index]
                    ));
                }
                output.push_str(&format!(
                    "ludex_http_request_duration_seconds_bucket{{{labels},le=\"+Inf\"}} {}\n",
                    histogram.count
                ));
                output.push_str(&format!(
                    "ludex_http_request_duration_seconds_sum{{{labels}}} {}\n",
                    histogram.sum
                ));
                output.push_str(&format!(
                    "ludex_http_request_duration_seconds_count{{{labels}}} {}\n",
                    histogram.count
                ));
            }
        }
        output
    }
}

/// Middleware recording every request. Mounted on the final router so the
/// matched route template is already in the request extensions.
pub async fn track(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let method = request.method().to_string();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "unmatched".to_string(), |path| path.as_str().to_string());
    let started = Instant::now();
    let response = next.run(request).await;
    state.metrics.record(
        &method,
        &route,
        response.status().as_u16(),
        started.elapsed().as_secs_f64(),
    );
    response
}

/// RAII gauge for an archive transfer in flight.
pub struct DownloadGuard<'a> {
    metrics: &'a Metrics,
}

impl Drop for DownloadGuard<'_> {
    fn drop(&mut self) {
        self.metrics
            .active_downloads
            .fetch_sub(1, Ordering::Relaxed);
    }
}

/// Merges adjacent slots until at most `max_points` remain.
fn downsample(slots: &[(i64, SlotCounters)], max_points: usize) -> Vec<SeriesPoint> {
    if slots.is_empty() {
        return Vec::new();
    }
    let group = slots.len().div_ceil(max_points.max(1)).max(1);
    slots
        .chunks(group)
        .map(|chunk| {
            let mut totals = SlotCounters::default();
            for (_, counters) in chunk {
                totals.success += counters.success;
                totals.redirect += counters.redirect;
                totals.client_error += counters.client_error;
                totals.server_error += counters.server_error;
                totals.other += counters.other;
                totals.duration_sum += counters.duration_sum;
                totals.duration_count += counters.duration_count;
            }
            let requests = totals.success
                + totals.redirect
                + totals.client_error
                + totals.server_error
                + totals.other;
            SeriesPoint {
                at: chunk[0].0 * TIMELINE_SLOT_SECONDS,
                requests,
                success: totals.success,
                redirects: totals.redirect,
                client_errors: totals.client_error,
                server_errors: totals.server_error,
                average_ms: if totals.duration_count == 0 {
                    0.0
                } else {
                    (totals.duration_sum / totals.duration_count as f64) * 1000.0
                },
            }
        })
        .collect()
}

/// Formats a slot index as a UTC timestamp for API responses.
#[must_use]
pub fn slot_time(seconds: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(seconds, 0)
        .single()
        .unwrap_or_else(Utc::now)
}

fn normalize_method(method: &str) -> String {
    match method {
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" => method.to_string(),
        _ => "OTHER".to_string(),
    }
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Last integrity report, as exposed by `/metrics` and health details.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IntegritySummary {
    /// Report id.
    pub id: i64,
    /// `completed` or `failed`.
    pub status: String,
    /// Findings recorded by the report.
    pub findings: i64,
    /// Start timestamp.
    pub started_at: Option<DateTime<Utc>>,
    /// End timestamp.
    pub finished_at: Option<DateTime<Utc>>,
}

/// Free space on the three volumes the server uses.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiskUsage {
    /// Free bytes under the data directory.
    pub data_free: u64,
    /// Free bytes under the games directory.
    pub games_free: u64,
    /// Free bytes under the media directory.
    pub media_free: u64,
}

/// Everything `/metrics` and `/api/health/details` need from the database and
/// the background workers, collected in one place.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Database replied to `SELECT 1`.
    pub db_ok: bool,
    /// Number of games.
    pub games: i64,
    /// Versions whose file is present.
    pub versions_available: i64,
    /// Versions marked missing.
    pub versions_missing: i64,
    /// Registered library roots.
    pub roots_total: i64,
    /// Roots currently available.
    pub roots_available: i64,
    /// Queued ingest jobs.
    pub ingest_queued: i64,
    /// Running ingest jobs (downloading or verifying).
    pub ingest_running: i64,
    /// Ingest jobs in error.
    pub ingest_failed: i64,
    /// Completed ingest jobs.
    pub ingest_done: i64,
    /// Cancelled ingest jobs.
    pub ingest_cancelled: i64,
    /// Current scan status.
    pub scan_running: bool,
    /// Last scan start.
    pub scan_started_at: Option<DateTime<Utc>>,
    /// Last scan end.
    pub scan_finished_at: Option<DateTime<Utc>>,
    /// Duration of the last completed scan.
    pub scan_duration_seconds: Option<f64>,
    /// Whether the last scan finished without errors.
    pub scan_last_success: bool,
    /// Files examined by the last scan.
    pub scan_scanned: i64,
    /// Files added by the last scan.
    pub scan_added: i64,
    /// Files refreshed by the last scan.
    pub scan_updated: i64,
    /// Files removed by the last scan.
    pub scan_removed: i64,
    /// Delivered webhooks.
    pub webhook_delivered: i64,
    /// Webhook deliveries waiting for a retry.
    pub webhook_pending: i64,
    /// Webhook deliveries abandoned after the last attempt.
    pub webhook_failed: i64,
    /// Extra attempts beyond the first (retries).
    pub webhook_retries: i64,
    /// Cached media files.
    pub media_files: u64,
    /// Cached media bytes.
    pub media_bytes: u64,
    /// Database file size, including WAL.
    pub db_bytes: u64,
    /// Free space on the configured volumes.
    pub disk: DiskUsage,
    /// Last integrity report.
    pub integrity: Option<IntegritySummary>,
}

/// Collects the snapshot. Database failures degrade the snapshot instead of
/// failing the endpoint.
pub async fn snapshot(state: &AppState) -> Snapshot {
    let db_ok = state.pool.ping().await;

    let games = scalar(&state.pool, "SELECT COUNT(*) FROM games").await;
    let versions_available = scalar(
        &state.pool,
        "SELECT COUNT(*) FROM game_versions WHERE missing = 0",
    )
    .await;
    let versions_missing = scalar(
        &state.pool,
        "SELECT COUNT(*) FROM game_versions WHERE missing != 0",
    )
    .await;
    let roots_total = scalar(&state.pool, "SELECT COUNT(*) FROM library_roots").await;
    let roots_available = scalar(
        &state.pool,
        "SELECT COUNT(*) FROM library_roots WHERE available = 1",
    )
    .await;

    let (webhook_delivered, webhook_pending, webhook_failed, webhook_retries) =
        webhook_counts(state).await;

    let (media_files, media_bytes) = media_usage(&state.config.media_dir).await;
    let db_bytes = match state.pool.backend() {
        crate::db::Backend::Sqlite => database_size(&state.config.database_url).await,
        crate::db::Backend::Postgres => state.pool.size_bytes().await,
    };

    let scan = state.scan.read().await.clone();
    let scan_duration_seconds = match (scan.started_at, scan.finished_at) {
        (Some(started), Some(finished)) => {
            let seconds = (finished - started).num_milliseconds() as f64 / 1000.0;
            Some(seconds.max(0.0))
        }
        _ => None,
    };
    let scan_last_success = scan.finished_at.is_some() && scan.errors.is_empty();

    let (ingest_queued, ingest_running, ingest_failed, ingest_done, ingest_cancelled) =
        match state.ingest.as_ref() {
            Some(handle) => {
                let counts = handle.counts();
                (
                    counts.queued,
                    counts.running,
                    counts.failed,
                    counts.done,
                    counts.cancelled,
                )
            }
            None => (0, 0, 0, 0, 0),
        };

    Snapshot {
        db_ok,
        games,
        versions_available,
        versions_missing,
        roots_total,
        roots_available,
        ingest_queued,
        ingest_running,
        ingest_failed,
        ingest_done,
        ingest_cancelled,
        scan_running: scan.running,
        scan_started_at: scan.started_at,
        scan_finished_at: scan.finished_at,
        scan_duration_seconds,
        scan_last_success,
        scan_scanned: scan.scanned,
        scan_added: scan.added,
        scan_updated: scan.updated,
        scan_removed: scan.removed,
        webhook_delivered,
        webhook_pending,
        webhook_failed,
        webhook_retries,
        media_files,
        media_bytes,
        db_bytes,
        disk: DiskUsage {
            data_free: free_space(&state.config.data_dir),
            games_free: free_space(&state.config.games_dir),
            media_free: free_space(&state.config.media_dir),
        },
        integrity: last_integrity(state).await,
    }
}

/// Appends the gauge section to a Prometheus response.
#[must_use]
pub fn render_snapshot(state: &AppState, snapshot: &Snapshot) -> String {
    let mut output = String::with_capacity(4096);
    output.push_str("# HELP ludex_build_info Build version and commit.\n");
    output.push_str("# TYPE ludex_build_info gauge\n");
    output.push_str(&format!(
        "ludex_build_info{{version=\"{}\",commit=\"{}\"}} 1\n",
        escape(env!("CARGO_PKG_VERSION")),
        escape(&commit())
    ));
    output.push_str("# HELP ludex_process_uptime_seconds Process uptime in seconds.\n");
    output.push_str("# TYPE ludex_process_uptime_seconds gauge\n");
    output.push_str(&format!(
        "ludex_process_uptime_seconds {}\n",
        state.uptime_seconds()
    ));
    output.push_str("# HELP ludex_database_size_bytes Database size in bytes (SQLite file + WAL, or PostgreSQL database).\n");
    output.push_str("# TYPE ludex_database_size_bytes gauge\n");
    output.push_str(&format!(
        "ludex_database_size_bytes {}\n",
        snapshot.db_bytes
    ));

    output.push_str("# HELP ludex_games Number of games in the library.\n");
    output.push_str("# TYPE ludex_games gauge\n");
    output.push_str(&format!("ludex_games {}\n", snapshot.games));
    output.push_str("# HELP ludex_game_versions Indexed game versions.\n");
    output.push_str("# TYPE ludex_game_versions gauge\n");
    output.push_str(&format!(
        "ludex_game_versions{{missing=\"false\"}} {}\n",
        snapshot.versions_available
    ));
    output.push_str(&format!(
        "ludex_game_versions{{missing=\"true\"}} {}\n",
        snapshot.versions_missing
    ));
    output.push_str("# HELP ludex_library_roots Configured library roots.\n");
    output.push_str("# TYPE ludex_library_roots gauge\n");
    output.push_str(&format!(
        "ludex_library_roots{{available=\"true\"}} {}\n",
        snapshot.roots_available
    ));
    output.push_str(&format!(
        "ludex_library_roots{{available=\"false\"}} {}\n",
        snapshot.roots_total - snapshot.roots_available
    ));

    output.push_str("# HELP ludex_ingest_jobs Ingest jobs by status.\n");
    output.push_str("# TYPE ludex_ingest_jobs gauge\n");
    for (status, value) in [
        ("queued", snapshot.ingest_queued),
        ("running", snapshot.ingest_running),
        ("failed", snapshot.ingest_failed),
        ("done", snapshot.ingest_done),
        ("cancelled", snapshot.ingest_cancelled),
    ] {
        output.push_str(&format!(
            "ludex_ingest_jobs{{status=\"{status}\"}} {value}\n"
        ));
    }

    output.push_str("# HELP ludex_scan_running Whether a library scan is running.\n");
    output.push_str("# TYPE ludex_scan_running gauge\n");
    output.push_str(&format!(
        "ludex_scan_running {}\n",
        i64::from(snapshot.scan_running)
    ));
    output.push_str(
        "# HELP ludex_scan_last_success_timestamp_seconds Unix time of the last successful scan.\n",
    );
    output.push_str("# TYPE ludex_scan_last_success_timestamp_seconds gauge\n");
    if snapshot.scan_last_success {
        if let Some(finished) = snapshot.scan_finished_at {
            output.push_str(&format!(
                "ludex_scan_last_success_timestamp_seconds {}\n",
                finished.timestamp()
            ));
        }
    }
    output.push_str("# HELP ludex_scan_duration_seconds Duration of the last completed scan.\n");
    output.push_str("# TYPE ludex_scan_duration_seconds gauge\n");
    if let Some(duration) = snapshot.scan_duration_seconds {
        output.push_str(&format!("ludex_scan_duration_seconds {duration}\n"));
    }
    output.push_str("# HELP ludex_scan_files Files handled by the last scan, by result.\n");
    output.push_str("# TYPE ludex_scan_files gauge\n");
    for (result, value) in [
        ("scanned", snapshot.scan_scanned),
        ("added", snapshot.scan_added),
        ("updated", snapshot.scan_updated),
        ("removed", snapshot.scan_removed),
    ] {
        output.push_str(&format!(
            "ludex_scan_files{{result=\"{result}\"}} {value}\n"
        ));
    }

    output.push_str("# HELP ludex_webhook_deliveries Webhook deliveries by state.\n");
    output.push_str("# TYPE ludex_webhook_deliveries gauge\n");
    for (state_name, value) in [
        ("delivered", snapshot.webhook_delivered),
        ("pending", snapshot.webhook_pending),
        ("failed", snapshot.webhook_failed),
    ] {
        output.push_str(&format!(
            "ludex_webhook_deliveries{{state=\"{state_name}\"}} {value}\n"
        ));
    }
    output.push_str(
        "# HELP ludex_webhook_retries Extra delivery attempts recorded in the database.\n",
    );
    output.push_str("# TYPE ludex_webhook_retries gauge\n");
    output.push_str(&format!(
        "ludex_webhook_retries {}\n",
        snapshot.webhook_retries
    ));

    output.push_str("# HELP ludex_media_cache_files Cached artwork files.\n");
    output.push_str("# TYPE ludex_media_cache_files gauge\n");
    output.push_str(&format!(
        "ludex_media_cache_files {}\n",
        snapshot.media_files
    ));
    output.push_str("# HELP ludex_media_cache_bytes Cached artwork bytes.\n");
    output.push_str("# TYPE ludex_media_cache_bytes gauge\n");
    output.push_str(&format!(
        "ludex_media_cache_bytes {}\n",
        snapshot.media_bytes
    ));

    output.push_str("# HELP ludex_disk_free_bytes Free bytes per volume.\n");
    output.push_str("# TYPE ludex_disk_free_bytes gauge\n");
    output.push_str(&format!(
        "ludex_disk_free_bytes{{volume=\"data\"}} {}\n",
        snapshot.disk.data_free
    ));
    output.push_str(&format!(
        "ludex_disk_free_bytes{{volume=\"games\"}} {}\n",
        snapshot.disk.games_free
    ));
    output.push_str(&format!(
        "ludex_disk_free_bytes{{volume=\"media\"}} {}\n",
        snapshot.disk.media_free
    ));

    output.push_str("# HELP ludex_integrity_findings Findings in the last integrity report.\n");
    output.push_str("# TYPE ludex_integrity_findings gauge\n");
    if let Some(report) = &snapshot.integrity {
        output.push_str(&format!(
            "ludex_integrity_findings{{status=\"{}\"}} {}\n",
            escape(&report.status),
            report.findings
        ));
    }

    output.push_str("# HELP ludex_storage_backend_info Active game archive backend.\n");
    output.push_str("# TYPE ludex_storage_backend_info gauge\n");
    output.push_str(&format!(
        "ludex_storage_backend_info{{backend=\"{}\"}} 1\n",
        state.storage.backend().as_str()
    ));
    output
}

/// Commit exposed by `/api/health/details` and `/metrics`.
#[must_use]
pub fn commit() -> String {
    std::env::var("LUDEX_GIT_COMMIT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

async fn scalar(pool: &Database, sql: &str) -> i64 {
    db::query_scalar::<i64>(sql)
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}

async fn webhook_counts(state: &AppState) -> (i64, i64, i64, i64) {
    let row = db::query_as::<(i64, i64, i64, i64)>(
        "SELECT
            CAST(COALESCE(SUM(CASE WHEN delivered_at IS NOT NULL THEN 1 ELSE 0 END), 0) AS BIGINT),
            CAST(COALESCE(SUM(CASE WHEN delivered_at IS NULL AND next_attempt_at IS NOT NULL THEN 1 ELSE 0 END), 0) AS BIGINT),
            CAST(COALESCE(SUM(CASE WHEN delivered_at IS NULL AND next_attempt_at IS NULL THEN 1 ELSE 0 END), 0) AS BIGINT),
            CAST(COALESCE(SUM(CASE WHEN attempts > 1 THEN attempts - 1 ELSE 0 END), 0) AS BIGINT)
         FROM webhook_deliveries",
    )
    .fetch_one(&state.pool)
    .await;
    match row {
        Ok(values) => values,
        Err(error) => {
            tracing::debug!(%error, "could not read webhook metrics");
            (0, 0, 0, 0)
        }
    }
}

pub(crate) async fn media_usage(media_dir: &std::path::Path) -> (u64, u64) {
    let directory = media_dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut files = 0_u64;
        let mut bytes = 0_u64;
        for entry in walkdir::WalkDir::new(&directory)
            .max_depth(1)
            .into_iter()
            .filter_map(Result::ok)
        {
            if entry.file_type().is_file() {
                files += 1;
                bytes += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            }
        }
        (files, bytes)
    })
    .await
    .unwrap_or((0, 0))
}

pub(crate) async fn database_size(database_url: &str) -> u64 {
    let Some(database) = crate::db::sqlite_path_from_url(database_url) else {
        return 0;
    };
    let mut total = 0;
    if let Ok(metadata) = tokio::fs::metadata(&database).await {
        total += metadata.len();
    }
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = database.clone().into_os_string();
        sidecar.push(suffix);
        if let Ok(metadata) = tokio::fs::metadata(std::path::PathBuf::from(sidecar)).await {
            total += metadata.len();
        }
    }
    total
}

pub(crate) fn free_space(path: &std::path::Path) -> u64 {
    fs2::available_space(path).unwrap_or(0)
}

/// Last integrity report, shared by `/metrics`, health details and the admin
/// observability dashboard.
pub(crate) async fn last_integrity(state: &AppState) -> Option<IntegritySummary> {
    let row = db::query_as::<(i64, String, String, Option<String>, i64)>(
        "SELECT r.id, r.status, r.started_at, r.finished_at,
                (SELECT COUNT(*) FROM integrity_findings f WHERE f.report_id = r.id)
         FROM integrity_reports r
         ORDER BY r.started_at DESC, r.id DESC LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await
    .ok()??;
    let (id, status, started_at, finished_at, findings) = row;
    Some(IntegritySummary {
        id,
        status,
        findings,
        started_at: db::parse_time(&started_at),
        finished_at: finished_at.as_deref().and_then(db::parse_time),
    })
}
