//! Shared application state handed to every axum handler.

use std::sync::Arc;
use std::time::Instant;

use sqlx::SqlitePool;
use tokio::sync::{mpsc, RwLock};

use crate::config::Config;
use crate::metadata::IgdbClient;
use crate::models::ScanStatus;

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
    pub scan_tx: mpsc::Sender<()>,
    /// Shared HTTP client for metadata and artwork.
    pub http: reqwest::Client,
    /// IGDB client, present when credentials are configured.
    pub igdb: Option<IgdbClient>,
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
