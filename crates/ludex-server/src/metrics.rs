//! Prometheus metrics (#48).
//!
//! The registry is a small hand-rolled structure: per-route counters and a
//! fixed-bucket latency histogram, plus a few live gauges read from SQLite and
//! the media backend when `/metrics` is scraped. No global recorder is
//! installed, which keeps concurrent tests independent and the dependency
//! surface unchanged.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::MatchedPath;
use axum::extract::State;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use subtle::ConstantTimeEq;

use crate::auth::{authenticate_token, bearer_token};
use crate::error::ApiError;
use crate::state::AppState;
use crate::storage::StorageStats;

/// Latency buckets, in seconds.
const DURATION_BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// Content type required by the Prometheus text exposition format.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// One route/status counter key.
type HttpKey = (String, String, u16);

#[derive(Debug, Clone, Copy, Default)]
struct Histogram {
    buckets: [u64; DURATION_BUCKETS.len()],
    sum: f64,
    count: u64,
}

impl Histogram {
    fn observe(&mut self, seconds: f64) {
        self.count += 1;
        self.sum += seconds;
        for (index, bound) in DURATION_BUCKETS.iter().enumerate() {
            if seconds <= *bound {
                self.buckets[index] += 1;
            }
        }
    }
}

#[derive(Debug, Default)]
struct Inner {
    http: BTreeMap<HttpKey, u64>,
    duration: BTreeMap<String, Histogram>,
    webhooks: BTreeMap<&'static str, u64>,
    scans: u64,
    scan_last_ms: f64,
}

/// In-process metrics registry.
#[derive(Debug)]
pub struct Metrics {
    started: Instant,
    active_downloads: AtomicI64,
    inner: Mutex<Inner>,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            active_downloads: AtomicI64::new(0),
            inner: Mutex::new(Inner::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Records one finished HTTP request.
    pub fn record_http(&self, method: &str, route: &str, status: u16, elapsed: Duration) {
        let seconds = elapsed.as_secs_f64();
        let mut inner = self.lock();
        *inner
            .http
            .entry((method.to_string(), route.to_string(), status))
            .or_insert(0) += 1;
        inner
            .duration
            .entry(route.to_string())
            .or_default()
            .observe(seconds);
    }

    /// Marks the start of a download; the returned guard decrements the gauge.
    #[must_use]
    pub fn track_download(&self) -> DownloadGuard<'_> {
        self.active_downloads.fetch_add(1, Ordering::SeqCst);
        DownloadGuard { metrics: self }
    }

    /// Current number of in-flight downloads.
    #[must_use]
    pub fn active_downloads(&self) -> i64 {
        self.active_downloads.load(Ordering::SeqCst)
    }

    /// Records one finished webhook delivery attempt.
    pub fn record_webhook(&self, outcome: &'static str) {
        *self.lock().webhooks.entry(outcome).or_insert(0) += 1;
    }

    /// Records a finished library scan.
    pub fn record_scan(&self, elapsed: Duration) {
        let mut inner = self.lock();
        inner.scans += 1;
        inner.scan_last_ms = elapsed.as_secs_f64() * 1_000.0;
    }

    /// Uptime in seconds.
    #[must_use]
    pub fn uptime_seconds(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// Renders the Prometheus text exposition format.
    #[must_use]
    pub fn render(&self, snapshot: &MetricsSnapshot) -> String {
        let mut out = String::with_capacity(4_096);
        let inner = self.lock();

        heading(&mut out, "ludex_build_info", "gauge", "Build information.");
        out.push_str(&format!(
            "ludex_build_info{{version=\"{}\"}} 1\n",
            escape(env!("CARGO_PKG_VERSION"))
        ));

        heading(&mut out, "ludex_uptime_seconds", "gauge", "Process uptime.");
        out.push_str(&format!(
            "ludex_uptime_seconds {:.3}\n",
            self.uptime_seconds()
        ));

        heading(
            &mut out,
            "ludex_http_requests_total",
            "counter",
            "HTTP requests by method, route template and status.",
        );
        for ((method, route, status), count) in &inner.http {
            out.push_str(&format!(
                "ludex_http_requests_total{{method=\"{}\",route=\"{}\",status=\"{}\"}} {}\n",
                escape(method),
                escape(route),
                status,
                count
            ));
        }

        heading(
            &mut out,
            "ludex_http_request_duration_seconds",
            "histogram",
            "HTTP request latency by route template.",
        );
        for (route, histogram) in &inner.duration {
            let labels = format!("route=\"{}\"", escape(route));
            for (index, bound) in DURATION_BUCKETS.iter().enumerate() {
                out.push_str(&format!(
                    "ludex_http_request_duration_seconds_bucket{{{labels},le=\"{bound}\"}} {}\n",
                    histogram.buckets[index]
                ));
            }
            out.push_str(&format!(
                "ludex_http_request_duration_seconds_bucket{{{labels},le=\"+Inf\"}} {}\n",
                histogram.count
            ));
            out.push_str(&format!(
                "ludex_http_request_duration_seconds_sum{{{labels}}} {:.6}\n",
                histogram.sum
            ));
            out.push_str(&format!(
                "ludex_http_request_duration_seconds_count{{{labels}}} {}\n",
                histogram.count
            ));
        }

        heading(
            &mut out,
            "ludex_active_downloads",
            "gauge",
            "Archive downloads currently streaming.",
        );
        out.push_str(&format!(
            "ludex_active_downloads {}\n",
            self.active_downloads()
        ));

        heading(&mut out, "ludex_games_total", "gauge", "Indexed games.");
        out.push_str(&format!("ludex_games_total {}\n", snapshot.games));

        heading(
            &mut out,
            "ludex_game_versions_total",
            "gauge",
            "Indexed game versions that are still on disk.",
        );
        out.push_str(&format!(
            "ludex_game_versions_total {}\n",
            snapshot.versions
        ));

        heading(&mut out, "ludex_users_total", "gauge", "User accounts.");
        out.push_str(&format!("ludex_users_total {}\n", snapshot.users));

        heading(
            &mut out,
            "ludex_ingest_jobs",
            "gauge",
            "Ingest jobs by state.",
        );
        for (state, count) in &snapshot.ingest {
            out.push_str(&format!(
                "ludex_ingest_jobs{{state=\"{}\"}} {}\n",
                escape(state),
                count
            ));
        }

        if let Some(media) = snapshot.media {
            heading(
                &mut out,
                "ludex_media_cache_files",
                "gauge",
                "Cached artwork files.",
            );
            out.push_str(&format!("ludex_media_cache_files {}\n", media.files));
            heading(
                &mut out,
                "ludex_media_cache_bytes",
                "gauge",
                "Cached artwork size in bytes.",
            );
            out.push_str(&format!("ludex_media_cache_bytes {}\n", media.bytes));
        }

        heading(
            &mut out,
            "ludex_media_backend_info",
            "gauge",
            "Active media storage backend.",
        );
        out.push_str(&format!(
            "ludex_media_backend_info{{backend=\"{}\"}} 1\n",
            escape(snapshot.storage_backend)
        ));

        heading(
            &mut out,
            "ludex_scans_total",
            "counter",
            "Library scans started by this process.",
        );
        out.push_str(&format!("ludex_scans_total {}\n", inner.scans));

        heading(
            &mut out,
            "ludex_scan_duration_seconds",
            "gauge",
            "Duration of the last library scan.",
        );
        out.push_str(&format!(
            "ludex_scan_duration_seconds {:.3}\n",
            inner.scan_last_ms / 1_000.0
        ));

        heading(
            &mut out,
            "ludex_webhook_deliveries_total",
            "counter",
            "Webhook delivery attempts by outcome.",
        );
        for (outcome, count) in &inner.webhooks {
            out.push_str(&format!(
                "ludex_webhook_deliveries_total{{outcome=\"{}\"}} {}\n",
                escape(outcome),
                count
            ));
        }

        heading(
            &mut out,
            "ludex_db_pool_connections",
            "gauge",
            "SQLite pool connections by state.",
        );
        out.push_str(&format!(
            "ludex_db_pool_connections{{state=\"open\"}} {}\n",
            snapshot.pool_size
        ));
        out.push_str(&format!(
            "ludex_db_pool_connections{{state=\"idle\"}} {}\n",
            snapshot.pool_idle
        ));

        out
    }
}

/// Live values injected by the scrape handler.
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    /// Indexed games.
    pub games: i64,
    /// Indexed versions that are not missing.
    pub versions: i64,
    /// User accounts.
    pub users: i64,
    /// Ingest jobs by state.
    pub ingest: Vec<(&'static str, usize)>,
    /// Media cache statistics, when cheap to compute.
    pub media: Option<StorageStats>,
    /// Backend label.
    pub storage_backend: &'static str,
    /// Open pool connections.
    pub pool_size: u32,
    /// Idle pool connections.
    pub pool_idle: u32,
}

fn heading(out: &mut String, name: &str, kind: &str, help: &str) {
    out.push_str(&format!("# HELP {name} {help}\n# TYPE {name} {kind}\n"));
}

fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Decrements the active download gauge when dropped.
pub struct DownloadGuard<'a> {
    metrics: &'a Metrics,
}

impl Drop for DownloadGuard<'_> {
    fn drop(&mut self) {
        self.metrics.active_downloads.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Records every HTTP response. Applied with `Router::layer`, so the matched
/// route template is available even though it is extracted inside the layer.
pub async fn track_http(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_string())
        .unwrap_or_else(|| "unmatched".to_string());
    let started = Instant::now();
    let response = next.run(request).await;
    state.metrics.record_http(
        method.as_str(),
        &route,
        response.status().as_u16(),
        started.elapsed(),
    );
    response
}

/// `GET /metrics`.
pub fn router() -> Router<AppState> {
    Router::new().route("/metrics", get(scrape))
}

async fn scrape(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(error) = authorize(&state, &headers).await {
        return error.into_response();
    }
    let snapshot = match snapshot(&state).await {
        Ok(snapshot) => snapshot,
        Err(error) => return error.into_response(),
    };
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, CONTENT_TYPE)],
        state.metrics.render(&snapshot),
    )
        .into_response()
}

/// `LUDEX_METRICS_TOKEN` when set, otherwise an administrator session or an
/// API key with the `admin` scope.
pub async fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let provided = bearer_token(headers);
    if let Some(expected) = state.config.metrics_token.as_deref() {
        let provided = provided.unwrap_or_default();
        if provided.as_bytes().ct_eq(expected.as_bytes()).into() {
            return Ok(());
        }
        return Err(ApiError::unauthorized());
    }
    let token = provided.ok_or_else(ApiError::unauthorized)?;
    let user = authenticate_token(state, &token).await?;
    user.require_admin_access()
}

async fn snapshot(state: &AppState) -> Result<MetricsSnapshot, ApiError> {
    let games = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM games")
        .fetch_one(&state.pool)
        .await?;
    let versions =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM game_versions WHERE missing = 0")
            .fetch_one(&state.pool)
            .await?;
    let users = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
        .fetch_one(&state.pool)
        .await?;
    let media = state.storage.media_stats().await.unwrap_or_else(|error| {
        tracing::warn!(error = %error, "could not compute media cache statistics");
        None
    });
    Ok(MetricsSnapshot {
        games,
        versions,
        users,
        ingest: crate::ingest::status_counts(&state.config.data_dir),
        media,
        storage_backend: state.storage.kind().as_str(),
        pool_size: state.pool.size(),
        pool_idle: state.pool.num_idle() as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> MetricsSnapshot {
        MetricsSnapshot {
            games: 3,
            versions: 4,
            users: 2,
            ingest: vec![("queued", 1), ("done", 2)],
            media: Some(StorageStats {
                files: 5,
                bytes: 1_024,
            }),
            storage_backend: "local",
            pool_size: 2,
            pool_idle: 1,
        }
    }

    #[test]
    fn renders_counters_gauges_and_histograms() {
        let metrics = Metrics::new();
        metrics.record_http("GET", "/api/health", 200, Duration::from_millis(3));
        metrics.record_http("GET", "/api/health", 500, Duration::from_millis(30));
        metrics.record_webhook("delivered");
        metrics.record_webhook("delivered");
        metrics.record_webhook("retry");
        metrics.record_scan(Duration::from_millis(250));
        let guard = metrics.track_download();
        let text = metrics.render(&snapshot());
        assert!(text.contains("# TYPE ludex_http_requests_total counter"));
        assert!(text.contains(
            "ludex_http_requests_total{method=\"GET\",route=\"/api/health\",status=\"200\"} 1"
        ));
        assert!(text.contains(
            "ludex_http_request_duration_seconds_bucket{route=\"/api/health\",le=\"+Inf\"} 2"
        ));
        assert!(text.contains("ludex_http_request_duration_seconds_count{route=\"/api/health\"} 2"));
        assert!(text.contains("ludex_active_downloads 1"));
        assert!(text.contains("ludex_games_total 3"));
        assert!(text.contains("ludex_game_versions_total 4"));
        assert!(text.contains("ludex_ingest_jobs{state=\"done\"} 2"));
        assert!(text.contains("ludex_media_cache_bytes 1024"));
        assert!(text.contains("ludex_media_backend_info{backend=\"local\"} 1"));
        assert!(text.contains("ludex_scans_total 1"));
        assert!(text.contains("ludex_scan_duration_seconds 0.250"));
        assert!(text.contains("ludex_webhook_deliveries_total{outcome=\"delivered\"} 2"));
        assert!(text.contains("ludex_db_pool_connections{state=\"idle\"} 1"));
        drop(guard);
        assert_eq!(metrics.active_downloads(), 0);
    }

    #[test]
    fn label_values_are_escaped() {
        let metrics = Metrics::new();
        metrics.record_http("GET", "/weird\"path\n", 200, Duration::ZERO);
        let text = metrics.render(&snapshot());
        assert!(text.contains("route=\"/weird\\\"path\\n\""));
    }

    #[test]
    fn histogram_buckets_are_cumulative() {
        let metrics = Metrics::new();
        metrics.record_http("GET", "/api/games", 200, Duration::from_millis(20));
        metrics.record_http("GET", "/api/games", 200, Duration::from_millis(200));
        let text = metrics.render(&snapshot());
        assert!(text.contains(
            "ludex_http_request_duration_seconds_bucket{route=\"/api/games\",le=\"0.005\"} 0"
        ));
        assert!(text.contains(
            "ludex_http_request_duration_seconds_bucket{route=\"/api/games\",le=\"0.025\"} 1"
        ));
        assert!(text.contains(
            "ludex_http_request_duration_seconds_bucket{route=\"/api/games\",le=\"0.25\"} 2"
        ));
    }
}
