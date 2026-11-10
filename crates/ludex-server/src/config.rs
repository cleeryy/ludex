//! Server configuration, loaded from the environment or built for the desktop
//! app's embedded local mode.

use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use ludex_core::models::Role;

/// Default bind address when `LUDEX_BIND` is unset.
pub const DEFAULT_BIND: &str = "0.0.0.0:8080";
/// Default scan interval, in seconds.
pub const DEFAULT_SCAN_INTERVAL_SECONDS: u64 = 300;
/// Default idle window before the folder watcher triggers a scan.
pub const DEFAULT_WATCHER_DEBOUNCE_MS: u64 = 5_000;
/// Default interval between scheduled integrity checks (one week).
pub const DEFAULT_INTEGRITY_INTERVAL_HOURS: u64 = 168;
/// Default grace period before an unreferenced media file is deleted.
pub const DEFAULT_MEDIA_GC_GRACE_HOURS: u64 = 24;
/// Maximum JSON request body accepted by the API (10 MiB).
pub const MAX_JSON_BYTES: usize = 10 * 1024 * 1024;
/// Default maximum size of a single cloud save upload (2 GiB).
pub const DEFAULT_SAVES_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Default S3 region when `LUDEX_S3_REGION` is unset.
pub const DEFAULT_S3_REGION: &str = "us-east-1";
/// Providers enabled when `LUDEX_ACHIEVEMENT_PROVIDERS` is unset.
pub const DEFAULT_ACHIEVEMENT_PROVIDERS: &[&str] =
    &["game_files", "gog_galaxy_api", "gog_galaxy_sqlite"];
/// Loopback JSON patterns tried by the `gog_galaxy_api` provider.
pub const DEFAULT_ACHIEVEMENT_API_URLS: &[&str] =
    &["http://localhost:29662/api/achievements?appId={appId}"];

/// Where game archives are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StorageBackend {
    /// Archives are read from the library roots on local disk (default).
    #[default]
    Local,
    /// Archives are uploaded to an S3-compatible bucket and proxied on read.
    S3,
}

impl StorageBackend {
    /// Stable identifier used in the database and the API.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::S3 => "s3",
        }
    }
}

impl FromStr for StorageBackend {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "local" | "filesystem" | "disk" => Ok(Self::Local),
            "s3" | "minio" | "garage" | "r2" | "b2" => Ok(Self::S3),
            other => anyhow::bail!("unknown storage backend `{other}`; expected `local` or `s3`"),
        }
    }
}

impl std::fmt::Display for StorageBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// S3-compatible object store credentials and addressing.
#[derive(Clone)]
pub struct S3Config {
    /// Custom endpoint (`https://minio.lan:9000`). `None` uses AWS S3.
    pub endpoint: Option<String>,
    /// Signing region. Any value works for most S3-compatible servers.
    pub region: String,
    /// Bucket holding the game archives.
    pub bucket: String,
    /// Access key id.
    pub access_key: String,
    /// Secret access key.
    pub secret_key: String,
    /// Path-style addressing (`endpoint/bucket/key`). Required by MinIO and
    /// most self-hosted gateways.
    pub path_style: bool,
    /// Optional key prefix, without leading or trailing slashes.
    pub prefix: Option<String>,
}

impl std::fmt::Debug for S3Config {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("S3Config")
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("access_key", &"<redacted>")
            .field("secret_key", &"<redacted>")
            .field("path_style", &self.path_style)
            .field("prefix", &self.prefix)
            .finish()
    }
}

impl S3Config {
    /// Prefixes an object key, without a leading slash.
    #[must_use]
    pub fn key(&self, relative: &str) -> String {
        let relative = relative.trim_start_matches('/');
        match self.prefix.as_deref().filter(|prefix| !prefix.is_empty()) {
            Some(prefix) => format!("{prefix}/{relative}"),
            None => relative.to_string(),
        }
    }
}
/// Default JWT access-token lifetime (7 days, the historical value).
pub const DEFAULT_JWT_TTL_MINUTES: u64 = 7 * 24 * 60;
/// Default global API request rate, per client IP. Zero disables limiting.
pub const DEFAULT_RATE_LIMIT_RPS: u32 = 100;
/// Default global API burst, per client IP.
pub const DEFAULT_RATE_LIMIT_BURST: u32 = 300;
/// Default failed logins before a lockout.
pub const DEFAULT_LOGIN_MAX_ATTEMPTS: u32 = 5;
/// Default lockout duration after too many failed logins.
pub const DEFAULT_LOGIN_LOCKOUT_SECONDS: u64 = 900;
/// Default chunk size accepted by resumable archive uploads (16 MiB).
pub const DEFAULT_UPLOAD_CHUNK_BYTES: u64 = 16 * 1024 * 1024;
/// Default maximum size of a resumable archive upload (100 GiB).
pub const DEFAULT_UPLOAD_MAX_BYTES: u64 = 100 * 1024 * 1024 * 1024;
/// Default lifetime of an unfinished resumable upload (24 h).
pub const DEFAULT_UPLOAD_TTL_HOURS: u64 = 24;

/// Runtime configuration for the Ludex server.
///
/// Every field can be set through `LUDEX_*` environment variables with
/// [`Config::from_env`]. The desktop app uses [`Config::for_local`] instead and
/// lets the server pick a random loopback port.
#[derive(Debug, Clone)]
pub struct Config {
    /// TCP address to listen on. Port 0 asks the OS for a free port.
    pub bind: SocketAddr,
    /// Directory holding `ludex.db` and the generated JWT secret.
    pub data_dir: PathBuf,
    /// Library volume: archives and portable executables.
    pub games_dir: PathBuf,
    /// Cache for covers, backgrounds and screenshots.
    pub media_dir: PathBuf,
    /// SQLite connection string.
    pub database_url: String,
    /// Explicit JWT secret. When `None`, a 32-byte secret is generated on
    /// first run and persisted as `{data_dir}/jwt.secret` (mode 0600).
    pub jwt_secret: Option<String>,
    /// Display name shown by the web UI.
    pub server_name: String,
    /// Optional first-run administrator, only used while there are zero users.
    pub admin_username: Option<String>,
    /// Password for [`Config::admin_username`].
    pub admin_password: Option<String>,
    /// IGDB / Twitch application client id.
    pub igdb_client_id: Option<String>,
    /// IGDB / Twitch application client secret.
    pub igdb_client_secret: Option<String>,
    /// Optional compiled web UI on disk. Takes precedence over `embed-web`.
    pub web_dir: Option<PathBuf>,
    /// Embedded mode used by the desktop app.
    pub local_mode: bool,
    /// Allow ingest downloads from private/loopback addresses.
    pub ingest_allow_private: bool,
    /// Allow webhook targets on private/loopback addresses (tests only).
    pub webhooks_allow_private: bool,
    /// Base delay for webhook retry backoff, in milliseconds.
    pub webhooks_backoff_ms: u64,
    /// Interval between automatic library scans. Zero disables periodic scans.
    pub scan_interval: Duration,
    /// Ordered library roots. The first entry is the ingest target
    /// (`LUDEX_GAMES_DIR`); the rest come from `LUDEX_LIBRARY_ROOTS`.
    pub library_roots: Vec<PathBuf>,
    /// Watch enabled library roots for new files (`LUDEX_WATCHER_ENABLED`).
    pub watcher_enabled: bool,
    /// Idle window used to coalesce filesystem events before a scan.
    pub watcher_debounce: Duration,
    /// Interval between scheduled integrity checks. Zero disables them.
    pub integrity_interval: Duration,
    /// Minimum age of an unreferenced media file before garbage collection.
    pub media_gc_grace: Duration,
    /// Interval between scheduled media garbage collections. Zero disables them.
    pub media_gc_interval: Duration,
    /// Maximum size of a single cloud save upload, in bytes.
    pub saves_max_bytes: u64,
    /// JWT access-token lifetime, in minutes.
    pub jwt_ttl_minutes: u64,
    /// Global API request rate per client IP. Zero disables limiting.
    pub rate_limit_rps: u32,
    /// Global API burst per client IP.
    pub rate_limit_burst: u32,
    /// Failed logins allowed before a lockout. Zero disables lockout.
    pub login_max_attempts: u32,
    /// Lockout duration after too many failed logins.
    pub login_lockout: Duration,
    /// Base delay of the exponential login backoff.
    pub login_backoff: Duration,
    /// Chunk size accepted by `PUT /api/uploads/{id}/parts/{n}`.
    pub upload_chunk_bytes: u64,
    /// Maximum accepted archive upload size.
    pub upload_max_bytes: u64,
    /// Lifetime of an unfinished resumable upload.
    pub upload_ttl: Duration,
    /// Serve the human-readable OpenAPI explorer at `/api/docs`.
    pub openapi_docs: bool,
    /// OIDC issuer URL. `None` disables SSO.
    pub oidc_issuer: Option<String>,
    /// OIDC client id.
    pub oidc_client_id: Option<String>,
    /// OIDC client secret (optional for public PKCE clients).
    pub oidc_client_secret: Option<String>,
    /// Redirect URI registered with the provider. Relative paths are resolved
    /// against `oidc_public_base_url` or the incoming request host.
    pub oidc_redirect_url: Option<String>,
    /// Space-separated scopes requested at the provider.
    pub oidc_scopes: String,
    /// Display name used by the web UI's SSO button.
    pub oidc_display_name: String,
    /// Create local accounts for unknown OIDC identities.
    pub oidc_auto_create: bool,
    /// Role assigned to newly provisioned OIDC users without admin groups.
    pub oidc_default_role: Role,
    /// Group/role claim values that grant the admin role.
    pub oidc_admin_groups: Vec<String>,
    /// Claim (dot-separated path) holding group/role values.
    pub oidc_groups_claim: String,
    /// Public origin used to build the OIDC redirect URI and the SPA callback
    /// URL. Must be an absolute http(s) origin/path without query or fragment.
    pub oidc_public_base_url: Option<String>,
    /// Game archive storage backend.
    pub storage_backend: StorageBackend,
    /// S3 configuration, present when `storage_backend` is [`StorageBackend::S3`].
    pub s3: Option<S3Config>,
    /// Bearer token required by `GET /metrics`. `None` keeps it public but
    /// rate-limited.
    pub metrics_token: Option<String>,
    /// Interval between automatic achievement rescans. Zero disables the job.
    pub achievement_scan_interval: Duration,
    /// Directories a scan may read (`LUDEX_ACHIEVEMENT_SCAN_ROOTS`).
    pub achievement_scan_roots: Vec<PathBuf>,
    /// Enabled local achievement providers.
    pub achievement_providers: Vec<String>,
    /// Loopback URL patterns used by the JSON provider.
    pub achievement_local_api_urls: Vec<String>,
    /// Explicit GOG Galaxy database path.
    pub achievement_galaxy_db: Option<PathBuf>,
    /// Allow non-loopback targets for the JSON provider (tests only).
    pub achievement_allow_remote_apis: bool,
}

impl Config {
    /// Reads configuration from `LUDEX_*` environment variables.
    pub fn from_env() -> Result<Self> {
        let data_dir = env_path("LUDEX_DATA_DIR", "./data");
        let games_dir = env_path("LUDEX_GAMES_DIR", "./games");
        let media_dir = env_path("LUDEX_MEDIA_DIR", "./media");
        let bind = env::var("LUDEX_BIND")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BIND.to_string())
            .parse()
            .context("LUDEX_BIND must be a socket address such as 0.0.0.0:8080")?;
        let database_url = env::var("LUDEX_DATABASE_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| default_database_url(&data_dir));
        let oidc_public_base_url = env_opt("LUDEX_OIDC_PUBLIC_BASE_URL")
            .map(|value| value.trim_end_matches('/').to_string());
        let oidc_redirect_url = env_opt("LUDEX_OIDC_REDIRECT_URL").or_else(|| {
            oidc_public_base_url
                .as_ref()
                .map(|base| format!("{base}/api/auth/oidc/callback"))
        });
        let oidc_default_role = env_opt("LUDEX_OIDC_DEFAULT_ROLE")
            .unwrap_or_else(|| "user".to_string())
            .parse::<Role>()
            .context("LUDEX_OIDC_DEFAULT_ROLE must be admin, editor or user")?;
        let oidc_admin_groups = env_opt("LUDEX_OIDC_ADMIN_GROUPS")
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|group| !group.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        let storage_backend = env_opt("LUDEX_STORAGE_BACKEND")
            .map(|value| value.parse::<StorageBackend>())
            .transpose()
            .context("LUDEX_STORAGE_BACKEND must be local or s3")?
            .unwrap_or_default();
        let s3 = match storage_backend {
            StorageBackend::Local => None,
            StorageBackend::S3 => Some(parse_s3_config()?),
        };

        let scan_interval_seconds = env::var("LUDEX_SCAN_INTERVAL_SECONDS")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.parse::<u64>())
            .transpose()
            .context("LUDEX_SCAN_INTERVAL_SECONDS must be an integer")?
            .unwrap_or(DEFAULT_SCAN_INTERVAL_SECONDS);

        let library_roots =
            parse_library_roots(env_opt("LUDEX_LIBRARY_ROOTS").as_deref(), &games_dir);
        let watcher_debounce_ms = env_opt("LUDEX_WATCHER_DEBOUNCE_MS")
            .map(|value| value.parse::<u64>())
            .transpose()
            .context("LUDEX_WATCHER_DEBOUNCE_MS must be an integer")?
            .unwrap_or(DEFAULT_WATCHER_DEBOUNCE_MS);
        let integrity_interval_hours = env_opt("LUDEX_INTEGRITY_INTERVAL_HOURS")
            .map(|value| value.parse::<u64>())
            .transpose()
            .context("LUDEX_INTEGRITY_INTERVAL_HOURS must be an integer")?
            .unwrap_or(DEFAULT_INTEGRITY_INTERVAL_HOURS);
        let media_gc_grace_hours = env_opt("LUDEX_MEDIA_GC_GRACE_HOURS")
            .map(|value| value.parse::<u64>())
            .transpose()
            .context("LUDEX_MEDIA_GC_GRACE_HOURS must be an integer")?
            .unwrap_or(DEFAULT_MEDIA_GC_GRACE_HOURS);
        let media_gc_interval_hours = env_opt("LUDEX_MEDIA_GC_INTERVAL_HOURS")
            .map(|value| value.parse::<u64>())
            .transpose()
            .context("LUDEX_MEDIA_GC_INTERVAL_HOURS must be an integer")?
            .unwrap_or(0);

        let saves_max_bytes = match env_opt("LUDEX_SAVES_MAX_BYTES") {
            Some(value) => {
                let parsed = value
                    .parse::<u64>()
                    .context("LUDEX_SAVES_MAX_BYTES must be an integer")?;
                anyhow::ensure!(
                    parsed > 0,
                    "LUDEX_SAVES_MAX_BYTES must be greater than zero"
                );
                parsed
            }
            None => DEFAULT_SAVES_MAX_BYTES,
        };

        let jwt_ttl_minutes = match env_opt("LUDEX_JWT_TTL_MINUTES") {
            Some(value) => value
                .parse::<u64>()
                .context("LUDEX_JWT_TTL_MINUTES must be an integer")?
                .max(1),
            None => DEFAULT_JWT_TTL_MINUTES,
        };
        let rate_limit_rps = match env_opt("LUDEX_RATE_LIMIT_RPS") {
            Some(value) => value
                .parse::<u32>()
                .context("LUDEX_RATE_LIMIT_RPS must be an integer")?,
            None => DEFAULT_RATE_LIMIT_RPS,
        };
        let rate_limit_burst = match env_opt("LUDEX_RATE_LIMIT_BURST") {
            Some(value) => value
                .parse::<u32>()
                .context("LUDEX_RATE_LIMIT_BURST must be an integer")?,
            None => DEFAULT_RATE_LIMIT_BURST,
        };
        let login_max_attempts = match env_opt("LUDEX_LOGIN_MAX_ATTEMPTS") {
            Some(value) => value
                .parse::<u32>()
                .context("LUDEX_LOGIN_MAX_ATTEMPTS must be an integer")?,
            None => DEFAULT_LOGIN_MAX_ATTEMPTS,
        };
        let login_lockout_seconds = match env_opt("LUDEX_LOGIN_LOCKOUT_SECONDS") {
            Some(value) => value
                .parse::<u64>()
                .context("LUDEX_LOGIN_LOCKOUT_SECONDS must be an integer")?,
            None => DEFAULT_LOGIN_LOCKOUT_SECONDS,
        };
        let login_backoff_ms = match env_opt("LUDEX_LOGIN_BACKOFF_MS") {
            Some(value) => value
                .parse::<u64>()
                .context("LUDEX_LOGIN_BACKOFF_MS must be an integer")?,
            None => 500,
        };
        let upload_chunk_bytes = match env_opt("LUDEX_UPLOAD_CHUNK_BYTES") {
            Some(value) => {
                let parsed = value
                    .parse::<u64>()
                    .context("LUDEX_UPLOAD_CHUNK_BYTES must be an integer")?;
                anyhow::ensure!(
                    parsed > 0,
                    "LUDEX_UPLOAD_CHUNK_BYTES must be greater than zero"
                );
                parsed.min(64 * 1024 * 1024)
            }
            None => DEFAULT_UPLOAD_CHUNK_BYTES,
        };
        let upload_max_bytes = match env_opt("LUDEX_UPLOAD_MAX_BYTES") {
            Some(value) => {
                let parsed = value
                    .parse::<u64>()
                    .context("LUDEX_UPLOAD_MAX_BYTES must be an integer")?;
                anyhow::ensure!(
                    parsed > 0,
                    "LUDEX_UPLOAD_MAX_BYTES must be greater than zero"
                );
                parsed
            }
            None => DEFAULT_UPLOAD_MAX_BYTES,
        };
        let upload_ttl_hours = match env_opt("LUDEX_UPLOAD_TTL_HOURS") {
            Some(value) => value
                .parse::<u64>()
                .context("LUDEX_UPLOAD_TTL_HOURS must be an integer")?,
            None => DEFAULT_UPLOAD_TTL_HOURS,
        };

        let achievement_scan_interval_hours = env_opt("LUDEX_ACHIEVEMENT_SCAN_INTERVAL_HOURS")
            .map(|value| value.parse::<u64>())
            .transpose()
            .context("LUDEX_ACHIEVEMENT_SCAN_INTERVAL_HOURS must be an integer")?
            .unwrap_or(0);
        let achievement_scan_roots =
            parse_path_list(env_opt("LUDEX_ACHIEVEMENT_SCAN_ROOTS").as_deref())
                .unwrap_or_else(|| library_roots.clone());
        let achievement_providers = match env_opt("LUDEX_ACHIEVEMENT_PROVIDERS") {
            Some(value) => {
                let providers = split_list(&value);
                if providers
                    .iter()
                    .any(|provider| provider.eq_ignore_ascii_case("none"))
                {
                    Vec::new()
                } else {
                    providers
                }
            }
            None => DEFAULT_ACHIEVEMENT_PROVIDERS
                .iter()
                .map(|provider| (*provider).to_string())
                .collect(),
        };
        let achievement_local_api_urls = env_opt("LUDEX_ACHIEVEMENT_LOCAL_API_URLS")
            .map(|value| split_list(&value))
            .filter(|urls| !urls.is_empty())
            .unwrap_or_else(|| {
                DEFAULT_ACHIEVEMENT_API_URLS
                    .iter()
                    .map(|url| (*url).to_string())
                    .collect()
            });
        let achievement_galaxy_db = env_opt("LUDEX_ACHIEVEMENT_GALAXY_DB").map(PathBuf::from);

        Ok(Self {
            bind,
            data_dir,
            games_dir,
            media_dir,
            database_url,
            jwt_secret: env_opt("LUDEX_JWT_SECRET"),
            server_name: env_opt("LUDEX_SERVER_NAME").unwrap_or_else(|| "Ludex".to_string()),
            admin_username: env_opt("LUDEX_ADMIN_USERNAME"),
            admin_password: env_opt("LUDEX_ADMIN_PASSWORD"),
            igdb_client_id: env_opt("LUDEX_IGDB_CLIENT_ID"),
            igdb_client_secret: env_opt("LUDEX_IGDB_CLIENT_SECRET"),
            web_dir: env_opt("LUDEX_WEB_DIR").map(PathBuf::from),
            local_mode: bool_env("LUDEX_LOCAL_MODE"),
            ingest_allow_private: bool_env("LUDEX_INGEST_ALLOW_PRIVATE"),
            webhooks_allow_private: bool_env("LUDEX_WEBHOOKS_ALLOW_PRIVATE"),
            webhooks_backoff_ms: env_opt("LUDEX_WEBHOOKS_BACKOFF_MS")
                .map(|value| value.parse::<u64>())
                .transpose()
                .context("LUDEX_WEBHOOKS_BACKOFF_MS must be an integer")?
                .unwrap_or(1_000),
            scan_interval: Duration::from_secs(scan_interval_seconds),
            library_roots,
            watcher_enabled: bool_env("LUDEX_WATCHER_ENABLED"),
            watcher_debounce: Duration::from_millis(watcher_debounce_ms),
            integrity_interval: Duration::from_secs(integrity_interval_hours * 3_600),
            media_gc_grace: Duration::from_secs(media_gc_grace_hours * 3_600),
            media_gc_interval: Duration::from_secs(media_gc_interval_hours * 3_600),
            saves_max_bytes,
            jwt_ttl_minutes,
            rate_limit_rps,
            rate_limit_burst,
            login_max_attempts,
            login_lockout: Duration::from_secs(login_lockout_seconds),
            login_backoff: Duration::from_millis(login_backoff_ms),
            upload_chunk_bytes,
            upload_max_bytes,
            upload_ttl: Duration::from_secs(upload_ttl_hours * 3_600),
            openapi_docs: bool_env("LUDEX_OPENAPI_DOCS"),
            oidc_issuer: env_opt("LUDEX_OIDC_ISSUER"),
            oidc_client_id: env_opt("LUDEX_OIDC_CLIENT_ID"),
            oidc_client_secret: env_opt("LUDEX_OIDC_CLIENT_SECRET"),
            oidc_redirect_url,
            oidc_scopes: env_opt("LUDEX_OIDC_SCOPES")
                .unwrap_or_else(|| "openid profile email".to_string()),
            oidc_display_name: env_opt("LUDEX_OIDC_DISPLAY_NAME")
                .unwrap_or_else(|| "SSO".to_string()),
            oidc_auto_create: bool_env_default("LUDEX_OIDC_AUTO_CREATE", true),
            oidc_default_role,
            oidc_admin_groups,
            oidc_groups_claim: env_opt("LUDEX_OIDC_GROUPS_CLAIM")
                .unwrap_or_else(|| "groups".to_string()),
            oidc_public_base_url,
            storage_backend,
            s3,
            metrics_token: env_opt("LUDEX_METRICS_TOKEN"),
            achievement_scan_interval: Duration::from_secs(achievement_scan_interval_hours * 3_600),
            achievement_scan_roots,
            achievement_providers,
            achievement_local_api_urls,
            achievement_galaxy_db,
            achievement_allow_remote_apis: bool_env("LUDEX_ACHIEVEMENT_ALLOW_REMOTE_APIS"),
        })
    }

    /// Configuration for the desktop app's in-process server.
    ///
    /// Binds an ephemeral loopback port, enables local mode (which
    /// auto-provisions an administrator and returns its token from
    /// [`crate::start`]) and does not read any bootstrap credentials.
    pub fn for_local(games_dir: PathBuf, data_dir: PathBuf, media_dir: PathBuf) -> Self {
        let database_url = default_database_url(&data_dir);
        let library_roots = vec![games_dir.clone()];
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            data_dir,
            games_dir: games_dir.clone(),
            media_dir,
            database_url,
            jwt_secret: None,
            server_name: "Ludex".to_string(),
            admin_username: None,
            admin_password: None,
            igdb_client_id: None,
            igdb_client_secret: None,
            web_dir: None,
            local_mode: true,
            ingest_allow_private: false,
            webhooks_allow_private: false,
            webhooks_backoff_ms: 1_000,
            scan_interval: Duration::from_secs(DEFAULT_SCAN_INTERVAL_SECONDS),
            library_roots,
            watcher_enabled: false,
            watcher_debounce: Duration::from_millis(DEFAULT_WATCHER_DEBOUNCE_MS),
            integrity_interval: Duration::from_secs(DEFAULT_INTEGRITY_INTERVAL_HOURS * 3_600),
            media_gc_grace: Duration::from_secs(DEFAULT_MEDIA_GC_GRACE_HOURS * 3_600),
            media_gc_interval: Duration::ZERO,
            saves_max_bytes: DEFAULT_SAVES_MAX_BYTES,
            jwt_ttl_minutes: DEFAULT_JWT_TTL_MINUTES,
            rate_limit_rps: 0,
            rate_limit_burst: DEFAULT_RATE_LIMIT_BURST,
            login_max_attempts: DEFAULT_LOGIN_MAX_ATTEMPTS,
            login_lockout: Duration::from_secs(DEFAULT_LOGIN_LOCKOUT_SECONDS),
            login_backoff: Duration::from_millis(500),
            upload_chunk_bytes: DEFAULT_UPLOAD_CHUNK_BYTES,
            upload_max_bytes: DEFAULT_UPLOAD_MAX_BYTES,
            upload_ttl: Duration::from_secs(DEFAULT_UPLOAD_TTL_HOURS * 3_600),
            openapi_docs: false,
            oidc_issuer: None,
            oidc_client_id: None,
            oidc_client_secret: None,
            oidc_redirect_url: None,
            oidc_scopes: "openid profile email".to_string(),
            oidc_display_name: "SSO".to_string(),
            oidc_auto_create: true,
            oidc_default_role: Role::User,
            oidc_admin_groups: Vec::new(),
            oidc_groups_claim: "groups".to_string(),
            oidc_public_base_url: None,
            storage_backend: StorageBackend::Local,
            s3: None,
            metrics_token: None,
            achievement_scan_interval: Duration::ZERO,
            achievement_scan_roots: vec![games_dir],
            achievement_providers: DEFAULT_ACHIEVEMENT_PROVIDERS
                .iter()
                .map(|provider| (*provider).to_string())
                .collect(),
            achievement_local_api_urls: DEFAULT_ACHIEVEMENT_API_URLS
                .iter()
                .map(|url| (*url).to_string())
                .collect(),
            achievement_galaxy_db: None,
            achievement_allow_remote_apis: false,
        }
    }

    /// Overrides the bind address.
    #[must_use]
    pub fn with_bind(mut self, bind: SocketAddr) -> Self {
        self.bind = bind;
        self
    }

    /// Overrides the display name.
    #[must_use]
    pub fn with_server_name(mut self, name: impl Into<String>) -> Self {
        self.server_name = name.into();
        self
    }

    /// Overrides the automatic scan interval.
    #[must_use]
    pub fn with_scan_interval(mut self, interval: Duration) -> Self {
        self.scan_interval = interval;
        self
    }

    /// Whether IGDB credentials are configured.
    #[must_use]
    pub fn igdb_enabled(&self) -> bool {
        self.igdb_client_id.is_some() && self.igdb_client_secret.is_some()
    }

    /// Whether OIDC SSO is configured.
    #[must_use]
    pub fn oidc_enabled(&self) -> bool {
        self.oidc_issuer.is_some()
    }

    /// Whether game archives are uploaded to and served from S3.
    #[must_use]
    pub fn s3_enabled(&self) -> bool {
        self.storage_backend == StorageBackend::S3 && self.s3.is_some()
    }
}

/// Builds the default SQLite URL for a data directory.
#[must_use]
pub fn default_database_url(data_dir: &Path) -> String {
    let path = data_dir.join("ludex.db");
    format!("sqlite://{}?mode=rwc", path.display())
}

/// Parses `LUDEX_LIBRARY_ROOTS` into an ordered, de-duplicated list.
///
/// The ingest target (`games_dir`) is always present and always first so new
/// downloads land in a known root. Empty entries are ignored.
#[must_use]
pub fn parse_library_roots(value: Option<&str>, games_dir: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for part in value.unwrap_or_default().split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let path = PathBuf::from(trimmed);
        if !roots.contains(&path) {
            roots.push(path);
        }
    }
    if roots.is_empty() {
        roots.push(games_dir.to_path_buf());
    } else if !roots.iter().any(|root| root == games_dir) {
        roots.insert(0, games_dir.to_path_buf());
    }
    roots
}

/// Splits a comma-separated environment value into trimmed, non-empty entries.
#[must_use]
pub fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parses a comma-separated list of paths, de-duplicated and in order.
#[must_use]
pub fn parse_path_list(value: Option<&str>) -> Option<Vec<PathBuf>> {
    let paths = split_list(value?)
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    (!paths.is_empty()).then_some(paths)
}

fn env_opt(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Reads the S3 settings. Every required value is validated before the server
/// binds, so a typo fails loudly instead of silently falling back to local
/// storage. No network call is made here.
fn parse_s3_config() -> Result<S3Config> {
    let bucket =
        env_opt("LUDEX_S3_BUCKET").context("LUDEX_S3_BUCKET is required for s3 storage")?;
    let access_key =
        env_opt("LUDEX_S3_ACCESS_KEY").context("LUDEX_S3_ACCESS_KEY is required for s3 storage")?;
    let secret_key =
        env_opt("LUDEX_S3_SECRET_KEY").context("LUDEX_S3_SECRET_KEY is required for s3 storage")?;
    let endpoint =
        env_opt("LUDEX_S3_ENDPOINT").map(|value| value.trim_end_matches('/').to_string());
    let prefix = env_opt("LUDEX_S3_PREFIX").map(|value| {
        value
            .trim()
            .trim_matches('/')
            .trim_start_matches('/')
            .to_string()
    });
    Ok(S3Config {
        endpoint,
        region: env_opt("LUDEX_S3_REGION").unwrap_or_else(|| DEFAULT_S3_REGION.to_string()),
        bucket,
        access_key,
        secret_key,
        path_style: bool_env("LUDEX_S3_PATH_STYLE"),
        prefix: prefix.filter(|value| !value.is_empty()),
    })
}

fn env_path(name: &str, default: &str) -> PathBuf {
    PathBuf::from(env_opt(name).unwrap_or_else(|| default.to_string()))
}

fn bool_env(name: &str) -> bool {
    bool_env_default(name, false)
}

fn bool_env_default(name: &str, default: bool) -> bool {
    env_opt(name).map_or(default, |value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_config_is_loopback_and_local() {
        let config = Config::for_local(
            PathBuf::from("/tmp/games"),
            PathBuf::from("/tmp/data"),
            PathBuf::from("/tmp/media"),
        );
        assert!(config.local_mode);
        assert!(config.bind.ip().is_loopback());
        assert_eq!(config.bind.port(), 0);
        assert!(config.database_url.contains("/tmp/data/ludex.db"));
        assert!(!config.igdb_enabled());
        assert_eq!(config.saves_max_bytes, DEFAULT_SAVES_MAX_BYTES);
    }

    #[test]
    fn storage_backend_parses_aliases() {
        assert_eq!(
            "local".parse::<StorageBackend>().expect("local"),
            StorageBackend::Local
        );
        assert_eq!(
            " disk ".parse::<StorageBackend>().expect("disk"),
            StorageBackend::Local
        );
        assert_eq!(
            "minio".parse::<StorageBackend>().expect("minio"),
            StorageBackend::S3
        );
        assert!("nfs".parse::<StorageBackend>().is_err());
    }

    #[test]
    fn s3_prefix_joins_without_duplicate_slashes() {
        let config = S3Config {
            endpoint: None,
            region: "us-east-1".to_string(),
            bucket: "ludex".to_string(),
            access_key: "AKIAEXAMPLE".to_string(),
            secret_key: "super-secret-value".to_string(),
            path_style: true,
            prefix: Some("prod".to_string()),
        };
        assert_eq!(config.key("/archives/a.zip"), "prod/archives/a.zip");
        let debug = format!("{config:?}");
        assert!(!debug.contains("super-secret-value"));
        assert!(!debug.contains("AKIAEXAMPLE"));
    }

    #[test]
    fn default_database_url_uses_rwc_mode() {
        let url = default_database_url(Path::new("/srv/ludex"));
        assert_eq!(url, "sqlite:///srv/ludex/ludex.db?mode=rwc");
    }

    #[test]
    fn library_roots_keep_games_dir_first_and_drop_duplicates() {
        let games = Path::new("/games");
        assert_eq!(parse_library_roots(None, games), vec![games.to_path_buf()]);
        assert_eq!(
            parse_library_roots(Some("  , /mnt/one , /mnt/one,/mnt/two "), games),
            vec![
                games.to_path_buf(),
                PathBuf::from("/mnt/one"),
                PathBuf::from("/mnt/two"),
            ]
        );
        assert_eq!(
            parse_library_roots(Some("/mnt/one,/games,/mnt/two"), games),
            vec![
                PathBuf::from("/mnt/one"),
                games.to_path_buf(),
                PathBuf::from("/mnt/two"),
            ]
        );
    }

    #[test]
    fn local_config_disables_watcher_and_scheduled_gc() {
        let config = Config::for_local(
            PathBuf::from("/tmp/games"),
            PathBuf::from("/tmp/data"),
            PathBuf::from("/tmp/media"),
        );
        assert!(!config.watcher_enabled);
        assert_eq!(config.library_roots, vec![PathBuf::from("/tmp/games")]);
        assert_eq!(config.media_gc_interval, Duration::ZERO);
        assert_eq!(config.media_gc_grace, Duration::from_secs(24 * 3_600));
        assert_eq!(config.achievement_scan_interval, Duration::ZERO);
        assert_eq!(config.achievement_providers.len(), 3);
        assert_eq!(
            config.achievement_scan_roots,
            vec![PathBuf::from("/tmp/games")]
        );
        assert!(!config.achievement_allow_remote_apis);
    }

    #[test]
    fn achievement_path_lists_are_trimmed_and_empty_is_none() {
        assert_eq!(
            parse_path_list(Some(" /mnt/games , , /mnt/extra ")),
            Some(vec![
                PathBuf::from("/mnt/games"),
                PathBuf::from("/mnt/extra")
            ])
        );
        assert_eq!(parse_path_list(Some("  , ")), None);
        assert_eq!(parse_path_list(None), None);
        assert_eq!(
            split_list(" game_files , gog_galaxy_api "),
            vec!["game_files".to_string(), "gog_galaxy_api".to_string()]
        );
    }
}
