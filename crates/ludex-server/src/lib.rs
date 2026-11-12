//! Ludex server: axum + SQLite, also embeddable by the desktop app.
//!
//! [`Config::from_env`] plus [`run`] is the standalone binary entry point.
//! [`Config::for_local`] plus [`start`] boots an ephemeral loopback server and
//! returns its URL and a token for the auto-provisioned local administrator.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use axum::extract::DefaultBodyLimit;
use axum::http::header::{
    ACCEPT, ACCEPT_RANGES, AUTHORIZATION, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE,
    CONTENT_TYPE, IF_RANGE, RANGE,
};
use axum::http::{HeaderValue, Request};
use axum::Router;
use ludex_core::models::Role;
use rand::RngCore;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot, watch, RwLock};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::trace::{MakeSpan, TraceLayer};
use tracing::Span;

pub mod achievements;
pub mod api_keys;
pub mod artwork;
pub mod audit;
pub mod auth;
pub mod backup;
pub mod collections;
pub mod community;
pub mod config;
pub mod controller_profiles;
pub mod db;
pub mod desktop_download;
pub mod desktop_handoff;
pub mod emulators;
pub mod error;
pub mod events;
pub mod extract;
pub mod gamevault;
pub mod ingest;
pub mod integrity;
pub mod media;
pub mod media_gc;
pub mod metadata;
pub mod metrics;
pub mod mfa;
pub mod models;
pub mod oidc;
pub mod openapi;
pub mod providers;
pub mod roots;
pub mod routes;
pub mod saves;
pub mod scanner;
pub mod secrets;
pub mod security;
pub mod share;
pub mod sql;
pub mod state;
pub mod storage;
pub mod stream;
pub mod totp;
pub mod uploads;
pub mod watcher;
pub mod web;
pub mod webhooks;

pub use config::Config;

use crate::metadata::IgdbClient;
use crate::models::ScanStatus;
use crate::oidc::OidcClient;
use crate::scanner::ScanRequest;
use crate::state::AppState;
use crate::storage::ArchiveStorage;

/// Handle to a server started with [`start`].
pub struct ServerHandle {
    /// Base URL, e.g. `http://127.0.0.1:43210`.
    pub url: String,
    /// JWT for the auto-provisioned local administrator (empty in server mode
    /// when no bootstrap administrator was configured).
    pub token: String,
    /// Sending on this channel (or dropping it) stops the server gracefully.
    pub shutdown: oneshot::Sender<()>,
}

/// Runs the standalone server until ctrl-c or SIGTERM.
pub async fn run(config: Config) -> Result<()> {
    let prepared = prepare(config).await?;
    tracing::info!(
        url = %prepared.url,
        local_mode = prepared.state.config.local_mode,
        "Ludex server listening"
    );

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = spawn_server(
        prepared.listener,
        prepared.router,
        shutdown_rx,
        prepared.cancel,
    );
    tokio::spawn(ingest::resume_pending(prepared.ingest_ctx));

    shutdown_signal().await;
    tracing::info!("shutting down");
    let _ = shutdown_tx.send(());
    if let Err(error) = server.await {
        tracing::error!(error = %error, "server task failed");
    }
    Ok(())
}

/// Starts the server and returns as soon as the listener is bound.
///
/// Used by the desktop app for its embedded local server.
pub async fn start(config: Config) -> Result<ServerHandle> {
    let prepared = prepare(config).await?;
    tracing::info!(url = %prepared.url, "Ludex server listening");

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    // The server task is detached: dropping the JoinHandle keeps it running.
    let _server = spawn_server(
        prepared.listener,
        prepared.router,
        shutdown_rx,
        prepared.cancel,
    );
    tokio::spawn(ingest::resume_pending(prepared.ingest_ctx));

    Ok(ServerHandle {
        url: prepared.url,
        token: prepared.token,
        shutdown: shutdown_tx,
    })
}

struct Prepared {
    state: AppState,
    listener: TcpListener,
    router: Router,
    url: String,
    token: String,
    cancel: watch::Sender<bool>,
    ingest_ctx: ingest::IngestContext,
}

async fn prepare(config: Config) -> Result<Prepared> {
    // Bind first so a port conflict fails before any database work.
    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("bind {}", config.bind))?;
    let address = listener.local_addr().context("read listener address")?;
    let url = public_url(address);

    tokio::fs::create_dir_all(&config.data_dir)
        .await
        .with_context(|| format!("create data directory {}", config.data_dir.display()))?;
    tokio::fs::create_dir_all(&config.games_dir)
        .await
        .with_context(|| format!("create games directory {}", config.games_dir.display()))?;
    tokio::fs::create_dir_all(&config.media_dir)
        .await
        .with_context(|| format!("create media directory {}", config.media_dir.display()))?;

    let secret = load_or_create_secret(&config).await?;
    let pool = db::connect_with(&config.database_url, config.database_max_connections).await?;
    tracing::info!(
        backend = pool.backend().as_str(),
        url = %db::redact_database_url(&config.database_url),
        max_connections = config.database_max_connections,
        "database ready"
    );
    // Registers `LUDEX_LIBRARY_ROOTS` and backfills the seeded root. Extra
    // roots are never created on disk: a missing path means "not mounted" and
    // must stay detectable by the scanner.
    roots::bootstrap(&pool, &config).await?;
    let http = reqwest::Client::builder()
        .user_agent(concat!("Ludex/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
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

    // Community metadata providers are configured at runtime; a broken row
    // (rotated secret, invalid mapping) is skipped instead of failing startup.
    let providers = providers::load_enabled(&pool, &secret, config.providers_allow_private).await;

    let (scan_tx, scan_rx) = mpsc::channel::<ScanRequest>(4);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    // Settings saved from the admin UI are the fallback; `LUDEX_OIDC_*`
    // environment variables win per field (see `OidcEnvLocked`).
    let stored_oidc = oidc::load_settings(&pool, &secret).await?;
    let resolved_oidc = oidc::resolve_settings(stored_oidc.as_ref(), &config);
    let (oidc, oidc_status) = if resolved_oidc.secret_error {
        // The server secret changed: the stored ciphertext is useless. Keep
        // the server bootable and let an administrator re-enter the secret.
        tracing::warn!(
            "the saved OIDC client secret cannot be decrypted; re-enter it in Admin → Authentication"
        );
        (
            None,
            oidc::OidcStatus {
                ok: false,
                issuer: resolved_oidc.settings.issuer.clone(),
                checked_at: None,
                error: Some(
                    "The saved OIDC client secret cannot be decrypted (server secret changed). \
                     Replace it in the admin UI."
                        .to_string(),
                ),
            },
        )
    } else {
        match OidcClient::from_settings(&resolved_oidc.settings, http.clone()) {
            Ok(client) => {
                let status = oidc::OidcStatus::initial(client.as_ref());
                (client, status)
            }
            // A broken environment configuration keeps failing startup loudly
            // (historical behaviour); a broken saved row only disables SSO.
            Err(error) if config.oidc_env_locked.issuer => return Err(error),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "saved OIDC configuration is invalid; SSO is disabled until it is fixed"
                );
                (
                    None,
                    oidc::OidcStatus {
                        ok: false,
                        issuer: resolved_oidc.settings.issuer.clone(),
                        checked_at: None,
                        error: Some(format!(
                            "The saved OIDC configuration is invalid: {error:#}"
                        )),
                    },
                )
            }
        }
    };
    let (events, event_rx) = events::channel();
    let ingest_ctx = ingest::IngestContext {
        games_dir: config.games_dir.clone(),
        data_dir: config.data_dir.clone(),
        scan_tx: scan_tx.clone(),
        allow_private: config.ingest_allow_private,
        events: Some(events.clone()),
    };
    let ingest_handle = ingest::handle(&ingest_ctx);

    let state = AppState {
        config: Arc::new(config.clone()),
        pool,
        jwt_secret: Arc::new(secret),
        started_at: Instant::now(),
        scan: Arc::new(RwLock::new(ScanStatus::default())),
        scan_tx: scan_tx.clone(),
        http,
        igdb,
        providers: Arc::new(RwLock::new(providers)),
        oidc: Arc::new(RwLock::new(oidc)),
        oidc_status: Arc::new(RwLock::new(oidc_status)),
        events,
        webhooks_notify: Arc::new(tokio::sync::Notify::new()),
        watcher: Arc::new(RwLock::new(watcher::WatcherStatus::default())),
        watcher_handle: Arc::new(RwLock::new(None)),
        integrity_busy: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        media_gc_busy: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        achievement_busy: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        storage: ArchiveStorage::from_config(&config),
        metrics: Arc::new(metrics::Metrics::default()),
        ingest: Some(ingest_handle),
        rate_limiter: Arc::new(security::RateLimiter::new(
            config.rate_limit_rps,
            config.rate_limit_burst,
        )),
        login_guard: Arc::new(security::LoginGuard::new(
            config.login_max_attempts,
            config.login_lockout,
            config.login_backoff,
        )),
        session_touch: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        desktop_handoffs: Arc::new(desktop_handoff::DesktopHandoffs::new(
            config.desktop_handoff_ttl,
        )),
        exchange_codes: Arc::new(desktop_handoff::ExchangeCodes::new()),
        handoff_create_limits: Arc::new(security::AttemptCounter::new(
            config.handoff_create_limit,
            Duration::from_secs(60),
        )),
        handoff_claim_limits: Arc::new(security::AttemptCounter::new(
            config.handoff_claim_limit,
            Duration::from_secs(60),
        )),
        invitation_limits: Arc::new(security::AttemptCounter::new(
            config.invitation_attempt_limit,
            Duration::from_secs(600),
        )),
        mfa_attempt_limits: Arc::new(security::AttemptCounter::new(
            config.mfa_attempt_limit,
            Duration::from_secs(300),
        )),
    };

    let watcher_handle = watcher::start(&state).await;
    *state.watcher_handle.write().await = watcher_handle;

    let token = bootstrap_admin(&state).await?;
    let router = build_router(&state, &ingest_ctx);

    tokio::spawn(scanner::worker(state.clone(), scan_rx, cancel_rx.clone()));
    tokio::spawn(events::worker(state.clone(), event_rx));
    tokio::spawn(webhooks::worker(state.clone()));
    tokio::spawn(integrity::scheduled_worker(
        state.clone(),
        cancel_rx.clone(),
    ));
    tokio::spawn(media_gc::scheduled_worker(state.clone(), cancel_rx.clone()));
    tokio::spawn(achievements::scheduled_worker(state.clone(), cancel_rx));
    tokio::spawn(uploads::cleanup_worker(state.clone()));
    // Initial scan in the background; never blocks startup.
    let _ = state.scan_tx.try_send(ScanRequest::all());

    Ok(Prepared {
        state,
        listener,
        router,
        url,
        token,
        cancel: cancel_tx,
        ingest_ctx,
    })
}

fn spawn_server(
    listener: TcpListener,
    router: Router,
    shutdown_rx: oneshot::Receiver<()>,
    cancel: watch::Sender<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let result = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        })
        .await;
        if let Err(error) = result {
            tracing::error!(error = %error, "HTTP server stopped with an error");
        }
        let _ = cancel.send(true);
    })
}

fn build_router(state: &AppState, ingest_ctx: &ingest::IngestContext) -> Router {
    let api = routes::api_router(state.config.saves_max_bytes).layer(
        axum::middleware::from_fn_with_state(state.clone(), security::rate_limit),
    );
    let app = Router::new()
        .nest("/api", api)
        .route("/metrics", axum::routing::get(routes::metrics::metrics));
    let app = web::attach(app, state);
    let app = app.with_state(state.clone());

    // The ingest router is owned by another crate module and does not enforce
    // authentication itself: require an admin/editor JWT for every route.
    let ingest_router = ingest::router(ingest_ctx.clone()).layer(
        axum::middleware::from_fn_with_state(state.clone(), auth::editor_only),
    );
    let app = app.merge(ingest_router);

    // Apply cross-cutting layers after merging so every route (including
    // ingest) gets CORS, tracing and the body limit.
    app.layer(DefaultBodyLimit::max(config::MAX_JSON_BYTES))
        .layer(cors_layer())
        .layer(TraceLayer::new_for_http().make_span_with(RequestSpan))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            metrics::track,
        ))
}

/// Trace span that records the path but never the query string: download
/// tickets are passed in `?token=` and must not end up in logs.
#[derive(Clone, Copy, Default)]
struct RequestSpan;

impl<B> MakeSpan<B> for RequestSpan {
    fn make_span(&mut self, request: &Request<B>) -> Span {
        tracing::info_span!(
            "http",
            method = %request.method(),
            path = %request.uri().path()
        )
    }
}

fn cors_layer() -> CorsLayer {
    let origins: Vec<HeaderValue> = [
        "tauri://localhost",
        "https://tauri.localhost",
        "http://tauri.localhost",
        "http://localhost:5173",
        "http://127.0.0.1:5173",
    ]
    .into_iter()
    .filter_map(|origin| HeaderValue::from_str(origin).ok())
    .collect();

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods(Any)
        .allow_headers([AUTHORIZATION, CONTENT_TYPE, ACCEPT, RANGE, IF_RANGE])
        .expose_headers([
            CONTENT_RANGE,
            ACCEPT_RANGES,
            CONTENT_LENGTH,
            CONTENT_DISPOSITION,
        ])
}

fn public_url(address: SocketAddr) -> String {
    let host = if address.ip().is_unspecified() {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
        address.ip()
    };
    format!("http://{}", SocketAddr::new(host, address.port()))
}

async fn load_or_create_secret(config: &Config) -> Result<Vec<u8>> {
    if let Some(secret) = config.jwt_secret.as_deref() {
        if secret.len() < 16 {
            anyhow::bail!("LUDEX_JWT_SECRET must be at least 16 bytes");
        }
        return Ok(secret.as_bytes().to_vec());
    }

    let path = config.data_dir.join("jwt.secret");
    if let Ok(existing) = tokio::fs::read(&path).await {
        if existing.len() >= 32 {
            set_private_permissions(&path).await?;
            return Ok(existing);
        }
    }

    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let encoded = hex::encode(bytes);
    tokio::fs::write(&path, encoded.as_bytes())
        .await
        .with_context(|| format!("write JWT secret {}", path.display()))?;
    set_private_permissions(&path).await?;
    Ok(encoded.into_bytes())
}

async fn set_private_permissions(path: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .await
            .with_context(|| format!("restrict permissions on {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

async fn bootstrap_admin(state: &AppState) -> Result<String> {
    if state.config.local_mode {
        if let Some((id, role)) = db::query_as::<(i64, String)>(
            "SELECT id, role FROM users WHERE role = 'admin' AND activated = 1
             ORDER BY id LIMIT 1",
        )
        .fetch_optional(&state.pool)
        .await
        .context("find local administrator")?
        {
            let role = role.parse::<Role>().unwrap_or(Role::Admin);
            return auth::issue_access_token(&state.jwt_secret, id, role);
        }
        let username = unique_username(state, "ludex").await?;
        let password = auth::generate_password();
        let id = create_user(state, &username, &password, Role::Admin).await?;
        state.events.emit(
            events::Event::new(audit::actions::USER_CREATED, events::Actor::System)
                .target("user", id.to_string())
                .metadata(serde_json::json!({
                    "username": username,
                    "role": Role::Admin.as_str(),
                    "source": "local_bootstrap",
                })),
        );
        tracing::info!(username = %username, "provisioned local administrator");
        return auth::issue_access_token(&state.jwt_secret, id, Role::Admin);
    }

    let users = db::count_users(&state.pool).await?;
    if users > 0 {
        return Ok(String::new());
    }
    let (Some(username), Some(password)) = (
        state.config.admin_username.as_deref(),
        state.config.admin_password.as_deref(),
    ) else {
        return Ok(String::new());
    };
    if password.chars().count() < 8 {
        tracing::warn!("LUDEX_ADMIN_PASSWORD is shorter than 8 characters; bootstrap skipped");
        return Ok(String::new());
    }
    let username = username.trim();
    if username.is_empty() {
        return Ok(String::new());
    }
    let id = create_user(state, username, password, Role::Admin).await?;
    state.events.emit(
        events::Event::new(audit::actions::USER_CREATED, events::Actor::System)
            .target("user", id.to_string())
            .metadata(serde_json::json!({
                "username": username,
                "role": Role::Admin.as_str(),
                "source": "bootstrap",
            })),
    );
    tracing::info!(username = %username, "provisioned bootstrap administrator");
    auth::issue_access_token(&state.jwt_secret, id, Role::Admin)
}

pub(crate) async fn unique_username(state: &AppState, base: &str) -> Result<String> {
    for attempt in 0..100 {
        let candidate = if attempt == 0 {
            base.to_string()
        } else {
            format!("{base}-{}", attempt + 1)
        };
        let exists = db::query_scalar::<i64>("SELECT COUNT(*) FROM users WHERE username = ?")
            .bind(&candidate)
            .fetch_one(&state.pool)
            .await
            .context("check username availability")?;
        if exists == 0 {
            return Ok(candidate);
        }
    }
    anyhow::bail!("could not allocate a local administrator username")
}

async fn create_user(state: &AppState, username: &str, password: &str, role: Role) -> Result<i64> {
    let password_hash = auth::hash_password(password)?;
    let now = db::db_time(db::now());
    db::query(
        "INSERT INTO users
            (username, password_hash, email, role, activated, avatar_path, created_at, updated_at)
         VALUES (?, ?, NULL, ?, 1, NULL, ?, ?)",
    )
    .bind(username)
    .bind(password_hash)
    .bind(role.as_str())
    .bind(&now)
    .bind(&now)
    .returning_id(&state.pool)
    .await
    .context("create administrator")
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}

/// Absolute path helper used by the desktop app when it needs the database file.
#[must_use]
pub fn database_path(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("ludex.db")
}
