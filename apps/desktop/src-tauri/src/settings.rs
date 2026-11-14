//! Persisted desktop settings (`{appDataDir}/settings.json`).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::linux_gaming::LaunchMode;
use crate::profiles::ServerProfile;

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

/// Gesture that opens the in-game overlay (#144).
///
/// `doubleHome` is the Steam-like double press of Guide/Home/PS; the LB+RB+Start
/// chord stays available as a fallback for pads whose Guide button never reaches
/// the host. `chord` leaves the Guide button to the OS/Steam. `keyboard` turns
/// both pad gestures off and keeps Shift+Tab only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum OverlayTrigger {
    #[default]
    DoubleHome,
    Chord,
    Keyboard,
}

impl UpdateChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            UpdateChannel::Stable => "stable",
            UpdateChannel::Early => "early",
        }
    }
}

/// Where a new install stages its temporary archive (#226).
///
/// `configured` keeps the historical behaviour: the default download location,
/// then `downloadDir`, then `{installDir}/.ludex-tmp`. `installVolume` always
/// writes next to the install root, so a NAS `downloadDir` no longer forces the
/// extraction to read the archive back over SMB.
///
/// The policy only decides where a **new** job writes. A job that already has a
/// stored `.part` keeps it exactly (`install::planned_paths`), so flipping this
/// setting never moves a resumable download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum DownloadStaging {
    #[default]
    Configured,
    InstallVolume,
}

impl DownloadStaging {
    pub fn as_str(self) -> &'static str {
        match self {
            DownloadStaging::Configured => "configured",
            DownloadStaging::InstallVolume => "installVolume",
        }
    }

    /// Parse the setting, falling back to `configured` for anything unknown.
    ///
    /// A newer web bundle or a hand-edited file must not fail the whole
    /// settings document: `load` would otherwise replace every setting with
    /// defaults. The known value is matched case-insensitively.
    pub fn parse(raw: &str) -> Self {
        if raw
            .trim()
            .eq_ignore_ascii_case(DownloadStaging::InstallVolume.as_str())
        {
            DownloadStaging::InstallVolume
        } else {
            DownloadStaging::Configured
        }
    }
}

impl<'de> Deserialize<'de> for DownloadStaging {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // `null` and an unknown string both mean "not chosen": keep the
        // historical staging instead of rejecting the payload.
        let raw = Option::<String>::deserialize(deserializer)?;
        Ok(raw
            .as_deref()
            .map(DownloadStaging::parse)
            .unwrap_or_default())
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
    /// True once the user confirmed an install root (quick-add in Settings →
    /// Storage, or an explicit folder edit). New installs then keep the saved
    /// default instead of preselecting the detected-drive recommendation
    /// (#145). Old settings files default to `false` and are never migrated
    /// silently.
    #[serde(default)]
    pub install_root_confirmed: bool,
    /// Where temporary archives are downloaded. Kept in sync with the default
    /// download location; empty means `{installDir}/.ludex-tmp`.
    pub download_dir: String,
    /// Where a new install stages its archive (#226). `configured` uses the
    /// download folder above; `installVolume` writes under
    /// `{installRoot}/.ludex-tmp` so extraction reads it locally.
    #[serde(default)]
    pub download_staging: DownloadStaging,
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
    /// Global download bandwidth cap in KiB/s. 0 = unlimited. The UI presents
    /// it in Mb/s (#199); the stored unit stays KiB/s so old settings keep working.
    pub bandwidth_limit_kib: u64,
    /// Abort a silent download response after this many seconds and resume
    /// with Range (#195). Clamped to 5..=600 when used.
    #[serde(default = "default_download_stall_timeout_secs")]
    pub download_stall_timeout_secs: u64,
    /// How long a failed connection keeps retrying before the job fails
    /// (#212), in seconds. Default 6 h; 0 = keep retrying while the job is
    /// active. Permanent errors stop immediately regardless.
    #[serde(default = "default_download_retry_window_secs")]
    pub download_retry_window_secs: u64,
    /// Re-enter incomplete downloads in the queue after an app restart (#212).
    /// Never applies to a user-paused or cancelled job.
    #[serde(default = "default_auto_resume_downloads")]
    pub auto_resume_downloads: bool,
    /// Parallel archive downloads (#221): 1 (default) or 2. Extraction is
    /// always serialised, so raising it only overlaps two transfers, never two
    /// unpackers; every slot is still gated by the disk-space ledger (#222).
    #[serde(default = "default_download_concurrency")]
    pub download_concurrency: u64,
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
    /// Version the user asked Ludex to stop prompting about (#163). The update
    /// stays installable from the update center; only notifications and the
    /// startup prompt are silenced, and a newer version prompts again.
    #[serde(default)]
    pub skipped_update_version: Option<String>,
    /// Version an OS notification was already raised for (#213). A new check,
    /// relaunch or second window must not repeat the same banner.
    #[serde(default)]
    pub notified_update_version: Option<String>,
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
    /// Download the canonical Ludusavi save-path manifest automatically (#153).
    /// On by default so a fresh install detects saves without installing
    /// Ludusavi; the startup check is throttled to once per 24 h.
    #[serde(default = "default_ludusavi_manifest_auto_update")]
    pub ludusavi_manifest_auto_update: bool,
    /// Mirror or pinned URL for the manifest download (#153). Empty uses the
    /// canonical upstream file.
    #[serde(default)]
    pub ludusavi_manifest_url: String,
    /// Wine/Proton prefix holding the game's `drive_c` (#107). Empty means
    /// auto-detect from the Steam compatdata folder, `$WINEPREFIX` or `~/.wine`.
    pub wine_prefix: String,
    /// Back up detected save files after each game session (#108). Per-game
    /// overrides live in SQLite; this is the global default.
    pub cloud_auto_sync: bool,
    /// Largest folder Ludex may promote automatically from a guessed save
    /// location (#180), in MiB. A broader folder is skipped with a reason and
    /// stays visible for a manual override. 0 disables the cap.
    #[serde(default = "default_cloud_save_folder_cap_mb")]
    pub cloud_save_folder_cap_mb: u64,
    /// Stable opaque identifier generated once per install and sent with every
    /// automatic snapshot (#108). Not a hardware fingerprint.
    pub cloud_device_id: String,
    /// Human-readable device name shown on snapshots (#108). `None` uses the
    /// host name, then "Desktop".
    pub cloud_device_label: Option<String>,
    /// Saved Ludex servers (#24). Metadata only; tokens live in the OS keychain
    /// or the SQLite fallback (`secrets.rs`).
    #[serde(default)]
    pub servers: Vec<ServerProfile>,
    /// Id of the profile currently backing `serverUrl`/`token`.
    #[serde(default)]
    pub active_server_id: String,
    /// `apps.json` override for the Sunshine integration (#97). Empty = detect.
    #[serde(default)]
    pub sunshine_apps_path: String,
    /// `sunshine.conf` override (#97). Empty = next to `apps.json`.
    #[serde(default)]
    pub sunshine_conf_path: String,
    /// Moonlight binary override (#98). Empty = detect.
    #[serde(default)]
    pub moonlight_binary: String,
    /// Moonlight config override (#98). Empty = detect.
    #[serde(default)]
    pub moonlight_config_path: String,
    /// Manual Moonlight host (name or IP) for builds Ludex cannot read (#151).
    /// Empty = use the hosts Moonlight saved.
    #[serde(default)]
    pub moonlight_host: String,
    /// Publish one `Ludex` console app to Sunshine instead of one app per game
    /// (#172). Off by default so an existing per-game setup is unchanged.
    #[serde(default)]
    pub console_mode: bool,
    /// Sunshine name of the console app (#172). Empty = `Ludex`.
    #[serde(default)]
    pub console_app_name: String,
    /// Start the console app fullscreen (default). Off publishes `--windowed`.
    #[serde(default = "default_console_fullscreen")]
    pub console_fullscreen: bool,
    /// In-console chord that quits back to the desktop (#172), e.g. `f12`.
    #[serde(default = "default_console_exit_shortcut")]
    pub console_exit_shortcut: String,
    /// Expert option: keep publishing one Sunshine app per game in addition to
    /// the console app (#172). Off by default; ignored while `consoleMode` is
    /// off (per-game publishing is then always on).
    #[serde(default)]
    pub console_per_game_apps: bool,
    /// Ordered GameVault install roots (#134). One entry per install location,
    /// e.g. `C:\Games\GameVault` and `D:\Games\GameVault`.
    #[serde(default)]
    pub gamevault_roots: Vec<String>,
    /// Legacy single GameVault root (#134). Only read to migrate an older
    /// settings file or an older web bundle; never written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gamevault_root: Option<String>,
    /// Folder depth used by GameVault folder scans (#134), clamped 4..=8.
    #[serde(default = "default_gamevault_scan_depth")]
    pub gamevault_scan_depth: u8,
    /// Launcher sources `launcher_scan` may read (#155), in display order.
    ///
    /// Every source is on by default, and a settings file written before #155
    /// has no key at all, so it deserializes to the full list. An empty list is
    /// meaningful: the user unchecked every source. Turning one off only stops
    /// the scan from reading it; rows already imported from it stay in the
    /// local index.
    #[serde(default = "default_launcher_sources")]
    pub launcher_sources: Vec<String>,
    /// Ludex UI language, mirrored from the web locale (#148). Only read when a
    /// game opted into config sync, so a launch can apply the language without
    /// the web view. Empty means "not known yet".
    #[serde(default)]
    pub ui_language: String,
    /// Ludex profile display name (or username), mirrored from the web profile
    /// (#148) for the same launch-time config sync. Empty means "not set".
    #[serde(default)]
    pub player_name: String,
    /// Account preference mirror (#201): the theme selection, Big Picture
    /// layout, library view, sound/haptics and update prompt the account
    /// document carries. The web app writes these; device paths, bandwidth,
    /// overlay settings, tokens and server profiles are never mirrored here.
    #[serde(default)]
    pub account_theme: String,
    /// Big Picture layout from the account document (`shelf`, `hero`, `grid`
    /// or `compact`); empty means "not known yet".
    #[serde(default)]
    pub account_big_picture_layout: String,
    /// Default library view from the account document (`grid` or `list`).
    #[serde(default)]
    pub account_library_view: String,
    /// Big Picture sound cues from the account document.
    #[serde(default)]
    pub account_sound: Option<bool>,
    /// Big Picture haptics from the account document.
    #[serde(default)]
    pub account_haptics: Option<bool>,
    /// Update prompt choice from the account document (`always`, `notify` or
    /// `off`). The channel and the skipped version stay device-local.
    #[serde(default)]
    pub account_update_prompt: String,
    /// Update notification channels from the account document (`app-and-os`,
    /// `app` or `off`, #213). Read before raising an OS banner; the webview can
    /// be in the tray when a startup check finds a new build.
    #[serde(default)]
    pub account_update_notifications: String,
    /// In-game controller overlay (#144). Enabling it never pauses the running
    /// game and sends nothing anywhere.
    #[serde(default = "default_overlay_enabled")]
    pub overlay_enabled: bool,
    /// Gesture recognised by the native global gamepad listener (#144).
    #[serde(default)]
    pub overlay_trigger: OverlayTrigger,
    /// Opacity of the overlay panel background, 0.35..=1.0 (#144).
    #[serde(default = "default_overlay_opacity")]
    pub overlay_opacity: f64,
    /// Size of the overlay panel relative to the screen, 0.6..=1.3 (#144).
    #[serde(default = "default_overlay_scale")]
    pub overlay_scale: f64,
    /// Content tabs shown in the overlay, in canonical order (#144). The game
    /// tab is always drawn; an empty list therefore means "game tab only".
    #[serde(default = "default_overlay_tabs")]
    pub overlay_tabs: Vec<String>,
    /// Games where the overlay stays off, for anti-cheat-sensitive titles
    /// (#144). The list is local to this device.
    #[serde(default)]
    pub overlay_disabled_games: Vec<i64>,
    /// Opt-in anonymous diagnostics and crash reports (#171). Off by default;
    /// nothing is ever uploaded without this flag.
    #[serde(default)]
    pub diagnostics_enabled: bool,
    /// Random install id generated locally when diagnostics are first used.
    /// Not derived from hardware, the account or the hostname (#171).
    #[serde(default)]
    pub diagnostics_install_id: String,
    /// Stable device-registry id (#184), generated once in local settings. It
    /// identifies this PC to the phone's remote-install screen; it is not a
    /// hardware fingerprint and never replaces the account token.
    #[serde(default)]
    pub device_id: String,
    /// Editable display name shown on the phone ("Gaming PC", "Bureau").
    /// Empty falls back to the host name, then "Desktop".
    #[serde(default)]
    pub device_name: Option<String>,
}

/// The overlay is on by default: it is the Steam-like behaviour players expect,
/// and it never touches the running game process.
pub const fn default_overlay_enabled() -> bool {
    true
}

pub const fn default_overlay_opacity() -> f64 {
    0.92
}

pub const fn default_overlay_scale() -> f64 {
    1.0
}

/// Every tab enabled for a fresh install.
pub fn default_overlay_tabs() -> Vec<String> {
    OVERLAY_TABS.iter().map(|tab| (*tab).to_string()).collect()
}

/// Canonical overlay tab ids, in display order.
pub const OVERLAY_TABS: [&str; 4] = ["game", "library", "downloads", "achievements"];

/// Keep known tab ids, deduplicated and in display order.
///
/// An empty input stays empty ("game tab only"); an unknown id is dropped
/// instead of being persisted, so a future tab can be added without breaking an
/// older settings file.
pub fn canonical_overlay_tabs(tabs: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for id in OVERLAY_TABS {
        if tabs.iter().any(|tab| tab.trim().eq_ignore_ascii_case(id)) {
            kept.push(id.to_string());
        }
    }
    kept
}

/// Clamp the overlay background opacity and replace non-finite values.
pub fn sanitize_overlay_opacity(value: f64) -> f64 {
    if !value.is_finite() {
        return default_overlay_opacity();
    }
    let clamped = value.clamp(0.35, 1.0);
    (clamped * 100.0).round() / 100.0
}

/// Clamp the overlay window scale and replace non-finite values.
pub fn sanitize_overlay_scale(value: f64) -> f64 {
    if !value.is_finite() {
        return default_overlay_scale();
    }
    let clamped = value.clamp(0.6, 1.3);
    (clamped * 100.0).round() / 100.0
}

/// Default executable search depth for folder scans (#134).
pub const fn default_gamevault_scan_depth() -> u8 {
    6
}

/// Every launcher source enabled (#155): the default for a fresh install and
/// for a settings file written before the key existed.
pub fn default_launcher_sources() -> Vec<String> {
    crate::launchers::LauncherSet::all()
        .ids()
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// The manifest download is on by default: a fresh install has no cached copy,
/// and save detection without it reports "no manifest" (#153).
pub const fn default_ludusavi_manifest_auto_update() -> bool {
    true
}

/// A guessed save folder above 2 GiB is almost certainly a broad folder
/// (Documents, a whole library); it is skipped instead of archived (#180).
pub const fn default_cloud_save_folder_cap_mb() -> u64 {
    2048
}

/// Seconds without a byte before a download response is abandoned and resumed
/// with Range (#195).
pub const fn default_download_stall_timeout_secs() -> u64 {
    30
}

/// How long a download keeps retrying a transient failure before giving up
/// (#212): six hours, long enough for a 40 GB install over a home line.
pub const fn default_download_retry_window_secs() -> u64 {
    6 * 60 * 60
}

/// Parallel downloads (#221). One by default: the pipeline already overlaps
/// the next download with the current extraction, and two transfers only help
/// on a fast link with a slow disk.
pub const fn default_download_concurrency() -> u64 {
    1
}

/// Resume incomplete downloads after an app restart (#212). On by default:
/// the job was already user-intended, and pausing exists for the opt-out.
pub const fn default_auto_resume_downloads() -> bool {
    true
}

/// A console session is meant for a TV: fullscreen by default (#172).
pub const fn default_console_fullscreen() -> bool {
    true
}

/// The default way out of a console session (#172).
pub fn default_console_exit_shortcut() -> String {
    crate::console::DEFAULT_EXIT_SHORTCUT.to_string()
}

impl Default for LocalSettings {
    fn default() -> Self {
        Self {
            mode: Mode::Local,
            server_url: String::new(),
            token: None,
            games_dir: None,
            install_dir: String::new(),
            install_root_confirmed: false,
            download_dir: String::new(),
            download_staging: DownloadStaging::Configured,
            locations: Vec::new(),
            server_port: 0,
            stream_install: true,
            keep_archives: false,
            archive_passwords: Vec::new(),
            bandwidth_limit_kib: 0,
            download_stall_timeout_secs: default_download_stall_timeout_secs(),
            download_retry_window_secs: default_download_retry_window_secs(),
            auto_resume_downloads: default_auto_resume_downloads(),
            download_concurrency: default_download_concurrency(),
            close_to_tray: true,
            native_title_bar: false,
            update_channel: UpdateChannel::Stable,
            check_updates_on_startup: true,
            last_update_check_at: None,
            skipped_update_version: None,
            notified_update_version: None,
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
            ludusavi_manifest_auto_update: default_ludusavi_manifest_auto_update(),
            ludusavi_manifest_url: String::new(),
            wine_prefix: String::new(),
            cloud_auto_sync: true,
            cloud_save_folder_cap_mb: default_cloud_save_folder_cap_mb(),
            cloud_device_id: String::new(),
            cloud_device_label: None,
            servers: Vec::new(),
            active_server_id: String::new(),
            sunshine_apps_path: String::new(),
            sunshine_conf_path: String::new(),
            moonlight_binary: String::new(),
            moonlight_config_path: String::new(),
            moonlight_host: String::new(),
            console_mode: false,
            console_app_name: crate::console::DEFAULT_APP_NAME.to_string(),
            console_fullscreen: default_console_fullscreen(),
            console_exit_shortcut: default_console_exit_shortcut(),
            console_per_game_apps: false,
            gamevault_roots: Vec::new(),
            gamevault_root: None,
            gamevault_scan_depth: default_gamevault_scan_depth(),
            launcher_sources: default_launcher_sources(),
            ui_language: String::new(),
            player_name: String::new(),
            account_theme: String::new(),
            account_big_picture_layout: String::new(),
            account_library_view: String::new(),
            account_sound: None,
            account_haptics: None,
            account_update_prompt: String::new(),
            account_update_notifications: String::new(),
            overlay_enabled: default_overlay_enabled(),
            overlay_trigger: OverlayTrigger::DoubleHome,
            overlay_opacity: default_overlay_opacity(),
            overlay_scale: default_overlay_scale(),
            overlay_tabs: default_overlay_tabs(),
            overlay_disabled_games: Vec::new(),
            diagnostics_enabled: false,
            diagnostics_install_id: String::new(),
            device_id: String::new(),
            device_name: None,
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
        if self.download_stall_timeout_secs == 0 {
            self.download_stall_timeout_secs = default_download_stall_timeout_secs();
        }
        self.download_stall_timeout_secs = self.download_stall_timeout_secs.clamp(5, 600);
        // #212: 0 is meaningful ("retry while the app is open"), anything else
        // is bounded to a sane range so a typo cannot mean ten years.
        if self.download_concurrency == 0 {
            self.download_concurrency = default_download_concurrency();
        }
        self.download_concurrency = self
            .download_concurrency
            .clamp(1, crate::state::MAX_CONCURRENT_DOWNLOADS as u64);
        if self.download_retry_window_secs != 0 {
            self.download_retry_window_secs =
                self.download_retry_window_secs.clamp(60, 7 * 24 * 60 * 60);
        }
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
        self.ludusavi_manifest_url = self
            .ludusavi_manifest_url
            .trim()
            .trim_end_matches('/')
            .to_string();
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
        self.sunshine_apps_path = self.sunshine_apps_path.trim().to_string();
        self.sunshine_conf_path = self.sunshine_conf_path.trim().to_string();
        self.moonlight_binary = self.moonlight_binary.trim().to_string();
        self.moonlight_config_path = self.moonlight_config_path.trim().to_string();
        self.moonlight_host = self.moonlight_host.trim().to_string();
        // #172: the console app name is a Sunshine label, not free-form copy;
        // keep it short and fall back to the default when blank.
        self.console_app_name = crate::console::app_name(&self.console_app_name)
            .chars()
            .take(48)
            .collect();
        self.console_exit_shortcut = crate::console::exit_shortcut(&self.console_exit_shortcut)
            .chars()
            .take(32)
            .collect();
        self.normalize_gamevault_roots();
        // Keep known sources only, deduplicated and in display order; an empty
        // set stays empty ("none enabled"), it never silently becomes "all".
        self.launcher_sources = crate::launchers::canonical_ids(&self.launcher_sources);
        self.ui_language = self
            .ui_language
            .trim()
            .chars()
            .take(16)
            .collect::<String>()
            .to_lowercase();
        self.player_name = self.player_name.trim().chars().take(60).collect();
        // #144: the overlay panel cannot be fully transparent (the game would
        // be unreachable) or so large that it hides the game; clamp both and
        // drop bad floats from a hand-edited settings file.
        self.overlay_opacity = sanitize_overlay_opacity(self.overlay_opacity);
        self.overlay_scale = sanitize_overlay_scale(self.overlay_scale);
        self.overlay_tabs = canonical_overlay_tabs(&self.overlay_tabs);
        let mut seen_games = std::collections::HashSet::new();
        self.overlay_disabled_games = self
            .overlay_disabled_games
            .drain(..)
            .filter(|game_id| *game_id > 0)
            .filter(|game_id| seen_games.insert(*game_id))
            .take(500)
            .collect();
        self.overlay_disabled_games.sort_unstable();
        // #171: the install id is local and random; keep it to the charset the
        // server accepts so a hand-edited settings file cannot break sending.
        self.diagnostics_install_id = self
            .diagnostics_install_id
            .trim()
            .chars()
            .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
            .take(64)
            .collect();
        // #184: the device id is local, random and only used as a routing key.
        // Keep the same charset the server accepts; an empty id is generated
        // on first heartbeat, never in `normalize` (a save from the web UI must
        // not invent identities silently).
        self.device_id = self
            .device_id
            .trim()
            .chars()
            .filter(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
            })
            .take(128)
            .collect();
        self.device_name = self
            .device_name
            .take()
            .map(|value| value.trim().chars().take(80).collect::<String>())
            .filter(|value| !value.is_empty());
        // #163: a skipped version is a display preference, never a security
        // decision; trim it and drop blanks so the field serialises cleanly.
        self.skipped_update_version = self
            .skipped_update_version
            .take()
            .map(|value| value.trim().chars().take(64).collect::<String>())
            .filter(|value| !value.is_empty());
        // #213: same rule for the version an OS banner already covered.
        self.notified_update_version = self
            .notified_update_version
            .take()
            .map(|value| value.trim().chars().take(64).collect::<String>())
            .filter(|value| !value.is_empty());
        self.active_server_id = self.active_server_id.trim().to_string();
        self.servers = self
            .servers
            .drain(..)
            .filter_map(|mut profile| {
                profile.id = profile.id.trim().to_string();
                profile.url = profile.url.trim().trim_end_matches('/').to_string();
                profile.label = profile.label.trim().chars().take(60).collect();
                if profile.id.is_empty() || profile.url.is_empty() {
                    return None;
                }
                if profile.label.is_empty() {
                    profile.label = profile.url.clone();
                }
                profile.username = profile
                    .username
                    .take()
                    .map(|value| value.trim().chars().take(80).collect::<String>())
                    .filter(|value| !value.is_empty());
                Some(profile)
            })
            .collect();
        let mut seen = std::collections::HashSet::new();
        self.servers
            .retain(|profile| seen.insert(profile.id.clone()));
        if !self
            .servers
            .iter()
            .any(|profile| profile.id == self.active_server_id)
        {
            self.active_server_id.clear();
        }
    }

    /// Trim, deduplicate and migrate the GameVault roots (#134).
    ///
    /// The list is the source of truth. A settings file (or an older web
    /// bundle) that only knows `gamevaultRoot` is migrated into the list, then
    /// the legacy field is cleared so it is never written back. Roots are not
    /// canonicalized here: a drive can be unplugged, and the scan reports that
    /// per root instead of dropping the entry.
    fn normalize_gamevault_roots(&mut self) {
        if let Some(legacy) = self
            .gamevault_root
            .take()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            self.gamevault_roots.push(legacy);
        }
        let mut seen = std::collections::HashSet::new();
        self.gamevault_roots.retain_mut(|raw| {
            *raw = raw.trim().to_string();
            if raw.is_empty() {
                return false;
            }
            seen.insert(gamevault_root_key(raw))
        });
        self.gamevault_scan_depth = if self.gamevault_scan_depth == 0 {
            default_gamevault_scan_depth()
        } else {
            self.gamevault_scan_depth.clamp(4, 8)
        };
    }

    /// Configured GameVault roots as paths (#134).
    pub fn gamevault_root_paths(&self) -> Vec<PathBuf> {
        self.gamevault_roots
            .iter()
            .map(|root| PathBuf::from(root.trim()))
            .filter(|path| !path.as_os_str().is_empty())
            .collect()
    }

    /// Effective executable search depth for folder scans (#134).
    pub fn gamevault_depth(&self) -> usize {
        usize::from(self.gamevault_scan_depth).clamp(4, 8)
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

    /// Folder a new install stages its temporary archive in (#226).
    ///
    /// `installVolume` always answers `{install_root}/.ludex-tmp`, so the
    /// extraction reads the archive locally even when the configured download
    /// folder is a network share. `configured` keeps today's resolution: the
    /// default download location, then `downloadDir`, then the same install
    /// volume folder.
    ///
    /// This is only consulted when a job has no pinned download folder and no
    /// stored `.part`; a resumable checkpoint is never relocated.
    pub fn effective_download_dir(&self, install_root: &Path) -> PathBuf {
        match self.download_staging {
            DownloadStaging::InstallVolume => install_root.join(".ludex-tmp"),
            DownloadStaging::Configured => self.configured_download_dir(install_root),
        }
    }

    /// Today's download folder precedence, kept apart so `resolve_download_dir`
    /// stays the single source for callers that do not know about #226.
    fn configured_download_dir(&self, install_root: &Path) -> PathBuf {
        if let Some(location) = self.default_location(LocationKind::Download) {
            let path = location.path.trim();
            if !path.is_empty() {
                return PathBuf::from(path);
            }
        }
        resolve_download_dir(self, install_root, None)
    }
}

/// Stable comparison key for a GameVault root: separators normalised and
/// case-insensitive on Windows, where paths are.
fn gamevault_root_key(raw: &str) -> String {
    let normalised = raw.trim().replace('\\', "/");
    let trimmed = normalised.trim_end_matches('/');
    let value = if trimmed.is_empty() {
        normalised.as_str()
    } else {
        trimmed
    };
    if cfg!(windows) {
        value.to_ascii_lowercase()
    } else {
        value.to_string()
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
    if !settings.ludusavi_manifest_url.is_empty() {
        let lower = settings.ludusavi_manifest_url.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return Err(format!(
                "The manifest URL must start with http:// or https:// (got {}).",
                settings.ludusavi_manifest_url
            ));
        }
        if settings
            .ludusavi_manifest_url
            .chars()
            .any(char::is_whitespace)
        {
            return Err("The manifest URL must not contain whitespace.".to_string());
        }
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
/// the extracted game. The `downloadStaging` policy is applied by
/// [`LocalSettings::effective_download_dir`] (#226); callers that resolve a
/// *new* job's folder should use that instead.
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
        assert_eq!(json["downloadStaging"], "configured");
        assert_eq!(json["keepArchives"], false);
        assert_eq!(json["bandwidthLimitKib"], 0);
        assert_eq!(json["closeToTray"], true);
        assert_eq!(json["nativeTitleBar"], false);
        assert_eq!(json["updateChannel"], "stable");
        assert_eq!(json["checkUpdatesOnStartup"], true);
        assert_eq!(json["lastUpdateCheckAt"], serde_json::Value::Null);
        assert_eq!(json["skippedUpdateVersion"], serde_json::Value::Null);
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
        assert_eq!(json["ludusaviManifestAutoUpdate"], true);
        assert_eq!(json["ludusaviManifestUrl"], "");
        assert_eq!(json["winePrefix"], "");
        assert_eq!(json["cloudAutoSync"], true);
        assert_eq!(json["cloudDeviceId"], "");
        assert_eq!(json["cloudDeviceLabel"], serde_json::Value::Null);
        assert_eq!(json["servers"], serde_json::json!([]));
        assert_eq!(json["activeServerId"], "");
        assert_eq!(json["sunshineAppsPath"], "");
        assert_eq!(json["sunshineConfPath"], "");
        assert_eq!(json["moonlightBinary"], "");
        assert_eq!(json["moonlightConfigPath"], "");
        assert_eq!(json["moonlightHost"], "");
        assert_eq!(json["consoleMode"], false);
        assert_eq!(json["consoleAppName"], "Ludex");
        assert_eq!(json["consoleFullscreen"], true);
        assert_eq!(json["consoleExitShortcut"], "f12");
        assert_eq!(json["consolePerGameApps"], false);
        // #145: old settings files and fresh installs are both unconfirmed.
        assert_eq!(json["installRootConfirmed"], false);
    }

    #[test]
    fn console_mode_settings_round_trip_and_normalize() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.console_mode = true;
        settings.console_app_name = "  Saloon  ".into();
        settings.console_fullscreen = false;
        settings.console_exit_shortcut = " Ctrl+Shift+Q ".into();
        settings.console_per_game_apps = true;
        settings.normalize(Path::new("/data"));

        assert!(settings.console_mode);
        assert_eq!(settings.console_app_name, "Saloon");
        assert!(!settings.console_fullscreen);
        assert_eq!(settings.console_exit_shortcut, "ctrl+shift+q");
        assert!(settings.console_per_game_apps);

        let json = serde_json::to_value(&settings).unwrap();
        let back: LocalSettings = serde_json::from_value(json).unwrap();
        assert_eq!(back.console_mode, settings.console_mode);
        assert_eq!(back.console_app_name, settings.console_app_name);
        assert_eq!(back.console_fullscreen, settings.console_fullscreen);
        assert_eq!(back.console_exit_shortcut, settings.console_exit_shortcut);
        assert_eq!(back.console_per_game_apps, settings.console_per_game_apps);

        // A blank name and an empty shortcut fall back to the defaults, and a
        // settings file written before #172 has no keys at all.
        let mut blank = LocalSettings::for_app_data(Path::new("/data"));
        blank.console_app_name = "   ".into();
        blank.console_exit_shortcut = String::new();
        blank.normalize(Path::new("/data"));
        assert_eq!(blank.console_app_name, "Ludex");
        assert_eq!(blank.console_exit_shortcut, "f12");

        let legacy: LocalSettings = serde_json::from_value(serde_json::json!({
            "installDir": "D:\\Games",
        }))
        .unwrap();
        assert!(!legacy.console_mode);
        assert!(legacy.console_fullscreen);
        assert!(!legacy.console_per_game_apps);
    }

    #[test]
    fn install_root_confirmation_roundtrips_without_touching_paths() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        assert!(!settings.install_root_confirmed);

        settings.install_root_confirmed = true;
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["installRootConfirmed"], true);
        let back: LocalSettings = serde_json::from_value(json).unwrap();
        assert!(back.install_root_confirmed);
        // Still the same default folder: confirming never moves an install.
        assert_eq!(back.install_dir, settings.install_dir);

        // A settings file written before #145 has no key at all.
        let legacy: LocalSettings = serde_json::from_value(serde_json::json!({
            "installDir": "D:\\Games",
        }))
        .unwrap();
        assert!(!legacy.install_root_confirmed);
        assert_eq!(legacy.install_dir, "D:\\Games");
    }

    #[test]
    fn server_profiles_are_normalized_and_deduplicated() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.active_server_id = " srv_1 ".into();
        settings.servers = vec![
            ServerProfile {
                id: "srv_1".into(),
                label: "  Home  ".into(),
                url: " https://home.example.com/ ".into(),
                username: Some("  cleeryy ".into()),
                last_used_at: None,
                created_at: None,
            },
            ServerProfile {
                id: "srv_1".into(),
                label: "Duplicate".into(),
                url: "https://other.example.com".into(),
                username: None,
                last_used_at: None,
                created_at: None,
            },
            ServerProfile {
                id: "  ".into(),
                label: "No id".into(),
                url: "https://skip.example.com".into(),
                username: None,
                last_used_at: None,
                created_at: None,
            },
        ];
        settings.sunshine_apps_path = " /opt/sunshine/apps.json ".into();
        settings.moonlight_binary = " /usr/bin/moonlight ".into();
        settings.normalize(Path::new("/data"));

        assert_eq!(settings.servers.len(), 1);
        assert_eq!(settings.servers[0].id, "srv_1");
        assert_eq!(settings.servers[0].label, "Home");
        assert_eq!(settings.servers[0].url, "https://home.example.com");
        assert_eq!(settings.servers[0].username.as_deref(), Some("cleeryy"));
        assert_eq!(settings.active_server_id, "srv_1");
        assert_eq!(settings.sunshine_apps_path, "/opt/sunshine/apps.json");
        assert_eq!(settings.moonlight_binary, "/usr/bin/moonlight");

        settings.active_server_id = "srv_missing".into();
        settings.normalize(Path::new("/data"));
        assert!(settings.active_server_id.is_empty());

        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["servers"].as_array().unwrap().len(), 1);
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
    fn device_registry_identity_is_local_and_roundtrips() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.device_id = " abc-123 ".into();
        settings.device_name = Some("  Gaming PC  ".into());
        settings.normalize(Path::new("/data"));
        assert_eq!(settings.device_id, "abc-123");
        assert_eq!(settings.device_name.as_deref(), Some("Gaming PC"));

        // The id is generated on first heartbeat, never invented by a save from
        // the web UI: normalize only sanitises what is there.
        settings.device_id.clear();
        settings.normalize(Path::new("/data"));
        assert!(settings.device_id.is_empty());

        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["deviceId"], "");
        assert_eq!(json["deviceName"], "Gaming PC");

        let restored: LocalSettings = serde_json::from_value(json).unwrap();
        assert_eq!(restored.device_name.as_deref(), Some("Gaming PC"));
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
    fn skipped_update_version_roundtrips_and_clears() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        assert!(settings.skipped_update_version.is_none());

        settings.skipped_update_version = Some(" 0.3.2-edge.179 ".into());
        settings.normalize(Path::new("/data"));
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["skippedUpdateVersion"], "0.3.2-edge.179");
        let back: LocalSettings = serde_json::from_value(json).unwrap();
        assert_eq!(
            back.skipped_update_version.as_deref(),
            Some("0.3.2-edge.179")
        );

        // A blank value is not a version: drop it instead of suppressing
        // prompts forever with an empty string.
        let mut blank = LocalSettings {
            skipped_update_version: Some("   ".into()),
            ..LocalSettings::default()
        };
        blank.normalize(Path::new("/data"));
        assert!(blank.skipped_update_version.is_none());

        // Settings files written before #163 have no key at all.
        let legacy: LocalSettings =
            serde_json::from_value(serde_json::json!({ "updateChannel": "stable" })).unwrap();
        assert!(legacy.skipped_update_version.is_none());
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
    fn ui_language_mirrors_the_account_locale_for_game_sync() {
        // #188: the web writes `uiLanguage` from the account preference so a
        // game launch does not use a different language than the interface.
        let mut settings = LocalSettings {
            ui_language: "  PT-BR  ".into(),
            ..LocalSettings::default()
        };
        settings.normalize(Path::new("/tmp/ludex-test"));
        assert_eq!(settings.ui_language, "pt-br");

        let json = serde_json::to_value(&settings).expect("serialize");
        assert_eq!(json["uiLanguage"], "pt-br");
        // The value survives a reload, which is what the offline cache needs.
        let parsed: LocalSettings = serde_json::from_value(json).expect("deserialize");
        assert_eq!(parsed.ui_language, "pt-br");
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
    fn download_staging_defaults_to_configured() {
        let settings = LocalSettings::for_app_data(Path::new("/data"));
        assert_eq!(settings.download_staging, DownloadStaging::Configured);
        let json = serde_json::to_value(&settings).expect("serialize");
        assert_eq!(json["downloadStaging"], "configured");

        // A settings file written before #226 has no key at all.
        let legacy: LocalSettings =
            serde_json::from_value(serde_json::json!({ "installDir": "D:\\Games" }))
                .expect("legacy settings");
        assert_eq!(legacy.download_staging, DownloadStaging::Configured);
        assert_eq!(legacy.install_dir, "D:\\Games");
    }

    #[test]
    fn download_staging_accepts_install_volume_and_falls_back() {
        let parsed: LocalSettings =
            serde_json::from_value(serde_json::json!({ "downloadStaging": "installVolume" }))
                .expect("installVolume");
        assert_eq!(parsed.download_staging, DownloadStaging::InstallVolume);
        assert_eq!(
            serde_json::to_value(parsed).expect("serialize")["downloadStaging"],
            "installVolume"
        );

        // Unknown values, the wrong case and `null` all keep today's staging
        // instead of failing the whole settings document.
        for raw in [
            serde_json::json!("install-volume"),
            serde_json::json!("futureMode"),
            serde_json::json!(null),
        ] {
            let parsed: LocalSettings =
                serde_json::from_value(serde_json::json!({ "downloadStaging": raw }))
                    .expect("unknown staging falls back");
            assert_eq!(parsed.download_staging, DownloadStaging::Configured);
        }
        assert_eq!(
            DownloadStaging::parse("  INSTALLVOLUME "),
            DownloadStaging::InstallVolume
        );
        assert_eq!(
            DownloadStaging::parse("configured"),
            DownloadStaging::Configured
        );
    }

    #[test]
    fn effective_download_dir_follows_the_staging_policy() {
        let install_root = Path::new("/games");
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        // The default: whatever the configured download folder resolves to.
        assert_eq!(
            settings.effective_download_dir(install_root),
            PathBuf::from("/games/.ludex-tmp")
        );
        settings.download_dir = "/mnt/nas/Downloads".into();
        assert_eq!(
            settings.effective_download_dir(install_root),
            PathBuf::from("/mnt/nas/Downloads")
        );

        // `installVolume` stages next to the install, whatever is configured.
        settings.download_staging = DownloadStaging::InstallVolume;
        assert_eq!(
            settings.effective_download_dir(install_root),
            PathBuf::from("/games/.ludex-tmp")
        );

        // #226: the free function stays the historical, policy-free resolution.
        assert_eq!(
            resolve_download_dir(&settings, install_root, None),
            PathBuf::from("/mnt/nas/Downloads")
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
    fn manifest_settings_roundtrip() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        assert!(settings.ludusavi_manifest_auto_update);
        settings.ludusavi_manifest_auto_update = false;
        settings.ludusavi_manifest_url = " https://mirror.example/ludusavi.yaml/ ".into();
        settings.normalize(Path::new("/data"));

        assert!(!settings.ludusavi_manifest_auto_update);
        assert_eq!(
            settings.ludusavi_manifest_url,
            "https://mirror.example/ludusavi.yaml"
        );

        let json = serde_json::to_value(&settings).unwrap();
        let back: LocalSettings = serde_json::from_value(json).unwrap();
        assert!(!back.ludusavi_manifest_auto_update);
        assert_eq!(
            back.ludusavi_manifest_url,
            "https://mirror.example/ludusavi.yaml"
        );

        // A settings file written before #153 has neither key.
        let legacy: LocalSettings =
            serde_json::from_value(serde_json::json!({ "mode": "local" })).unwrap();
        assert!(legacy.ludusavi_manifest_auto_update);
        assert!(legacy.ludusavi_manifest_url.is_empty());

        // The mirror must be an http(s) URL.
        let invalid = LocalSettings {
            ludusavi_manifest_url: "file:///tmp/manifest.yaml".into(),
            ..LocalSettings::default()
        };
        assert!(validate(&invalid).is_err());
        let valid = LocalSettings {
            ludusavi_manifest_url: "https://mirror.example/manifest.yaml".into(),
            ..LocalSettings::default()
        };
        assert!(validate(&valid).is_ok());
    }

    #[test]
    fn partial_remote_settings_are_allowed() {
        let settings = LocalSettings {
            mode: Mode::Remote,
            ..LocalSettings::default()
        };
        assert!(validate(&settings).is_ok());
    }

    /* -------------------- launcher sources (#155) -------------------------- */

    #[test]
    fn launcher_sources_default_to_every_source() {
        let settings = LocalSettings::for_app_data(Path::new("/data"));
        assert_eq!(
            settings.launcher_sources,
            vec!["steam", "epic", "gog", "ea", "ubisoft", "xbox", "itch", "heroic", "lutris"]
        );

        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["launcherSources"][0], "steam");
        assert_eq!(json["launcherSources"].as_array().unwrap().len(), 9);

        // A settings file written before #155 has no key: additive migration,
        // nothing to rewrite and every source stays on.
        let legacy: LocalSettings =
            serde_json::from_value(serde_json::json!({ "installDir": "D:\\Games" })).unwrap();
        assert_eq!(legacy.launcher_sources.len(), 9);
        assert!(legacy.launcher_sources.contains(&"lutris".to_string()));
    }

    #[test]
    fn launcher_sources_are_normalized_and_roundtrip() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.launcher_sources = vec![
            " Lutris ".into(),
            "epic".into(),
            "epic".into(),
            "uplay".into(),
            "nope".into(),
        ];
        settings.normalize(Path::new("/data"));
        assert_eq!(
            settings.launcher_sources,
            vec!["epic", "ubisoft", "lutris"],
            "unknown ids dropped, duplicates and order canonicalized"
        );

        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(
            json["launcherSources"],
            serde_json::json!(["epic", "ubisoft", "lutris"])
        );
        let back: LocalSettings = serde_json::from_value(json).unwrap();
        assert_eq!(back.launcher_sources, settings.launcher_sources);

        // "None" is a real choice and must survive a save; it never falls back
        // to the default list.
        let mut none = LocalSettings::for_app_data(Path::new("/data"));
        none.launcher_sources = Vec::new();
        none.normalize(Path::new("/data"));
        assert!(none.launcher_sources.is_empty());
        let json = serde_json::to_value(&none).unwrap();
        assert_eq!(json["launcherSources"], serde_json::json!([]));
        let back: LocalSettings = serde_json::from_value(json).unwrap();
        assert!(back.launcher_sources.is_empty());
    }

    #[test]
    fn disabling_a_launcher_source_keeps_the_other_settings() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.install_dir = "/games".into();
        settings.steam_root = "/steam".into();
        settings.launcher_sources = vec!["steam".into()];
        settings.normalize(Path::new("/data"));

        // Only the source list changes: nothing else is reset or migrated.
        assert_eq!(settings.install_dir, "/games");
        assert_eq!(settings.steam_root, "/steam");
        assert_eq!(settings.launcher_sources, vec!["steam"]);
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

    #[test]
    fn legacy_single_gamevault_root_migrates_to_the_list() {
        // An older settings file or web bundle only knows `gamevaultRoot`.
        let raw = serde_json::json!({
            "installDir": "/games",
            "gamevaultRoot": "  D:\\Games\\GameVault  ",
        });
        let mut settings: LocalSettings = serde_json::from_value(raw).unwrap();
        settings.normalize(Path::new("/data"));

        assert_eq!(settings.gamevault_roots, vec!["D:\\Games\\GameVault"]);
        assert!(settings.gamevault_root.is_none());
        // The legacy field is not written back.
        let json = serde_json::to_value(&settings).unwrap();
        assert!(json.get("gamevaultRoot").is_none());
        assert_eq!(
            json["gamevaultRoots"],
            serde_json::json!(["D:\\Games\\GameVault"])
        );
        assert_eq!(json["gamevaultScanDepth"], 6);
    }

    #[test]
    fn gamevault_roots_are_trimmed_deduplicated_and_depth_is_clamped() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.gamevault_roots = vec![
            "  C:\\Games\\GameVault  ".into(),
            "C:\\Games\\GameVault".into(),
            "   ".into(),
            "/mnt/gamevault".into(),
        ];
        settings.gamevault_scan_depth = 0;
        settings.normalize(Path::new("/data"));

        assert_eq!(settings.gamevault_roots.len(), 2);
        assert_eq!(settings.gamevault_roots[0], "C:\\Games\\GameVault");
        assert_eq!(settings.gamevault_roots[1], "/mnt/gamevault");
        // 0 means "not configured" and falls back to the documented default.
        assert_eq!(settings.gamevault_depth(), 6);

        settings.gamevault_scan_depth = 2;
        settings.normalize(Path::new("/data"));
        assert_eq!(settings.gamevault_depth(), 4);

        settings.gamevault_scan_depth = 42;
        settings.normalize(Path::new("/data"));
        assert_eq!(settings.gamevault_depth(), 8);
    }

    #[test]
    fn overlay_settings_default_and_roundtrip() {
        let dir = Path::new("/tmp/ludex-overlay");
        let mut settings = LocalSettings::for_app_data(dir);
        settings.normalize(dir);

        assert!(settings.overlay_enabled);
        assert_eq!(settings.overlay_trigger, OverlayTrigger::DoubleHome);
        assert!((settings.overlay_opacity - 0.92).abs() < f64::EPSILON);
        assert!((settings.overlay_scale - 1.0).abs() < f64::EPSILON);
        assert_eq!(
            settings.overlay_tabs,
            vec!["game", "library", "downloads", "achievements"]
        );
        assert!(settings.overlay_disabled_games.is_empty());

        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["overlayEnabled"], true);
        assert_eq!(json["overlayTrigger"], "doubleHome");
        assert_eq!(json["overlayOpacity"], 0.92);
        assert_eq!(json["overlayScale"], 1.0);
        assert_eq!(
            json["overlayTabs"],
            serde_json::json!(["game", "library", "downloads", "achievements"])
        );
        assert_eq!(json["overlayDisabledGames"], serde_json::json!([]));

        let restored: LocalSettings = serde_json::from_value(json).unwrap();
        assert!(restored.overlay_enabled);
        assert_eq!(restored.overlay_trigger, OverlayTrigger::DoubleHome);
        assert_eq!(restored.overlay_tabs, settings.overlay_tabs);
    }

    #[test]
    fn overlay_settings_are_normalized_and_survive_a_partial_file() {
        // A settings file written before #144 has no overlay key at all.
        let raw = serde_json::json!({ "installDir": "/games", "streamInstall": false });
        let mut settings: LocalSettings = serde_json::from_value(raw).unwrap();
        assert!(settings.overlay_enabled);
        assert_eq!(settings.overlay_trigger, OverlayTrigger::DoubleHome);
        assert_eq!(settings.overlay_tabs, default_overlay_tabs());

        settings.overlay_enabled = false;
        settings.overlay_trigger = OverlayTrigger::Chord;
        settings.overlay_opacity = 4.5;
        settings.overlay_scale = 0.1;
        settings.overlay_tabs = vec![
            "downloads".into(),
            "unknown".into(),
            " GAME ".into(),
            "Downloads".into(),
        ];
        settings.overlay_disabled_games = vec![42, -1, 42, 0, 7];
        settings.normalize(Path::new("/data"));

        assert!(!settings.overlay_enabled);
        assert_eq!(settings.overlay_trigger, OverlayTrigger::Chord);
        assert!((settings.overlay_opacity - 1.0).abs() < f64::EPSILON);
        assert!((settings.overlay_scale - 0.6).abs() < f64::EPSILON);
        // Canonical order, deduplicated, unknown ids dropped.
        assert_eq!(settings.overlay_tabs, vec!["game", "downloads"]);
        assert_eq!(settings.overlay_disabled_games, vec![7, 42]);

        settings.overlay_opacity = f64::NAN;
        settings.overlay_scale = f64::INFINITY;
        settings.normalize(Path::new("/data"));
        assert!((settings.overlay_opacity - 0.92).abs() < f64::EPSILON);
        assert!((settings.overlay_scale - 1.0).abs() < f64::EPSILON);

        // An empty tab list is meaningful: the game tab only, never "all".
        settings.overlay_tabs = Vec::new();
        settings.normalize(Path::new("/data"));
        assert!(settings.overlay_tabs.is_empty());
    }

    #[test]
    fn download_concurrency_defaults_to_one_and_is_bounded() {
        let mut settings = LocalSettings::default();
        assert_eq!(settings.download_concurrency, 1);

        // A settings file written before #221 has no key: the default applies.
        let raw = serde_json::json!({ "installDir": "/games" });
        let mut legacy: LocalSettings = serde_json::from_value(raw).unwrap();
        assert_eq!(legacy.download_concurrency, 1);

        // Two is the documented maximum: extraction is always serialised.
        settings.download_concurrency = 2;
        settings.normalize(Path::new("/data"));
        assert_eq!(settings.download_concurrency, 2);
        settings.download_concurrency = 9;
        settings.normalize(Path::new("/data"));
        assert_eq!(
            settings.download_concurrency,
            u64::from(crate::state::MAX_CONCURRENT_DOWNLOADS as u32)
        );
        // Zero is not "unlimited": it falls back to the single-download default.
        settings.download_concurrency = 0;
        settings.normalize(Path::new("/data"));
        assert_eq!(settings.download_concurrency, 1);
        assert_eq!(legacy.download_concurrency, 1);
    }

    #[test]
    fn download_retry_defaults_and_bounds() {
        let mut settings = LocalSettings::default();
        assert!(settings.auto_resume_downloads);
        assert_eq!(settings.download_retry_window_secs, 6 * 60 * 60);

        // A settings file from before #212 has neither key: both defaults apply.
        let raw = serde_json::json!({ "installDir": "/games" });
        let mut legacy: LocalSettings = serde_json::from_value(raw).unwrap();
        assert!(legacy.auto_resume_downloads);
        assert_eq!(legacy.download_retry_window_secs, 6 * 60 * 60);

        // 0 means "keep retrying while the app is open" and is preserved.
        settings.download_retry_window_secs = 0;
        settings.normalize(Path::new("/data"));
        assert_eq!(settings.download_retry_window_secs, 0);

        settings.download_retry_window_secs = 3;
        settings.normalize(Path::new("/data"));
        assert_eq!(settings.download_retry_window_secs, 60);

        settings.download_retry_window_secs = 30 * 24 * 60 * 60;
        settings.normalize(Path::new("/data"));
        assert_eq!(settings.download_retry_window_secs, 7 * 24 * 60 * 60);

        // The opt-out survives normalization.
        legacy.auto_resume_downloads = false;
        legacy.normalize(Path::new("/data"));
        assert!(!legacy.auto_resume_downloads);
    }

    #[test]
    fn overlay_trigger_values_roundtrip_as_camel_case() {
        for (trigger, expected) in [
            (OverlayTrigger::DoubleHome, "doubleHome"),
            (OverlayTrigger::Chord, "chord"),
            (OverlayTrigger::Keyboard, "keyboard"),
        ] {
            let json = serde_json::to_value(trigger).unwrap();
            assert_eq!(json, expected);
            let restored: OverlayTrigger = serde_json::from_value(json).unwrap();
            assert_eq!(restored, trigger);
        }
        assert!(serde_json::from_value::<OverlayTrigger>(serde_json::json!("nope")).is_err());
    }
}
