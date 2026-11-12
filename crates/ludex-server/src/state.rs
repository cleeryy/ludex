//! Shared application state handed to every axum handler.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::db::Database;
use tokio::sync::{mpsc, RwLock};

use crate::config::Config;
use crate::events::EventSink;
use crate::ingest::IngestHandle;
use crate::metadata::IgdbClient;
use crate::metrics::Metrics;
use crate::models::ScanStatus;
use crate::oidc::{OidcClient, OidcStatus};
use crate::scanner::ScanRequest;
use crate::storage::ArchiveStorage;
use crate::watcher::{WatcherHandle, WatcherStatus};

/// State shared by all routes and background tasks.
#[derive(Clone)]
pub struct AppState {
    /// Immutable startup configuration.
    pub config: Arc<Config>,
    /// Database pool (SQLite or PostgreSQL).
    pub pool: Database,
    /// JWT signing secret.
    pub jwt_secret: Arc<Vec<u8>>,
    /// Process start, used by the health endpoint.
    pub started_at: Instant,
    /// Current scan status.
    pub scan: Arc<RwLock<ScanStatus>>,
    /// Queue consumed by the library scan worker.
    pub scan_tx: mpsc::Sender<ScanRequest>,
    /// Shared HTTP client for metadata and artwork.
    pub http: reqwest::Client,
    /// IGDB client, present when credentials are configured.
    pub igdb: Option<IgdbClient>,
    /// Enabled metadata providers, ordered by priority. Reloaded by the admin
    /// API after a create/update/delete/reorder; read by enrichment.
    pub providers: Arc<RwLock<Vec<crate::providers::MetadataProviderConfig>>>,
    /// OIDC client, atomically replaced when an administrator saves new SSO
    /// settings. `None` means SSO is disabled.
    pub oidc: Arc<RwLock<Option<OidcClient>>>,
    /// Last discovery check, shown by the admin Authentication page.
    pub oidc_status: Arc<RwLock<OidcStatus>>,
    /// Audit + webhook event bus.
    pub events: EventSink,
    /// Wakes the webhook delivery worker when a delivery is queued.
    pub webhooks_notify: Arc<tokio::sync::Notify>,
    /// Live folder watcher status.
    pub watcher: Arc<RwLock<WatcherStatus>>,
    /// Handle used to reload the watcher after a root change.
    pub watcher_handle: Arc<RwLock<Option<WatcherHandle>>>,
    /// Guards against two concurrent integrity reports.
    pub integrity_busy: Arc<AtomicBool>,
    /// Guards against two concurrent media garbage collections.
    pub media_gc_busy: Arc<AtomicBool>,
    /// Guards against two concurrent achievement scans.
    pub achievement_busy: Arc<AtomicBool>,
    /// Guards against two concurrent backup, verify or restore runs (#169).
    pub backup_busy: Arc<AtomicBool>,
    /// Game archive storage (local disk or S3-compatible bucket).
    pub storage: ArchiveStorage,
    /// In-process Prometheus registry.
    pub metrics: Arc<Metrics>,
    /// Ingest manager, used by health details and metrics.
    pub ingest: Option<IngestHandle>,
    /// Per-IP API rate limiter, configured by `LUDEX_RATE_LIMIT_*`.
    pub rate_limiter: Arc<crate::security::RateLimiter>,
    /// Per-IP+username login throttle and lockout.
    pub login_guard: Arc<crate::security::LoginGuard>,
    /// Last `auth_sessions.last_seen_at` refresh per token, in Unix seconds.
    pub session_touch: Arc<Mutex<HashMap<String, i64>>>,
    /// Pending desktop SSO handoffs, keyed by the desktop-generated state (#157).
    pub desktop_handoffs: Arc<crate::desktop_handoff::DesktopHandoffs>,
    /// Short-lived one-time login exchange codes (web OIDC and desktop handoff).
    pub exchange_codes: Arc<crate::desktop_handoff::ExchangeCodes>,
    /// Per-account throttle for handoff creation (#157).
    pub handoff_create_limits: Arc<crate::security::AttemptCounter>,
    /// Per-address throttle for handoff claims (#157).
    pub handoff_claim_limits: Arc<crate::security::AttemptCounter>,
}

impl AppState {
    /// Uptime in seconds.
    #[must_use]
    pub fn uptime_seconds(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    /// Whether the IGDB provider is available.
    #[must_use]
    pub fn igdb_enabled(&self) -> bool {
        self.igdb.is_some()
    }

    /// Snapshot of the current OIDC client. Cloning it is cheap (the flow
    /// state is shared), and the read lock is never held across an `await`.
    pub async fn oidc_client(&self) -> Option<OidcClient> {
        self.oidc.read().await.clone()
    }
}

#[cfg(test)]
impl AppState {
    /// Minimal state for unit tests: real SQLite database, no network clients.
    ///
    /// The caller owns `scan_tx`, which makes scan triggers observable.
    pub async fn for_tests(config: Config, scan_tx: mpsc::Sender<ScanRequest>) -> Self {
        let pool = crate::db::connect_for_tests(&config.database_url)
            .await
            .expect("connect test database");
        crate::roots::bootstrap(&pool, &config)
            .await
            .expect("bootstrap test roots");
        let storage = ArchiveStorage::from_config(&config);
        let rate_limiter =
            crate::security::RateLimiter::new(config.rate_limit_rps, config.rate_limit_burst);
        let login_guard = crate::security::LoginGuard::new(
            config.login_max_attempts,
            config.login_lockout,
            config.login_backoff,
        );
        let desktop_handoffs =
            crate::desktop_handoff::DesktopHandoffs::new(config.desktop_handoff_ttl);
        let handoff_create_limits = crate::security::AttemptCounter::new(
            config.handoff_create_limit,
            std::time::Duration::from_secs(60),
        );
        let handoff_claim_limits = crate::security::AttemptCounter::new(
            config.handoff_claim_limit,
            std::time::Duration::from_secs(60),
        );
        Self {
            config: Arc::new(config),
            pool,
            jwt_secret: Arc::new(vec![7_u8; 32]),
            started_at: Instant::now(),
            scan: Arc::new(RwLock::new(ScanStatus::default())),
            scan_tx,
            http: reqwest::Client::new(),
            igdb: None,
            providers: Arc::new(RwLock::new(Vec::new())),
            oidc: Arc::new(RwLock::new(None)),
            oidc_status: Arc::new(RwLock::new(OidcStatus::default())),
            events: crate::events::channel().0,
            webhooks_notify: Arc::new(tokio::sync::Notify::new()),
            watcher: Arc::new(RwLock::new(WatcherStatus::default())),
            watcher_handle: Arc::new(RwLock::new(None)),
            integrity_busy: Arc::new(AtomicBool::new(false)),
            media_gc_busy: Arc::new(AtomicBool::new(false)),
            achievement_busy: Arc::new(AtomicBool::new(false)),
            backup_busy: Arc::new(AtomicBool::new(false)),
            storage,
            metrics: Arc::new(Metrics::default()),
            ingest: None,
            rate_limiter: Arc::new(rate_limiter),
            login_guard: Arc::new(login_guard),
            session_touch: Arc::new(Mutex::new(HashMap::new())),
            desktop_handoffs: Arc::new(desktop_handoffs),
            exchange_codes: Arc::new(crate::desktop_handoff::ExchangeCodes::new()),
            handoff_create_limits: Arc::new(handoff_create_limits),
            handoff_claim_limits: Arc::new(handoff_claim_limits),
        }
    }
}
