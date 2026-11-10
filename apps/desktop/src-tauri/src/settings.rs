//! Persisted desktop settings (`{appDataDir}/settings.json`).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::linux_gaming::LaunchMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Local,
    Remote,
}

/// Desktop update channel.
///
/// `stable` follows the latest GitHub release; `early` follows the moving
/// `edge` prerelease, which is published from `main` between releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Early,
}

impl UpdateChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            UpdateChannel::Stable => "stable",
            UpdateChannel::Early => "early",
        }
    }
}

/// Mirrors `LocalSettings` from `docs/CONTRACT.md`.
///
/// `keepArchives` and `archivePasswords` are desktop-only extras: the web UI
/// may omit them, and `save_settings` merges partial payloads, so they survive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LocalSettings {
    pub mode: Mode,
    /// Remote server URL, or the embedded server URL in local mode.
    pub server_url: String,
    /// JWT for the remote server or the auto-provisioned local admin.
    pub token: Option<String>,
    /// Folder scanned by the embedded server in local mode.
    pub games_dir: Option<String>,
    /// Where games are installed.
    pub install_dir: String,
    /// Where temporary archives are downloaded.
    pub download_dir: String,
    /// Preferred embedded server port, 0 = random.
    pub server_port: u16,
    /// Install remote ZIP versions from the tar stream (no archive on disk).
    pub stream_install: bool,
    /// Keep the downloaded archive after a successful install.
    pub keep_archives: bool,
    /// Passwords tried, in order, when extracting an archive.
    pub archive_passwords: Vec<String>,
    /// Global download bandwidth cap in KiB/s. 0 = unlimited.
    pub bandwidth_limit_kib: u64,
    /// Closing the window hides Ludex to the tray instead of quitting.
    pub close_to_tray: bool,
    /// Release channel followed by the updater.
    pub update_channel: UpdateChannel,
    /// Check for app updates shortly after startup (at most once per 24 h).
    pub check_updates_on_startup: bool,
    /// Last successful or attempted update check, RFC 3339. Used for the
    /// once-per-24 h throttle; cleared by a manual check.
    pub last_update_check_at: Option<String>,
    /// Publish the running game as Discord Rich Presence (#12). Off by default.
    pub discord_presence: bool,
    /// Discord application id used for Rich Presence. An application id is not
    /// a bot token; it only labels the activity.
    pub discord_client_id: String,
    /// Argument sets tried in order by `run_installer` (#7). Empty means the
    /// built-in NSIS/Inno/InstallShield presets.
    pub installer_silent_args: Vec<Vec<String>>,
    /// How long one silent install attempt may run before it is killed.
    pub installer_timeout_secs: u64,
    /// Steam root overriding discovery (#13). Empty means auto-detect.
    pub steam_root: String,
    /// `umu-run` override for the compatibility scan (#90). Empty = detect.
    pub compat_umu_path: String,
    /// Proton folder (or its `proton` script) override (#90). Empty = detect.
    pub compat_proton_path: String,
    /// `wine` override for the compatibility scan (#90). Empty = detect.
    pub compat_wine_path: String,
    /// Launch mode new games start with on Linux (#89).
    pub default_launch_mode: LaunchMode,
}

impl Default for LocalSettings {
    fn default() -> Self {
        Self {
            mode: Mode::Local,
            server_url: String::new(),
            token: None,
            games_dir: None,
            install_dir: String::new(),
            download_dir: String::new(),
            server_port: 0,
            stream_install: true,
            keep_archives: false,
            archive_passwords: Vec::new(),
            bandwidth_limit_kib: 0,
            close_to_tray: true,
            update_channel: UpdateChannel::Stable,
            check_updates_on_startup: true,
            last_update_check_at: None,
            discord_presence: false,
            discord_client_id: String::new(),
            installer_silent_args: Vec::new(),
            installer_timeout_secs: 600,
            steam_root: String::new(),
            compat_umu_path: String::new(),
            compat_proton_path: String::new(),
            compat_wine_path: String::new(),
            default_launch_mode: LaunchMode::Auto,
        }
    }
}

impl LocalSettings {
    pub fn for_app_data(app_data: &Path) -> Self {
        Self {
            install_dir: app_data.join("Games").to_string_lossy().to_string(),
            // Empty means "same volume as the install folder": the effective
            // download dir becomes `{installDir}/.ludex-tmp`.
            download_dir: String::new(),
            ..Self::default()
        }
    }

    /// Fill empty paths with defaults and normalise the server URL.
    pub fn normalize(&mut self, app_data: &Path) {
        let defaults = Self::for_app_data(app_data);
        if self.install_dir.trim().is_empty() {
            self.install_dir = defaults.install_dir;
        }
        // `downloadDir` empty is meaningful: use `{installDir}/.ludex-tmp`.
        self.install_dir = self.install_dir.trim().to_string();
        self.download_dir = self.download_dir.trim().to_string();
        self.server_url = self.server_url.trim().trim_end_matches('/').to_string();
        self.games_dir = self
            .games_dir
            .take()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        self.archive_passwords = self
            .archive_passwords
            .drain(..)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect();
        self.discord_client_id = self.discord_client_id.trim().to_string();
        self.steam_root = self.steam_root.trim().to_string();
        self.compat_umu_path = self.compat_umu_path.trim().to_string();
        self.compat_proton_path = self.compat_proton_path.trim().to_string();
        self.compat_wine_path = self.compat_wine_path.trim().to_string();
        self.installer_silent_args = self
            .installer_silent_args
            .drain(..)
            .map(|set| {
                set.into_iter()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<String>>()
            })
            .filter(|set| !set.is_empty())
            .collect();
        // A zero/absurd timeout would either kill every installer instantly or
        // hang an install for a day.
        self.installer_timeout_secs = self.installer_timeout_secs.clamp(30, 7200);
    }

    pub fn install_path(&self) -> PathBuf {
        PathBuf::from(&self.install_dir)
    }

    pub fn download_path(&self) -> PathBuf {
        PathBuf::from(&self.download_dir)
    }
}

/// Reject obviously invalid settings with an actionable message.
pub fn validate(settings: &LocalSettings) -> Result<(), String> {
    if !settings.server_url.is_empty() {
        let lower = settings.server_url.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return Err(format!(
                "Server URL must start with http:// or https:// (got {}).",
                settings.server_url
            ));
        }
        if settings.server_url.chars().any(char::is_whitespace) {
            return Err("Server URL must not contain whitespace.".to_string());
        }
    }
    // A remote URL may be filled in a later wizard step; commands that need it
    // report a clear error themselves.
    Ok(())
}

/// Effective download directory for an install.
///
/// Priority: the per-install `downloadDir` override, then the user setting,
/// then `{installDir}/.ludex-tmp` so the archive lands on the same volume as
/// the extracted game.
pub fn resolve_download_dir(
    settings: &LocalSettings,
    install_root: &Path,
    override_dir: Option<&str>,
) -> PathBuf {
    if let Some(directory) = override_dir
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(directory);
    }
    let configured = settings.download_dir.trim();
    if !configured.is_empty() {
        return PathBuf::from(configured);
    }
    install_root.join(".ludex-tmp")
}

pub fn ensure_dirs(settings: &LocalSettings) -> Result<(), String> {
    let install_dir = settings.install_path();
    fs::create_dir_all(&install_dir).map_err(|e| {
        format!(
            "Could not create the install folder {}: {e}",
            install_dir.display()
        )
    })?;
    if !settings.download_dir.trim().is_empty() {
        let download_dir = settings.download_path();
        fs::create_dir_all(&download_dir).map_err(|e| {
            format!(
                "Could not create the download folder {}: {e}",
                download_dir.display()
            )
        })?;
    }
    if let Some(games_dir) = settings.games_dir.as_deref() {
        let path = Path::new(games_dir);
        fs::create_dir_all(path)
            .map_err(|e| format!("Could not create the games folder {}: {e}", path.display()))?;
    }
    Ok(())
}

/// Load settings, falling back to defaults when the file is missing or corrupt.
/// A corrupt file is preserved next to the original for inspection.
pub fn load(path: &Path, app_data: &Path) -> LocalSettings {
    match fs::read_to_string(path) {
        Ok(raw) => match serde_json::from_str::<LocalSettings>(&raw) {
            Ok(mut settings) => {
                settings.normalize(app_data);
                settings
            }
            Err(error) => {
                tracing::warn!(error = %error, path = %path.display(), "settings file is invalid, using defaults");
                let _ = fs::rename(path, path.with_extension("json.invalid"));
                LocalSettings::for_app_data(app_data)
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            LocalSettings::for_app_data(app_data)
        }
        Err(error) => {
            tracing::warn!(error = %error, path = %path.display(), "cannot read settings, using defaults");
            LocalSettings::for_app_data(app_data)
        }
    }
}

/// Write settings atomically: temp file in the same directory, fsync, rename.
pub fn save(path: &Path, settings: &LocalSettings) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Invalid settings path: {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|e| {
        format!(
            "Could not create the settings folder {}: {e}",
            parent.display()
        )
    })?;

    let tmp = path.with_extension("json.tmp");
    let payload = serde_json::to_vec_pretty(settings)
        .map_err(|e| format!("Could not serialize settings: {e}"))?;

    {
        let mut file = fs::File::create(&tmp)
            .map_err(|e| format!("Could not write settings to {}: {e}", tmp.display()))?;
        file.write_all(&payload)
            .map_err(|e| format!("Could not write settings to {}: {e}", tmp.display()))?;
        file.sync_all()
            .map_err(|e| format!("Could not flush settings to disk: {e}"))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The token lives in this file.
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }

    // `rename` replaces the destination atomically on Unix. On Windows it fails
    // when the destination exists, so retry after removing it.
    if let Err(first_error) = fs::rename(&tmp, path) {
        if path.exists() {
            fs::remove_file(path).map_err(|e| {
                format!(
                    "Could not replace {} (rename failed: {first_error}; remove failed: {e})",
                    path.display()
                )
            })?;
            fs::rename(&tmp, path)
                .map_err(|e| format!("Could not save settings to {}: {e}", path.display()))?;
        } else {
            return Err(format!(
                "Could not save settings to {}: {first_error}",
                path.display()
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_app_data() {
        let dir = Path::new("/tmp/ludex-test");
        let settings = LocalSettings::for_app_data(dir);
        assert!(settings.install_dir.ends_with("Games"));
        assert!(settings.download_dir.is_empty());
        assert_eq!(settings.mode, Mode::Local);
        assert!(!settings.keep_archives);
        assert!(settings.stream_install);

        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["streamInstall"], true);
        assert_eq!(json["downloadDir"], "");
        assert_eq!(json["keepArchives"], false);
        assert_eq!(json["bandwidthLimitKib"], 0);
        assert_eq!(json["closeToTray"], true);
        assert_eq!(json["updateChannel"], "stable");
        assert_eq!(json["checkUpdatesOnStartup"], true);
        assert_eq!(json["lastUpdateCheckAt"], serde_json::Value::Null);
        assert_eq!(json["discordPresence"], false);
        assert_eq!(json["discordClientId"], "");
        assert_eq!(json["installerSilentArgs"], serde_json::json!([]));
        assert_eq!(json["installerTimeoutSecs"], 600);
        assert_eq!(json["steamRoot"], "");
        assert_eq!(json["compatUmuPath"], "");
        assert_eq!(json["compatProtonPath"], "");
        assert_eq!(json["compatWinePath"], "");
        assert_eq!(json["defaultLaunchMode"], "auto");
    }

    #[test]
    fn installer_and_presence_settings_roundtrip() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.discord_presence = true;
        settings.discord_client_id = " 1234 ".into();
        settings.installer_silent_args = vec![
            vec![" /S ".into(), " ".into()],
            vec![],
            vec!["/VERYSILENT".into(), "/NORESTART".into()],
        ];
        settings.installer_timeout_secs = 5;
        settings.steam_root = " /steam ".into();
        settings.normalize(Path::new("/data"));

        assert_eq!(settings.discord_client_id, "1234");
        assert_eq!(
            settings.installer_silent_args,
            vec![
                vec!["/S".to_string()],
                vec!["/VERYSILENT".to_string(), "/NORESTART".to_string()]
            ]
        );
        // Clamped to the documented minimum.
        assert_eq!(settings.installer_timeout_secs, 30);
        assert_eq!(settings.steam_root, "/steam");

        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["discordPresence"], true);
        let back: LocalSettings = serde_json::from_value(json).unwrap();
        assert_eq!(back.installer_silent_args.len(), 2);
    }

    #[test]
    fn update_channel_roundtrips() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.update_channel = UpdateChannel::Early;
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["updateChannel"], "early");
        let back: LocalSettings = serde_json::from_value(json).unwrap();
        assert_eq!(back.update_channel, UpdateChannel::Early);
    }

    #[test]
    fn normalize_clears_blank_values() {
        let mut settings = LocalSettings {
            server_url: " http://127.0.0.1:1234/ ".into(),
            games_dir: Some("   ".into()),
            ..LocalSettings::default()
        };
        settings.normalize(Path::new("/tmp/ludex-test"));
        assert_eq!(settings.server_url, "http://127.0.0.1:1234");
        assert!(settings.games_dir.is_none());
        assert!(!settings.install_dir.is_empty());
        assert!(settings.download_dir.is_empty());
    }

    #[test]
    fn download_dir_priority() {
        let install_root = Path::new("/games");
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));

        assert_eq!(
            resolve_download_dir(&settings, install_root, None),
            PathBuf::from("/games/.ludex-tmp")
        );
        assert_eq!(
            resolve_download_dir(&settings, install_root, Some(" /tmp/custom ")),
            PathBuf::from("/tmp/custom")
        );

        settings.download_dir = "/var/tmp/ludex".into();
        assert_eq!(
            resolve_download_dir(&settings, install_root, None),
            PathBuf::from("/var/tmp/ludex")
        );
        assert_eq!(
            resolve_download_dir(&settings, install_root, Some("/tmp/custom")),
            PathBuf::from("/tmp/custom")
        );
    }

    #[test]
    fn compatibility_settings_are_trimmed_and_roundtrip() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.compat_umu_path = " /usr/bin/umu-run ".into();
        settings.compat_proton_path = " /opt/GE-Proton9-20 ".into();
        settings.compat_wine_path = " ".into();
        settings.default_launch_mode = LaunchMode::Proton;
        settings.normalize(Path::new("/data"));

        assert_eq!(settings.compat_umu_path, "/usr/bin/umu-run");
        assert_eq!(settings.compat_proton_path, "/opt/GE-Proton9-20");
        assert!(settings.compat_wine_path.is_empty());
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["defaultLaunchMode"], "proton");
        let back: LocalSettings = serde_json::from_value(json).unwrap();
        assert_eq!(back.default_launch_mode, LaunchMode::Proton);
    }

    #[test]
    fn partial_remote_settings_are_allowed() {
        let settings = LocalSettings {
            mode: Mode::Remote,
            ..LocalSettings::default()
        };
        assert!(validate(&settings).is_ok());
    }
}
