//! Persisted desktop settings (`{appDataDir}/settings.json`).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

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

/// What a named folder is used for (#99).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocationKind {
    /// Root of installed games: `{path}/{slug}/{version}`.
    Install,
    /// Temporary archives and staging copies.
    Download,
    /// Scratch space for extraction or repairs.
    Temp,
}

impl LocationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LocationKind::Install => "install",
            LocationKind::Download => "download",
            LocationKind::Temp => "temp",
        }
    }

    pub fn default_label(self) -> &'static str {
        match self {
            LocationKind::Install => "Games",
            LocationKind::Download => "Downloads",
            LocationKind::Temp => "Scratch",
        }
    }
}

/// A user-named install, download or temp folder (#99).
///
/// Locations are additive: a machine can keep a fast SSD for installs, a large
/// spinning disk for archives and a USB drive for one game. The install queue
/// resolves a location to a path when a job starts, so unplugging a location
/// only fails that job with an actionable error instead of breaking the queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Location {
    /// Stable id used by `install_game` (`locationId`). Filled by `normalize`.
    pub id: String,
    pub label: String,
    pub path: String,
    pub kind: LocationKind,
    /// The default folder for this kind. At most one per kind after normalize.
    pub default: bool,
}

impl Default for Location {
    fn default() -> Self {
        Self {
            id: String::new(),
            label: String::new(),
            path: String::new(),
            kind: LocationKind::Install,
            default: false,
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
    /// Where games are installed. Kept in sync with the default install
    /// location (#99); old UI payloads may set it directly.
    pub install_dir: String,
    /// Where temporary archives are downloaded. Kept in sync with the default
    /// download location; empty means `{installDir}/.ludex-tmp`.
    pub download_dir: String,
    /// Named install/download/temp folders (#99). Empty in old settings files;
    /// `normalize` migrates the two single folders above into entries.
    pub locations: Vec<Location>,
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
    /// Use the native title bar instead of Ludex's own (#111). Windows and Linux
    /// ship undecorated; this restores the OS title bar, Snap Layouts and the
    /// system window menu. Ignored on macOS, which always keeps its native
    /// traffic lights.
    pub native_title_bar: bool,
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
    /// Ludusavi manifest file used for save detection (#107). Empty means
    /// auto-discover the usual Ludusavi data folder; the manifest is never
    /// bundled with Ludex.
    pub ludusavi_manifest_path: String,
    /// `ludusavi` executable used for save detection (#107). Empty means look
    /// for `ludusavi` on PATH.
    pub ludusavi_binary: String,
    /// Wine/Proton prefix holding the game's `drive_c` (#107). Empty means
    /// auto-detect from the Steam compatdata folder, `$WINEPREFIX` or `~/.wine`.
    pub wine_prefix: String,
    /// Back up detected save files after each game session (#108). Per-game
    /// overrides live in SQLite; this is the global default.
    pub cloud_auto_sync: bool,
    /// Stable opaque identifier generated once per install and sent with every
    /// automatic snapshot (#108). Not a hardware fingerprint.
    pub cloud_device_id: String,
    /// Human-readable device name shown on snapshots (#108). `None` uses the
    /// host name, then "Desktop".
    pub cloud_device_label: Option<String>,
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
            locations: Vec::new(),
            server_port: 0,
            stream_install: true,
            keep_archives: false,
            archive_passwords: Vec::new(),
            bandwidth_limit_kib: 0,
            close_to_tray: true,
            native_title_bar: false,
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
            ludusavi_manifest_path: String::new(),
            ludusavi_binary: String::new(),
            wine_prefix: String::new(),
            cloud_auto_sync: true,
            cloud_device_id: String::new(),
            cloud_device_label: None,
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
        self.normalize_locations(app_data);
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
        self.ludusavi_manifest_path = self.ludusavi_manifest_path.trim().to_string();
        self.ludusavi_binary = self.ludusavi_binary.trim().to_string();
        self.wine_prefix = self.wine_prefix.trim().to_string();
        self.cloud_device_id = self.cloud_device_id.trim().to_string();
        self.cloud_device_label = self
            .cloud_device_label
            .take()
            .map(|value| value.trim().chars().take(60).collect::<String>())
            .filter(|value| !value.is_empty());
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

    /// Trim, deduplicate and migrate the named locations (#99).
    ///
    /// Rules:
    /// - an old settings file with only `installDir`/`downloadDir` becomes one
    ///   install location (default) plus one download location when set;
    /// - ids and labels are filled in, paths trimmed, duplicates dropped;
    /// - exactly one default per kind is kept;
    /// - `installDir`/`downloadDir` mirror the default locations so an older
    ///   web bundle keeps reading a coherent value.
    fn normalize_locations(&mut self, app_data: &Path) {
        for location in &mut self.locations {
            location.id = location.id.trim().to_string();
            location.label = location.label.trim().chars().take(60).collect();
            location.path = location.path.trim().to_string();
        }
        self.locations.retain(|location| !location.path.is_empty());

        let mut seen_ids = std::collections::HashSet::new();
        let mut seen_paths = std::collections::HashSet::new();
        self.locations.retain_mut(|location| {
            let path_key = format!(
                "{}:{}",
                location.kind.as_str(),
                location.path.replace('\\', "/")
            );
            if seen_paths.contains(&path_key) {
                return false;
            }
            seen_paths.insert(path_key);
            if location.id.is_empty() {
                location.id = format!("loc-{}", uuid::Uuid::new_v4().simple());
            } else if seen_ids.contains(&location.id) {
                // Duplicate id: the first entry is the one commands can still
                // resolve; keeping a second one would make the picker ambiguous.
                return false;
            }
            seen_ids.insert(location.id.clone());
            true
        });

        if self.locations.is_empty() {
            let install_path = if self.install_dir.trim().is_empty() {
                app_data.join("Games").to_string_lossy().to_string()
            } else {
                self.install_dir.trim().to_string()
            };
            self.locations.push(Location {
                id: "games".to_string(),
                label: "Games".to_string(),
                path: install_path,
                kind: LocationKind::Install,
                default: true,
            });
            if !self.download_dir.trim().is_empty() {
                self.locations.push(Location {
                    id: "downloads".to_string(),
                    label: "Downloads".to_string(),
                    path: self.download_dir.trim().to_string(),
                    kind: LocationKind::Download,
                    default: true,
                });
            }
        }

        for kind in [
            LocationKind::Install,
            LocationKind::Download,
            LocationKind::Temp,
        ] {
            let mut found_default = false;
            for location in self.locations.iter_mut().filter(|item| item.kind == kind) {
                if location.label.is_empty() {
                    location.label = kind.default_label().to_string();
                }
                if location.default && !found_default {
                    found_default = true;
                } else if location.default {
                    location.default = false;
                }
            }
            if !found_default {
                if let Some(location) = self.locations.iter_mut().find(|item| item.kind == kind) {
                    location.default = true;
                }
            }
        }

        if let Some(path) = self
            .default_location(LocationKind::Install)
            .map(|location| location.path.clone())
        {
            self.install_dir = path;
        }
        self.download_dir = self
            .default_location(LocationKind::Download)
            .map(|location| location.path.clone())
            .unwrap_or_default();
    }

    pub fn location(&self, id: &str) -> Option<&Location> {
        self.locations
            .iter()
            .find(|location| !id.trim().is_empty() && location.id == id.trim())
    }

    pub fn default_location(&self, kind: LocationKind) -> Option<&Location> {
        self.locations
            .iter()
            .find(|location| location.kind == kind && location.default)
            .or_else(|| self.locations.iter().find(|location| location.kind == kind))
    }

    /// Resolve a location id to a path. `None` returns the default location of
    /// that kind (falling back to the legacy single folder).
    pub fn resolve_location(
        &self,
        id: Option<&str>,
        kind: LocationKind,
    ) -> Result<PathBuf, String> {
        if let Some(id) = id.map(str::trim).filter(|value| !value.is_empty()) {
            let location = self.location(id).ok_or_else(|| {
                format!("The selected location no longer exists (id {id}). Pick another one in Settings → Locations.")
            })?;
            if location.path.trim().is_empty() {
                return Err(format!(
                    "The location “{}” has no folder. Add one in Settings → Locations.",
                    location.label
                ));
            }
            return Ok(PathBuf::from(location.path.trim()));
        }
        match kind {
            LocationKind::Install => Ok(self.install_path()),
            LocationKind::Download => {
                let configured = self.download_dir.trim();
                if configured.is_empty() {
                    Err("No download location is configured.".to_string())
                } else {
                    Ok(PathBuf::from(configured))
                }
            }
            LocationKind::Temp => Ok(std::env::temp_dir().join("ludex")),
        }
    }

    pub fn install_path(&self) -> PathBuf {
        self.default_location(LocationKind::Install)
            .map(|location| PathBuf::from(location.path.trim()))
            .unwrap_or_else(|| PathBuf::from(&self.install_dir))
    }

    pub fn download_path(&self) -> PathBuf {
        self.default_location(LocationKind::Download)
            .map(|location| PathBuf::from(location.path.trim()))
            .unwrap_or_else(|| PathBuf::from(&self.download_dir))
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
    for location in &settings.locations {
        crate::locations::validate(&location.label, &location.path)?;
    }
    // A remote URL may be filled in a later wizard step; commands that need it
    // report a clear error themselves.
    Ok(())
}

/// Apply an old-style `installDir`/`downloadDir` patch to the merged settings
/// JSON (#99). The new UI sends `locations`; an older bundle only knows the two
/// single folders, so map those onto the default location of the matching kind
/// instead of silently ignoring them.
pub fn apply_legacy_folder_patch(
    merged: &mut Value,
    install_dir: Option<&str>,
    download_dir: Option<&str>,
) {
    let Some(root) = merged.as_object_mut() else {
        return;
    };
    let locations = root
        .entry("locations")
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(locations) = locations.as_array_mut() else {
        return;
    };

    let mut set_path = |kind: &str, label: &str, path: &str| {
        let trimmed = path.trim();
        if trimmed.is_empty() && kind != "download" {
            return;
        }
        let existing = locations
            .iter()
            .position(|location| {
                location.get("kind").and_then(Value::as_str) == Some(kind)
                    && location.get("default").and_then(Value::as_bool) == Some(true)
            })
            .or_else(|| {
                locations
                    .iter()
                    .position(|location| location.get("kind").and_then(Value::as_str) == Some(kind))
            });
        match existing {
            Some(index) => {
                if let Some(entry) = locations[index].as_object_mut() {
                    entry.insert("path".to_string(), Value::String(trimmed.to_string()));
                    entry.insert("default".to_string(), Value::Bool(true));
                }
            }
            None if !trimmed.is_empty() => locations.push(serde_json::json!({
                "id": "",
                "label": label,
                "path": trimmed,
                "kind": kind,
                "default": true,
            })),
            None => {}
        }
    };

    if let Some(path) = install_dir {
        set_path("install", "Games", path);
    }
    if let Some(path) = download_dir {
        set_path("download", "Downloads", path);
    }
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

/// Best-effort folder creation.
///
/// A location can be a USB drive that is not plugged in right now: failing the
/// whole settings save (or the queue) because one folder is missing would be
/// worse than reporting it when the folder is actually used. Installs validate
/// the selected location themselves and fail with an actionable message.
pub fn ensure_dirs(settings: &LocalSettings) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for location in &settings.locations {
        let path = PathBuf::from(location.path.trim());
        if location.path.trim().is_empty() || !seen.insert(path.clone()) {
            continue;
        }
        if let Err(error) = fs::create_dir_all(&path) {
            tracing::warn!(
                error = %error,
                path = %path.display(),
                label = %location.label,
                "could not create the location folder; it will be checked when used"
            );
        }
    }
    if settings.locations.is_empty() {
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
        assert_eq!(json["nativeTitleBar"], false);
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
        assert_eq!(json["ludusaviManifestPath"], "");
        assert_eq!(json["ludusaviBinary"], "");
        assert_eq!(json["winePrefix"], "");
        assert_eq!(json["cloudAutoSync"], true);
        assert_eq!(json["cloudDeviceId"], "");
        assert_eq!(json["cloudDeviceLabel"], serde_json::Value::Null);
    }

    #[test]
    fn cloud_settings_are_trimmed_and_roundtrip() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.cloud_device_id = " device-123 ".into();
        settings.cloud_device_label = Some("  Living room PC  ".into());
        settings.normalize(Path::new("/data"));

        assert_eq!(settings.cloud_device_id, "device-123");
        assert_eq!(
            settings.cloud_device_label.as_deref(),
            Some("Living room PC")
        );

        settings.cloud_device_label = Some("   ".into());
        settings.normalize(Path::new("/data"));
        assert!(settings.cloud_device_label.is_none());

        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["cloudDeviceId"], "device-123");
        assert_eq!(json["cloudAutoSync"], true);
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
        settings.ludusavi_manifest_path = " /data/manifest.yaml ".into();
        settings.ludusavi_binary = " /usr/bin/ludusavi ".into();
        settings.wine_prefix = " /prefix/pfx ".into();
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
        assert_eq!(settings.ludusavi_manifest_path, "/data/manifest.yaml");
        assert_eq!(settings.ludusavi_binary, "/usr/bin/ludusavi");
        assert_eq!(settings.wine_prefix, "/prefix/pfx");

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
    fn native_title_bar_roundtrips() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        // The custom (undecorated) title bar is the default on Windows and Linux.
        assert!(!settings.native_title_bar);

        settings.native_title_bar = true;
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["nativeTitleBar"], true);
        let back: LocalSettings = serde_json::from_value(json).unwrap();
        assert!(back.native_title_bar);

        // Settings files written before #111 have no such key: keep the custom bar.
        let legacy: LocalSettings =
            serde_json::from_value(serde_json::json!({ "closeToTray": false })).unwrap();
        assert!(!legacy.native_title_bar);
        assert!(!legacy.close_to_tray);
    }

    #[test]
    fn partial_remote_settings_are_allowed() {
        let settings = LocalSettings {
            mode: Mode::Remote,
            ..LocalSettings::default()
        };
        assert!(validate(&settings).is_ok());
    }

    #[test]
    fn old_single_folders_migrate_to_locations() {
        // A settings file written before #99 has no `locations` key at all.
        let raw = serde_json::json!({
            "mode": "local",
            "installDir": "/mnt/games",
            "downloadDir": "/mnt/tmp",
            "serverUrl": "",
            "streamInstall": false
        });
        let mut settings: LocalSettings = serde_json::from_value(raw).unwrap();
        settings.normalize(Path::new("/data"));

        assert_eq!(settings.locations.len(), 2);
        let install = settings.default_location(LocationKind::Install).unwrap();
        assert_eq!(install.path, "/mnt/games");
        assert!(install.default);
        assert_eq!(install.label, "Games");
        let download = settings.default_location(LocationKind::Download).unwrap();
        assert_eq!(download.path, "/mnt/tmp");
        assert!(download.default);
        // The legacy fields mirror the defaults for older bundles.
        assert_eq!(settings.install_dir, "/mnt/games");
        assert_eq!(settings.download_dir, "/mnt/tmp");
    }

    #[test]
    fn migration_keeps_one_default_per_kind() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.locations = vec![
            Location {
                id: "a".into(),
                label: "A".into(),
                path: "/a".into(),
                kind: LocationKind::Install,
                default: true,
            },
            Location {
                id: "b".into(),
                label: "B".into(),
                path: "/b".into(),
                kind: LocationKind::Install,
                default: true,
            },
            Location {
                id: "c".into(),
                label: "".into(),
                path: "/c".into(),
                kind: LocationKind::Temp,
                default: false,
            },
            Location {
                id: "a".into(),
                label: "Duplicate".into(),
                path: "/d".into(),
                kind: LocationKind::Install,
                default: false,
            },
        ];
        settings.normalize(Path::new("/data"));

        let installs: Vec<&Location> = settings
            .locations
            .iter()
            .filter(|location| location.kind == LocationKind::Install)
            .collect();
        assert_eq!(installs.len(), 2);
        assert_eq!(installs.iter().filter(|item| item.default).count(), 1);
        assert_eq!(installs[0].path, "/a");
        assert!(installs[0].default);
        let temp = settings
            .locations
            .iter()
            .find(|location| location.kind == LocationKind::Temp)
            .unwrap();
        assert_eq!(temp.label, "Scratch");
        assert!(temp.default);
        assert_eq!(settings.install_dir, "/a");
    }

    #[test]
    fn legacy_folder_patch_updates_the_default_location() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.normalize(Path::new("/data"));
        let mut merged = serde_json::to_value(&settings).unwrap();

        apply_legacy_folder_patch(&mut merged, Some("/ssd/games"), Some("/big/tmp"));
        let mut next: LocalSettings = serde_json::from_value(merged).unwrap();
        next.normalize(Path::new("/data"));

        assert_eq!(next.install_dir, "/ssd/games");
        assert_eq!(next.download_dir, "/big/tmp");
        assert_eq!(
            next.default_location(LocationKind::Install).unwrap().path,
            "/ssd/games"
        );
        assert_eq!(next.locations.len(), 2);
    }

    #[test]
    fn resolved_locations_ignore_empty_ids_and_reject_unknown_ones() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.normalize(Path::new("/data"));
        assert!(settings
            .resolve_location(Some("  "), LocationKind::Install)
            .is_ok());
        assert!(settings
            .resolve_location(Some("gone"), LocationKind::Install)
            .unwrap_err()
            .contains("no longer exists"));
    }
}
