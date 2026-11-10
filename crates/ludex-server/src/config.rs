//! Server configuration, loaded from the environment or built for the desktop
//! app's embedded local mode.

use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use ludex_core::models::Role;

/// Default bind address when `LUDEX_BIND` is unset.
pub const DEFAULT_BIND: &str = "0.0.0.0:8080";
/// Default scan interval, in seconds.
pub const DEFAULT_SCAN_INTERVAL_SECONDS: u64 = 300;
/// Maximum JSON request body accepted by the API (10 MiB).
pub const MAX_JSON_BYTES: usize = 10 * 1024 * 1024;
/// Default maximum size of a single cloud save upload (2 GiB).
pub const DEFAULT_SAVES_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

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
    /// Maximum size of a single cloud save upload, in bytes.
    pub saves_max_bytes: u64,
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

        let scan_interval_seconds = env::var("LUDEX_SCAN_INTERVAL_SECONDS")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.parse::<u64>())
            .transpose()
            .context("LUDEX_SCAN_INTERVAL_SECONDS must be an integer")?
            .unwrap_or(DEFAULT_SCAN_INTERVAL_SECONDS);

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
            saves_max_bytes,
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
        })
    }

    /// Configuration for the desktop app's in-process server.
    ///
    /// Binds an ephemeral loopback port, enables local mode (which
    /// auto-provisions an administrator and returns its token from
    /// [`crate::start`]) and does not read any bootstrap credentials.
    pub fn for_local(games_dir: PathBuf, data_dir: PathBuf, media_dir: PathBuf) -> Self {
        let database_url = default_database_url(&data_dir);
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            data_dir,
            games_dir,
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
            saves_max_bytes: DEFAULT_SAVES_MAX_BYTES,
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
}

/// Builds the default SQLite URL for a data directory.
#[must_use]
pub fn default_database_url(data_dir: &Path) -> String {
    let path = data_dir.join("ludex.db");
    format!("sqlite://{}?mode=rwc", path.display())
}

fn env_opt(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
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
    fn default_database_url_uses_rwc_mode() {
        let url = default_database_url(Path::new("/srv/ludex"));
        assert_eq!(url, "sqlite:///srv/ludex/ludex.db?mode=rwc");
    }
}
