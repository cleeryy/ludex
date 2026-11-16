//! Shared application state handed to every axum handler.
//!
//! [`AppState`] is a thin facade over six subsystem structs (#237). Each one
//! groups the fields a single concern owns, so a handler depends on
//! `state.core.pool` or `state.transfer.storage` instead of one 40-field
//! struct, and wiring a feature means touching one subsystem rather than the
//! whole state.

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
    /// Configuration, database, HTTP client and the event bus.
    pub core: Arc<Core>,
    /// Accounts, tokens, sessions and every request throttle.
    pub auth: Arc<Auth>,
    /// Library scanning and metadata enrichment.
    pub catalog: Arc<Catalog>,
    /// Archive storage, transfer leases and the extracted cache.
    pub transfer: Arc<Transfer>,
    /// Optional outbound integrations.
    pub integrations: Arc<Integrations>,
    /// Metrics, watcher, ingest and the background-worker busy flags.
    pub ops: Arc<Ops>,
}

/// Startup configuration and the resources every subsystem needs.
pub struct Core {
    /// Immutable startup configuration.
    pub config: Arc<Config>,
    /// Database pool (SQLite or PostgreSQL).
    pub pool: Database,
    /// Process start, used by the health endpoint.
    pub started_at: Instant,
    /// Shared HTTP client for metadata and artwork.
    pub http: reqwest::Client,
    /// Audit + webhook event bus.
    pub events: EventSink,
}

/// Authentication, authorization and throttling state.
pub struct Auth {
    /// JWT signing secret.
    pub jwt_secret: Arc<Vec<u8>>,
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
    /// Per-address throttle for anonymous diagnostics reports (#171).
    pub diagnostics_limits: Arc<crate::security::AttemptCounter>,
    /// Per-address throttle for invitation redemption (#166).
    pub invitation_limits: Arc<crate::security::AttemptCounter>,
    /// Per-address-and-account throttle for second-factor attempts (#166).
    pub mfa_attempt_limits: Arc<crate::security::AttemptCounter>,
    /// Per-account throttle for handoff creation (#157).
    pub handoff_create_limits: Arc<crate::security::AttemptCounter>,
    /// Per-address throttle for handoff claims (#157).
    pub handoff_claim_limits: Arc<crate::security::AttemptCounter>,
    /// Per-account throttle for friend requests (#161).
    pub friend_request_limits: Arc<crate::security::AttemptCounter>,
    /// Per-account throttle for device heartbeats (#184).
    pub device_heartbeat_limits: Arc<crate::security::AttemptCounter>,
}

/// Library catalog: scanning, metadata providers and content hashing.
pub struct Catalog {
    /// Current scan status.
    pub scan: Arc<RwLock<ScanStatus>>,
    /// Queue consumed by the library scan worker.
    pub scan_tx: mpsc::Sender<ScanRequest>,
    /// IGDB client, present when credentials are configured.
    pub igdb: Option<IgdbClient>,
    /// Enabled metadata providers, ordered by priority. Reloaded by the admin
    /// API after a create/update/delete/reorder; read by enrichment.
    pub providers: Arc<RwLock<Vec<crate::providers::MetadataProviderConfig>>>,
    /// Full archive SHA-256 backfill state (#216).
    pub content_hashes: Arc<crate::content_hash::ContentHasher>,
}

/// Archive storage and in-flight transfer bookkeeping.
pub struct Transfer {
    /// Game archive storage (local disk or S3-compatible bucket).
    pub storage: ArchiveStorage,
    /// In-flight archive reads, used to refuse deleting a version in use (#168).
    pub transfers: Arc<ActiveTransfers>,
    /// Download admission: the per-address/version reconnect budget for
    /// single-stream clients (#195) plus job-scoped request and concurrent-part
    /// budgets for parallel ranged downloads (#217).
    pub download_limits: Arc<crate::download_limits::DownloadLimiter>,
    /// Server-side extracted cache and file manifest transport (#231).
    pub extracted: Arc<crate::extracted_cache::ExtractedCache>,
}

/// Optional outbound and enterprise integrations.
pub struct Integrations {
    /// OIDC client, atomically replaced when an administrator saves new SSO
    /// settings. `None` means SSO is disabled.
    pub oidc: Arc<RwLock<Option<OidcClient>>>,
    /// Last discovery check, shown by the admin Authentication page.
    pub oidc_status: Arc<RwLock<OidcStatus>>,
    /// VirusTotal lookup scanner: injectable transport, single-flight table and
    /// the in-memory per-minute window (#224).
    pub virustotal: Arc<crate::virustotal::Scanner>,
    /// Wakes the webhook delivery worker when a delivery is queued.
    pub webhooks_notify: Arc<tokio::sync::Notify>,
}

/// Operational state: observability, background workers and their guards.
pub struct Ops {
    /// In-process Prometheus registry.
    pub metrics: Arc<Metrics>,
    /// Ingest manager, used by health details and metrics.
    pub ingest: Option<IngestHandle>,
    /// Live folder watcher status.
    pub watcher: Arc<RwLock<WatcherStatus>>,
    /// Handle used to reload the watcher after a root change.
    pub watcher_handle: Arc<RwLock<Option<WatcherHandle>>>,
    /// Guards against two concurrent integrity reports.
    pub integrity_busy: Arc<AtomicBool>,
    /// Guards against two concurrent media garbage collections.
    pub media_gc_busy: Arc<AtomicBool>,
    /// Guards against two concurrent Library Health Center reports (#165).
    pub library_health_busy: Arc<AtomicBool>,
    /// Guards against two concurrent achievement scans.
    pub achievement_busy: Arc<AtomicBool>,
    /// Guards against two concurrent backup, verify or restore runs (#169).
    pub backup_busy: Arc<AtomicBool>,
}

/// Counts the handlers currently reading each version's archive (#168).
///
/// A `DELETE` is refused while a download or stream still holds a lease. The
/// registry is process-local: a multi-instance deployment would need a shared
/// store, but the file also stays readable through the open handle on Unix.
#[derive(Debug, Default)]
pub struct ActiveTransfers {
    inner: Mutex<HashMap<i64, usize>>,
}

impl ActiveTransfers {
    /// Registers one reader and returns a lease that deregisters on drop.
    pub fn begin(self: &Arc<Self>, version_id: i64) -> TransferLease {
        if let Ok(mut active) = self.inner.lock() {
            *active.entry(version_id).or_insert(0) += 1;
        }
        TransferLease {
            registry: Arc::clone(self),
            version_id,
        }
    }

    /// Whether any handler currently reads this version.
    #[must_use]
    pub fn is_active(&self, version_id: i64) -> bool {
        self.inner
            .lock()
            .map(|active| active.get(&version_id).is_some_and(|count| *count > 0))
            .unwrap_or(false)
    }
}

/// RAII lease handed out by [`ActiveTransfers::begin`].
#[derive(Debug)]
pub struct TransferLease {
    registry: Arc<ActiveTransfers>,
    version_id: i64,
}

impl Drop for TransferLease {
    fn drop(&mut self) {
        if let Ok(mut active) = self.registry.inner.lock() {
            if let Some(count) = active.get_mut(&self.version_id) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    active.remove(&self.version_id);
                }
            }
        }
    }
}

impl AppState {
    /// Uptime in seconds.
    #[must_use]
    pub fn uptime_seconds(&self) -> u64 {
        self.core.started_at.elapsed().as_secs()
    }

    /// Whether the IGDB provider is available.
    #[must_use]
    pub fn igdb_enabled(&self) -> bool {
        self.catalog.igdb.is_some()
    }

    /// Snapshot of the current OIDC client. Cloning it is cheap (the flow
    /// state is shared), and the read lock is never held across an `await`.
    pub async fn oidc_client(&self) -> Option<OidcClient> {
        self.integrations.oidc.read().await.clone()
    }
}

#[cfg(test)]
impl AppState {
    /// Swaps the configuration of a state no one else holds yet.
    ///
    /// Tests rebuild the same state with a different config; the subsystem is
    /// behind an `Arc`, so the swap is only possible before it is cloned.
    pub fn set_config(&mut self, config: Config) {
        Arc::get_mut(&mut self.core)
            .expect("state is still uniquely owned")
            .config = Arc::new(config);
    }

    /// Swaps the archive storage of a state no one else holds yet.
    pub fn set_storage(&mut self, storage: ArchiveStorage) {
        Arc::get_mut(&mut self.transfer)
            .expect("state is still uniquely owned")
            .storage = storage;
    }

    /// Swaps the VirusTotal scanner of a state no one else holds yet.
    pub fn set_virustotal(&mut self, scanner: Arc<crate::virustotal::Scanner>) {
        Arc::get_mut(&mut self.integrations)
            .expect("state is still uniquely owned")
            .virustotal = scanner;
    }

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
        let diagnostics_limits = crate::security::AttemptCounter::new(
            config.diagnostics_rate_limit,
            std::time::Duration::from_secs(3_600),
        );
        let invitation_limits = crate::security::AttemptCounter::new(
            config.invitation_attempt_limit,
            std::time::Duration::from_secs(600),
        );
        let mfa_attempt_limits = crate::security::AttemptCounter::new(
            config.mfa_attempt_limit,
            std::time::Duration::from_secs(300),
        );
        let http = reqwest::Client::new();
        let virustotal = Arc::new(crate::virustotal::Scanner::new(&config, http.clone()));
        let extracted = Arc::new(crate::extracted_cache::ExtractedCache::new(&config));
        Self {
            core: Arc::new(Core {
                config: Arc::new(config),
                pool,
                started_at: Instant::now(),
                http,
                events: crate::events::channel().0,
            }),
            auth: Arc::new(Auth {
                jwt_secret: Arc::new(vec![7_u8; 32]),
                rate_limiter: Arc::new(rate_limiter),
                login_guard: Arc::new(login_guard),
                session_touch: Arc::new(Mutex::new(HashMap::new())),
                desktop_handoffs: Arc::new(desktop_handoffs),
                exchange_codes: Arc::new(crate::desktop_handoff::ExchangeCodes::new()),
                diagnostics_limits: Arc::new(diagnostics_limits),
                invitation_limits: Arc::new(invitation_limits),
                mfa_attempt_limits: Arc::new(mfa_attempt_limits),
                handoff_create_limits: Arc::new(handoff_create_limits),
                handoff_claim_limits: Arc::new(handoff_claim_limits),
                friend_request_limits: Arc::new(crate::security::AttemptCounter::new(
                    crate::social::FRIEND_REQUEST_LIMIT_PER_HOUR,
                    std::time::Duration::from_secs(3600),
                )),
                device_heartbeat_limits: Arc::new(crate::security::AttemptCounter::new(
                    crate::devices::HEARTBEAT_LIMIT_PER_MINUTE,
                    std::time::Duration::from_secs(60),
                )),
            }),
            catalog: Arc::new(Catalog {
                scan: Arc::new(RwLock::new(ScanStatus::default())),
                scan_tx,
                igdb: None,
                providers: Arc::new(RwLock::new(Vec::new())),
                content_hashes: Arc::new(crate::content_hash::ContentHasher::default()),
            }),
            transfer: Arc::new(Transfer {
                storage,
                transfers: Arc::new(ActiveTransfers::default()),
                download_limits: Arc::new(crate::download_limits::DownloadLimiter::new()),
                extracted,
            }),
            integrations: Arc::new(Integrations {
                oidc: Arc::new(RwLock::new(None)),
                oidc_status: Arc::new(RwLock::new(OidcStatus::default())),
                virustotal,
                webhooks_notify: Arc::new(tokio::sync::Notify::new()),
            }),
            ops: Arc::new(Ops {
                metrics: Arc::new(Metrics::default()),
                ingest: None,
                watcher: Arc::new(RwLock::new(WatcherStatus::default())),
                watcher_handle: Arc::new(RwLock::new(None)),
                integrity_busy: Arc::new(AtomicBool::new(false)),
                media_gc_busy: Arc::new(AtomicBool::new(false)),
                library_health_busy: Arc::new(AtomicBool::new(false)),
                achievement_busy: Arc::new(AtomicBool::new(false)),
                backup_busy: Arc::new(AtomicBool::new(false)),
            }),
        }
    }
}
