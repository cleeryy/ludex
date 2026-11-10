//! Re-run metadata enrichment for every game in the library.
//!
//! This is a maintenance tool: it is useful after changing image sizes or
//! provider fields, when the normal scanner only enriches new games. It uses
//! the same configuration as the server (`LUDEX_*`), opens the existing
//! database/media directory and does not start an HTTP listener.
//!
//! ```bash
//! cargo run -p ludex-server --example refresh_metadata
//! ```
//!
//! Run it while the server is stopped (or accept SQLite WAL concurrency); it
//! writes the same `games` rows and media files as the background enricher.

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use ludex_server::config::Config;
use ludex_server::metadata::IgdbClient;
use ludex_server::models::ScanStatus;
use ludex_server::state::AppState;
use ludex_server::{db, metadata};
use tokio::sync::{mpsc, RwLock};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "refresh_metadata=info,ludex_server=info".into()),
        )
        .init();

    let config = Config::from_env().context("read LUDEX_* configuration")?;
    let pool = db::connect(&config.database_url).await?;
    let http = reqwest::Client::builder()
        .user_agent(concat!("Ludex/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .context("build HTTP client")?;
    let igdb = match (
        config.igdb_client_id.as_deref(),
        config.igdb_client_secret.as_deref(),
    ) {
        (Some(client_id), Some(client_secret))
            if !client_id.trim().is_empty() && !client_secret.trim().is_empty() =>
        {
            Some(IgdbClient::new(
                http.clone(),
                client_id.to_string(),
                client_secret.to_string(),
            ))
        }
        _ => None,
    };
    ludex_server::roots::bootstrap(&pool, &config)
        .await
        .context("bootstrap library roots")?;
    let (scan_tx, _scan_rx) = mpsc::channel(1);
    let state = AppState {
        config: Arc::new(config),
        pool,
        jwt_secret: Arc::new(vec![0_u8; 32]),
        started_at: Instant::now(),
        scan: Arc::new(RwLock::new(ScanStatus::default())),
        scan_tx,
        http,
        igdb,
        oidc: None,
        events: ludex_server::events::channel().0,
        webhooks_notify: Arc::new(tokio::sync::Notify::new()),
        watcher: Arc::new(RwLock::new(ludex_server::watcher::WatcherStatus::default())),
        watcher_handle: Arc::new(RwLock::new(None)),
        integrity_busy: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        media_gc_busy: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };

    let ids = sqlx::query_scalar::<_, i64>("SELECT id FROM games ORDER BY id")
        .fetch_all(&state.pool)
        .await
        .context("list games")?;
    tracing::info!(count = ids.len(), "refreshing metadata");

    for id in ids {
        metadata::enrich_game(&state, id).await;
        tracing::info!(game_id = id, "refresh attempted");
    }
    tracing::info!("metadata refresh complete");
    Ok(())
}
