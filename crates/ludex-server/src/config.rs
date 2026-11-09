//! Server configuration, loaded from the environment or built for the desktop
//! app's embedded local mode.

use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

/// Default bind address when `LUDEX_BIND` is unset.
pub const DEFAULT_BIND: &str = "0.0.0.0:8080";
/// Default scan interval, in seconds.
pub const DEFAULT_SCAN_INTERVAL_SECONDS: u64 = 300;
/// Maximum JSON request body accepted by the API (10 MiB).
pub const MAX_JSON_BYTES: usize = 10 * 1024 * 1024;

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
    /// Interval between automatic library scans. Zero disables periodic scans.
    pub scan_interval: Duration,
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
        let scan_interval_seconds = env::var("LUDEX_SCAN_INTERVAL_SECONDS")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.parse::<u64>())
            .transpose()
            .context("LUDEX_SCAN_INTERVAL_SECONDS must be an integer")?
            .unwrap_or(DEFAULT_SCAN_INTERVAL_SECONDS);

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
            scan_interval: Duration::from_secs(scan_interval_seconds),
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
            scan_interval: Duration::from_secs(DEFAULT_SCAN_INTERVAL_SECONDS),
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
    env_opt(name).is_some_and(|value| {
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
    }

    #[test]
    fn default_database_url_uses_rwc_mode() {
        let url = default_database_url(Path::new("/srv/ludex"));
        assert_eq!(url, "sqlite:///srv/ludex/ludex.db?mode=rwc");
    }
}
