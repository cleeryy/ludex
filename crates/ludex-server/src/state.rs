//! Shared application state handed to every axum handler.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use sqlx::SqlitePool;
use tokio::sync::{mpsc, RwLock};

use crate::config::Config;
use crate::events::EventSink;
use crate::metadata::IgdbClient;
use crate::models::ScanStatus;
use crate::oidc::OidcClient;
use crate::scanner::ScanRequest;
use crate::watcher::{WatcherHandle, WatcherStatus};

/// State shared by all routes and background tasks.
#[derive(Clone)]
pub struct AppState {
    /// Immutable startup configuration.
    pub config: Arc<Config>,
    /// SQLite pool.
    pub pool: SqlitePool,
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
    /// OIDC client, present when SSO is configured.
    pub oidc: Option<OidcClient>,
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
}

#[cfg(test)]
impl AppState {
    /// Minimal state for unit tests: real SQLite database, no network clients.
    ///
    /// The caller owns `scan_tx`, which makes scan triggers observable.
    pub async fn for_tests(config: Config, scan_tx: mpsc::Sender<ScanRequest>) -> Self {
        let pool = crate::db::connect(&config.database_url)
            .await
            .expect("connect test database");
        crate::roots::bootstrap(&pool, &config)
            .await
            .expect("bootstrap test roots");
        Self {
            config: Arc::new(config),
            pool,
            jwt_secret: Arc::new(vec![7_u8; 32]),
            started_at: Instant::now(),
            scan: Arc::new(RwLock::new(ScanStatus::default())),
            scan_tx,
            http: reqwest::Client::new(),
            igdb: None,
            oidc: None,
            events: crate::events::channel().0,
            webhooks_notify: Arc::new(tokio::sync::Notify::new()),
            watcher: Arc::new(RwLock::new(WatcherStatus::default())),
            watcher_handle: Arc::new(RwLock::new(None)),
            integrity_busy: Arc::new(AtomicBool::new(false)),
            media_gc_busy: Arc::new(AtomicBool::new(false)),
        }
    }
}
