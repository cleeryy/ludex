//! Drive-based install root suggestions (#145).
//!
//! A fresh Windows install used to propose only `{appDataDir}/Games`, which
//! sits on the system drive and is usually hidden. This module proposes
//! `<drive>:\Games` for every fixed drive (plus `~/Games` and the common
//! `/mnt/...`, `/media/...`, `/Volumes/...` mounts elsewhere), reports free
//! space and writability, and marks one suggestion as the default.
//!
//! Nothing here creates a directory or changes a setting. Discovery and space
//! probing sit behind traits so the tests can describe drives that do not
//! exist on the build machine.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::settings::LocalSettings;

/// Below this much free space a suggestion is flagged as nearly full. A 2026
/// game plus its archive routinely needs tens of gigabytes.
pub const NEARLY_FULL_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// Free space below this share of the volume is also "nearly full".
const NEARLY_FULL_PERCENT: u64 = 10;

/// What kind of volume a mount is. Only fixed volumes are proposed on Windows
/// by default; network and removable drives stay out unless the user adds them
/// by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VolumeKind {
    Fixed,
    Removable,
    #[cfg_attr(not(windows), allow(dead_code))] // only built by the Windows probe
    Network,
    Other,
}

/// One mounted volume as reported by the platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// Volume root: `C:\` on Windows, `/mnt/games` on Unix.
    pub root: PathBuf,
    pub kind: VolumeKind,
    /// Volume label when the platform reports one.
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    MacOs,
    Linux,
}

/// The raw platform picture the pure suggestion code works from.
#[derive(Debug, Clone)]
pub struct Discovery {
    pub platform: Platform,
    pub home: Option<PathBuf>,
    pub mounts: Vec<Mount>,
}

/// Reads the volumes of the running machine.
pub trait MountReader {
    fn discovery(&self) -> Discovery;
}

/// Free/total/writability probe. Kept separate from [`MountReader`] so a test
/// can describe an empty `C:\Games` with 500 GB free without a C: drive.
pub trait SpaceProbe {
    fn free_bytes(&self, path: &Path) -> Option<u64>;
    fn total_bytes(&self, path: &Path) -> Option<u64>;
    /// True when a file can be created in the folder, or its nearest existing
    /// parent. Must not leave anything behind.
    fn writable(&self, path: &Path) -> bool;
}

/// A proposed install root.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageSuggestion {
    /// Candidate install root. Never created by this module.
    pub path: String,
    /// Short volume name: `C:`, `Home`, `games`.
    pub label: String,
    /// Volume root the candidate lives on (`C:\`, `/mnt/games`).
    pub volume: String,
    pub kind: VolumeKind,
    /// The folder already exists (never created here).
    pub exists: bool,
    pub writable: bool,
    pub free_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    /// Suggested default for a new install.
    pub recommended: bool,
    /// Matches the current default install location in settings.
    pub current: bool,
    /// `{appDataDir}/Games`, used when no drive is suitable.
    pub fallback: bool,
    /// Little free space left; warn before adding it.
    pub nearly_full: bool,
    /// Read-only parent, unplugged share, missing data folder…
    pub note: Option<String>,
}

/// Response of `suggest_storage_locations`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageSuggestions {
    pub suggestions: Vec<StorageSuggestion>,
    /// Path of the recommended suggestion, when one is usable.
    pub recommended_path: Option<String>,
    /// Current default install root, mirroring `installDir`.
    pub current_path: String,
    /// The current root is the app-data fallback, not a folder the user picked.
    pub current_is_fallback: bool,
    /// The user never confirmed an install root, so the install dialog may
    /// preselect `recommendedPath` without touching the saved settings.
    pub needs_choice: bool,
    pub platform: Platform,
}

struct Candidate {
    path: PathBuf,
    label: String,
    volume: String,
    kind: VolumeKind,
    fallback: bool,
}

fn path_key(path: &Path) -> String {
    let normalised = path.to_string_lossy().replace('\\', "/");
    let trimmed = normalised.trim_end_matches('/').to_string();
    let value = if trimmed.is_empty() {
        normalised
    } else {
        trimmed
    };
    if cfg!(windows) {
        value.to_ascii_lowercase()
    } else {
        value
    }
}

fn volume_display(root: &Path) -> String {
    // `C:\` -> `C:` on Windows.
    if let Some(Component::Prefix(prefix)) = root.components().next() {
        return prefix
            .as_os_str()
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_string();
    }
    let raw = root.to_string_lossy();
    let trimmed = raw.trim_end_matches(['\\', '/']);
    let name = trimmed.split(['\\', '/']).next_back().unwrap_or("");
    if name.is_empty() {
        trimmed.to_string()
    } else {
        name.to_string()
    }
}

fn games(root: &Path) -> PathBuf {
    let raw = root.to_string_lossy();
    // Tests describe Windows drives on any host, where `Path::join` would add a
    // Unix separator between `C:\` and `Games`.
    if raw.ends_with('\\') || raw.ends_with('/') {
        PathBuf::from(format!("{raw}Games"))
    } else if raw.contains('\\') || raw.get(1..2) == Some(":") {
        PathBuf::from(format!("{raw}\\Games"))
    } else {
        root.join("Games")
    }
}

/// A mount that is already named `games` is used as-is: `/mnt/games` should not
/// become `/mnt/games/Games`.
fn games_on_mount(root: &Path) -> PathBuf {
    let named_games = root
        .file_name()
        .map(|name| name.to_string_lossy().eq_ignore_ascii_case("games"))
        .unwrap_or(false);
    if named_games {
        root.to_path_buf()
    } else {
        games(root)
    }
}

/// Candidate roots for the reported platform, before probing.
fn candidates(discovery: &Discovery) -> Vec<Candidate> {
    let mut list = Vec::new();
    let mut push = |path: PathBuf, label: String, volume: String, kind: VolumeKind| {
        list.push(Candidate {
            path,
            label,
            volume,
            kind,
            fallback: false,
        });
    };

    match discovery.platform {
        Platform::Windows => {
            // Fixed drives only: network shares and removable media are opt-in,
            // and a disconnected `Z:` must not break a fresh install.
            for mount in discovery
                .mounts
                .iter()
                .filter(|mount| mount.kind == VolumeKind::Fixed)
            {
                let volume = volume_display(&mount.root);
                let label = mount
                    .label
                    .clone()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| volume.clone());
                push(games(&mount.root), label, volume, mount.kind);
            }
        }
        Platform::Linux | Platform::MacOs => {
            if let Some(home) = discovery.home.as_deref() {
                push(
                    games(home),
                    "Home".to_string(),
                    home.to_string_lossy().to_string(),
                    VolumeKind::Fixed,
                );
            }
            let (prefixes, keep): (&[&str], &[VolumeKind]) = match discovery.platform {
                Platform::MacOs => (&["/Volumes"], &[VolumeKind::Fixed, VolumeKind::Other]),
                _ => (
                    &["/mnt", "/media"],
                    &[VolumeKind::Fixed, VolumeKind::Removable],
                ),
            };
            for mount in &discovery.mounts {
                if !keep.contains(&mount.kind) {
                    continue;
                }
                if !prefixes.iter().any(|prefix| mount.root.starts_with(prefix)) {
                    continue;
                }
                let volume = volume_display(&mount.root);
                let label = mount
                    .label
                    .clone()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| volume.clone());
                push(
                    games_on_mount(&mount.root),
                    label,
                    mount.root.to_string_lossy().to_string(),
                    mount.kind,
                );
            }
        }
    }

    list
}

fn nearly_full(free: u64, total: Option<u64>) -> bool {
    if free < NEARLY_FULL_BYTES {
        return true;
    }
    match total {
        Some(total) if total > 0 => free.saturating_mul(100) / total < NEARLY_FULL_PERCENT,
        _ => false,
    }
}

/// Build the suggestion list from an injected machine description.
///
/// Pure apart from `path.is_dir()` and stat calls inside `probe`: no folder is
/// created, no setting is read back into the caller.
pub fn suggest_with(
    reader: &dyn MountReader,
    probe: &dyn SpaceProbe,
    settings: &LocalSettings,
    app_data: &Path,
) -> StorageSuggestions {
    let discovery = reader.discovery();
    suggest_from(&discovery, probe, settings, app_data)
}

fn suggest_from(
    discovery: &Discovery,
    probe: &dyn SpaceProbe,
    settings: &LocalSettings,
    app_data: &Path,
) -> StorageSuggestions {
    let current = settings.install_path();
    let fallback = games(app_data);
    let current_key = path_key(&current);
    let fallback_key = path_key(&fallback);
    let current_is_fallback = current_key == fallback_key;
    let current_text = current.to_string_lossy().to_string();

    let mut ordered: Vec<Candidate> = Vec::new();
    // The current choice comes first so it survives deduplication and keeps its
    // `current` flag even when it is also a discovered drive root.
    if !current.as_os_str().is_empty() {
        ordered.push(Candidate {
            label: volume_display(&current),
            volume: current
                .parent()
                .map(|parent| parent.to_string_lossy().to_string())
                .unwrap_or_else(|| current_text.clone()),
            path: current.clone(),
            kind: VolumeKind::Fixed,
            fallback: current_is_fallback,
        });
    }
    ordered.extend(candidates(discovery));
    ordered.push(Candidate {
        path: fallback.clone(),
        label: String::new(),
        volume: app_data.to_string_lossy().to_string(),
        kind: VolumeKind::Fixed,
        fallback: true,
    });

    let mut seen = HashSet::new();
    let mut suggestions = Vec::new();
    for candidate in ordered {
        let key = path_key(&candidate.path);
        if !seen.insert(key.clone()) {
            // A duplicate current/fallback entry only contributes its flags.
            if let Some(existing) = suggestions
                .iter_mut()
                .find(|item: &&mut StorageSuggestion| path_key(Path::new(&item.path)) == key)
            {
                existing.current |= key == current_key;
                existing.fallback |= candidate.fallback;
            }
            continue;
        }
        let path = candidate.path;
        let exists = path.is_dir();
        let writable = probe.writable(&path);
        let free_bytes = probe.free_bytes(&path);
        let total_bytes = probe.total_bytes(&path);
        let nearly_full = free_bytes
            .map(|free| nearly_full(free, total_bytes))
            .unwrap_or(false);
        let note = if !writable {
            Some(if exists {
                "This folder is read-only for your user.".to_string()
            } else {
                "Ludex cannot create files here; its parent folder is not writable.".to_string()
            })
        } else if !exists {
            Some("The folder will be created when an install uses it.".to_string())
        } else {
            None
        };
        let fallback_entry = candidate.fallback || key == fallback_key;
        suggestions.push(StorageSuggestion {
            path: path.to_string_lossy().to_string(),
            label: if candidate.label.is_empty() {
                "App data".to_string()
            } else {
                candidate.label
            },
            volume: candidate.volume,
            kind: candidate.kind,
            exists,
            writable,
            free_bytes,
            total_bytes,
            recommended: false,
            current: key == current_key,
            fallback: fallback_entry,
            nearly_full,
            note,
        });
    }

    // Recommendation: an explicit current choice wins; otherwise the roomiest
    // writable drive; otherwise the app-data fallback.
    let recommended_key = if !current_is_fallback {
        suggestions
            .iter()
            .find(|item| item.current && item.writable)
            .map(|item| path_key(Path::new(&item.path)))
    } else {
        None
    }
    .or_else(|| {
        let mut best: Option<&StorageSuggestion> = None;
        for item in suggestions
            .iter()
            .filter(|item| !item.fallback && item.writable && !item.nearly_full)
        {
            let room = item.free_bytes.unwrap_or(0);
            let best_room = best.and_then(|current| current.free_bytes).unwrap_or(0);
            if best.is_none() || room > best_room {
                best = Some(item);
            }
        }
        best.map(|item| path_key(Path::new(&item.path)))
    })
    .or_else(|| {
        suggestions
            .iter()
            .filter(|item| !item.fallback && item.writable)
            .max_by_key(|item| item.free_bytes.unwrap_or(0))
            .map(|item| path_key(Path::new(&item.path)))
    })
    .or_else(|| {
        suggestions
            .iter()
            .find(|item| item.fallback && item.writable)
            .map(|item| path_key(Path::new(&item.path)))
    });

    if let Some(recommended_key) = recommended_key.as_deref() {
        if let Some(item) = suggestions
            .iter_mut()
            .find(|item| path_key(Path::new(&item.path)) == recommended_key)
        {
            item.recommended = true;
        }
    }

    let recommended_path = suggestions
        .iter()
        .find(|item| item.recommended)
        .map(|item| item.path.clone());

    StorageSuggestions {
        suggestions,
        recommended_path,
        current_path: current_text,
        current_is_fallback,
        needs_choice: !settings.install_root_confirmed,
        platform: discovery.platform,
    }
}

/// Suggest install roots for the running machine. Never creates anything.
pub fn suggest(settings: &LocalSettings, app_data: &Path) -> StorageSuggestions {
    suggest_with(&NativeMountReader, &NativeSpaceProbe, settings, app_data)
}

/* ----------------------------- native probing ---------------------------- */

pub struct NativeSpaceProbe;

impl SpaceProbe for NativeSpaceProbe {
    fn free_bytes(&self, path: &Path) -> Option<u64> {
        crate::paths::nearest_existing(path).and_then(|dir| fs2::available_space(dir).ok())
    }

    fn total_bytes(&self, path: &Path) -> Option<u64> {
        crate::paths::nearest_existing(path).and_then(|dir| fs2::total_space(dir).ok())
    }

    fn writable(&self, path: &Path) -> bool {
        crate::locations::probe_writable(path)
    }
}

pub struct NativeMountReader;

impl MountReader for NativeMountReader {
    fn discovery(&self) -> Discovery {
        Discovery {
            platform: current_platform(),
            home: std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from)
                .filter(|path| !path.as_os_str().is_empty()),
            mounts: native_mounts(),
        }
    }
}

fn current_platform() -> Platform {
    if cfg!(windows) {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        Platform::Linux
    }
}

#[cfg(windows)]
fn native_mounts() -> Vec<Mount> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLogicalDrives() -> u32;
        fn GetDriveTypeW(root_path_name: *const u16) -> u32;
    }

    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;

    let mask = unsafe { GetLogicalDrives() };
    let mut mounts = Vec::new();
    for index in 0..26u32 {
        if mask & (1 << index) == 0 {
            continue;
        }
        let letter = char::from(b'A' + index as u8);
        let root = format!("{letter}:\\");
        let wide: Vec<u16> = std::ffi::OsStr::new(&root)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let kind = match unsafe { GetDriveTypeW(wide.as_ptr()) } {
            DRIVE_REMOVABLE => VolumeKind::Removable,
            DRIVE_FIXED => VolumeKind::Fixed,
            DRIVE_REMOTE => VolumeKind::Network,
            _ => VolumeKind::Other,
        };
        mounts.push(Mount {
            root: PathBuf::from(root),
            kind,
            label: None,
        });
    }
    mounts
}

#[cfg(not(windows))]
fn native_mounts() -> Vec<Mount> {
    // (base folder, volume kind, also list its children: `/media/<user>/<vol>`)
    #[cfg(target_os = "macos")]
    let bases: &[(&str, VolumeKind, bool)] = &[("/Volumes", VolumeKind::Fixed, false)];
    #[cfg(not(target_os = "macos"))]
    let bases: &[(&str, VolumeKind, bool)] = &[
        ("/mnt", VolumeKind::Fixed, false),
        ("/media", VolumeKind::Removable, true),
    ];

    let mut mounts = Vec::new();
    for (base, kind, nested) in bases {
        let Ok(entries) = std::fs::read_dir(base) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // Symlinks are skipped: `/Volumes/Macintosh HD` points at `/`, and a
            // linked mount would duplicate the whole root.
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.is_dir() {
                continue;
            }
            let mut roots = vec![path.clone()];
            if *nested {
                if let Ok(children) = std::fs::read_dir(&path) {
                    for child in children.flatten() {
                        let child_path = child.path();
                        let is_dir = std::fs::symlink_metadata(&child_path)
                            .map(|metadata| metadata.is_dir())
                            .unwrap_or(false);
                        if is_dir {
                            roots.push(child_path);
                        }
                    }
                }
            }
            for root in roots {
                mounts.push(Mount {
                    root,
                    kind: *kind,
                    label: None,
                });
            }
        }
    }
    mounts
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct FakeReader(Discovery);

    impl MountReader for FakeReader {
        fn discovery(&self) -> Discovery {
            self.0.clone()
        }
    }

    fn suggest_for(
        discovery: &Discovery,
        probe: &dyn SpaceProbe,
        settings: &LocalSettings,
        app_data: &Path,
    ) -> StorageSuggestions {
        suggest_with(&FakeReader(discovery.clone()), probe, settings, app_data)
    }

    struct FakeProbe {
        free: HashMap<String, u64>,
        total: HashMap<String, u64>,
        read_only: HashSet<String>,
    }

    impl FakeProbe {
        fn new() -> Self {
            Self {
                free: HashMap::new(),
                total: HashMap::new(),
                read_only: HashSet::new(),
            }
        }

        fn with(mut self, path: &str, free: u64, total: u64) -> Self {
            self.free.insert(path.to_string(), free);
            self.total.insert(path.to_string(), total);
            self
        }

        fn read_only(mut self, path: &str) -> Self {
            self.read_only.insert(path.to_string());
            self
        }
    }

    impl SpaceProbe for FakeProbe {
        fn free_bytes(&self, path: &Path) -> Option<u64> {
            self.free.get(&path.to_string_lossy().to_string()).copied()
        }

        fn total_bytes(&self, path: &Path) -> Option<u64> {
            self.total.get(&path.to_string_lossy().to_string()).copied()
        }

        fn writable(&self, path: &Path) -> bool {
            !self.read_only.contains(&path.to_string_lossy().to_string())
        }
    }

    const GB: u64 = 1024 * 1024 * 1024;

    fn windows_discovery(mounts: Vec<(&str, VolumeKind)>) -> Discovery {
        Discovery {
            platform: Platform::Windows,
            home: Some(PathBuf::from("C:\\Users\\player")),
            mounts: mounts
                .into_iter()
                .map(|(root, kind)| Mount {
                    root: PathBuf::from(root),
                    kind,
                    label: None,
                })
                .collect(),
        }
    }

    fn settings_at(path: &str) -> LocalSettings {
        let mut settings =
            LocalSettings::for_app_data(Path::new("C:\\Users\\player\\AppData\\Roaming\\ludex"));
        settings.install_dir = path.to_string();
        settings
    }

    #[test]
    fn fixed_drives_become_games_roots_and_network_or_removable_are_skipped() {
        let discovery = windows_discovery(vec![
            ("C:\\", VolumeKind::Fixed),
            ("D:\\", VolumeKind::Fixed),
            ("Z:\\", VolumeKind::Network),
            ("E:\\", VolumeKind::Removable),
        ]);
        let probe = FakeProbe::new().with("C:\\Games", 100 * GB, 500 * GB).with(
            "D:\\Games",
            900 * GB,
            2000 * GB,
        );
        let suggestion = suggest_for(
            &discovery,
            &probe,
            &settings_at("C:\\Users\\player\\AppData\\Roaming\\ludex\\Games"),
            Path::new("C:\\Users\\player\\AppData\\Roaming\\ludex"),
        );

        let paths: Vec<&str> = suggestion
            .suggestions
            .iter()
            .map(|item| item.path.as_str())
            .collect();
        assert!(paths.contains(&"C:\\Games"));
        assert!(paths.contains(&"D:\\Games"));
        assert!(!paths.iter().any(|path| path.starts_with("Z:")));
        assert!(!paths.iter().any(|path| path.starts_with("E:")));
        // The roomiest fixed drive is recommended for a user who never chose.
        assert_eq!(suggestion.recommended_path.as_deref(), Some("D:\\Games"));
        assert!(suggestion.needs_choice);
        assert!(suggestion.current_is_fallback);
    }

    #[test]
    fn an_explicit_current_choice_wins_over_a_roomier_drive() {
        let discovery = windows_discovery(vec![
            ("C:\\", VolumeKind::Fixed),
            ("D:\\", VolumeKind::Fixed),
        ]);
        let probe = FakeProbe::new()
            .with("C:\\Games", 900 * GB, 2000 * GB)
            .with("D:\\Games\\Ludex", 100 * GB, 500 * GB);
        let mut settings = settings_at("D:\\Games\\Ludex");
        settings.install_root_confirmed = true;
        let suggestion = suggest_for(
            &discovery,
            &probe,
            &settings,
            Path::new("C:\\Users\\player\\AppData\\Roaming\\ludex"),
        );

        assert_eq!(
            suggestion.recommended_path.as_deref(),
            Some("D:\\Games\\Ludex")
        );
        assert!(!suggestion.needs_choice);
        assert!(!suggestion.current_is_fallback);
        assert_eq!(suggestion.current_path, "D:\\Games\\Ludex");
    }

    #[test]
    fn read_only_drives_are_never_recommended() {
        let discovery = windows_discovery(vec![
            ("C:\\", VolumeKind::Fixed),
            ("D:\\", VolumeKind::Fixed),
        ]);
        let probe = FakeProbe::new()
            .with("C:\\Games", 900 * GB, 2000 * GB)
            .read_only("C:\\Games")
            .with("D:\\Games", 50 * GB, 200 * GB);
        let suggestion = suggest_for(
            &discovery,
            &probe,
            &settings_at("C:\\Users\\player\\AppData\\Roaming\\ludex\\Games"),
            Path::new("C:\\Users\\player\\AppData\\Roaming\\ludex"),
        );

        let c_games = suggestion
            .suggestions
            .iter()
            .find(|item| item.path == "C:\\Games")
            .expect("C:\\Games is still listed");
        assert!(!c_games.writable);
        assert!(!c_games.recommended);
        assert!(c_games.note.is_some());
        assert_eq!(suggestion.recommended_path.as_deref(), Some("D:\\Games"));
    }

    #[test]
    fn nothing_suitable_recommends_the_app_data_fallback() {
        let discovery = windows_discovery(vec![("C:\\", VolumeKind::Fixed)]);
        let probe = FakeProbe::new().read_only("C:\\Games");
        let app_data = Path::new("C:\\Users\\player\\AppData\\Roaming\\ludex");
        let suggestion = suggest_for(
            &discovery,
            &probe,
            &settings_at("C:\\Users\\player\\AppData\\Roaming\\ludex\\Games"),
            app_data,
        );

        assert_eq!(
            suggestion.recommended_path.as_deref(),
            Some("C:\\Users\\player\\AppData\\Roaming\\ludex\\Games")
        );
        let fallback = suggestion
            .suggestions
            .iter()
            .find(|item| item.fallback)
            .expect("the app-data folder is always listed");
        assert!(fallback.recommended);
        assert!(fallback.current);
    }

    #[test]
    fn nearly_full_drives_are_flagged_and_used_only_as_a_last_resort() {
        let discovery = windows_discovery(vec![
            ("C:\\", VolumeKind::Fixed),
            ("D:\\", VolumeKind::Fixed),
            ("E:\\", VolumeKind::Fixed),
        ]);
        let probe = FakeProbe::new()
            // Huge volume, 2 GB left: larger absolute free space than D but
            // nearly full, so it must not win the recommendation.
            .with("C:\\Games", 2 * GB, 100 * GB)
            .with("D:\\Games", 50 * GB, 200 * GB)
            .with("E:\\Games", GB, 20 * GB);
        let suggestion = suggest_for(
            &discovery,
            &probe,
            &settings_at("C:\\Users\\player\\AppData\\Roaming\\ludex\\Games"),
            Path::new("C:\\Users\\player\\AppData\\Roaming\\ludex"),
        );

        let c_games = suggestion
            .suggestions
            .iter()
            .find(|item| item.path == "C:\\Games")
            .unwrap();
        assert!(c_games.nearly_full);
        assert_eq!(suggestion.recommended_path.as_deref(), Some("D:\\Games"));
    }

    #[test]
    fn suggestion_never_creates_the_proposed_folder() {
        let root = std::env::temp_dir().join(format!(
            "ludex-suggest-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&root).unwrap();
        let discovery = Discovery {
            platform: Platform::Linux,
            home: Some(root.join("home")),
            mounts: vec![Mount {
                root: root.join("mnt/games"),
                kind: VolumeKind::Fixed,
                label: None,
            }],
        };
        let suggestion = suggest_for(
            &discovery,
            &NativeSpaceProbe,
            &LocalSettings::for_app_data(&root.join("app-data")),
            &root.join("app-data"),
        );

        assert!(!suggestion.suggestions.is_empty());
        for item in &suggestion.suggestions {
            assert!(
                !Path::new(&item.path).exists(),
                "{} must not be created by a suggestion",
                item.path
            );
            assert!(!item.exists);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn linux_suggests_home_mnt_and_media_mounts() {
        let discovery = Discovery {
            platform: Platform::Linux,
            home: Some(PathBuf::from("/home/player")),
            mounts: vec![
                Mount {
                    root: PathBuf::from("/mnt/games"),
                    kind: VolumeKind::Fixed,
                    label: None,
                },
                Mount {
                    root: PathBuf::from("/media/usb/Kingston"),
                    kind: VolumeKind::Removable,
                    label: None,
                },
                // A mount outside the usual prefixes is not proposed.
                Mount {
                    root: PathBuf::from("/srv/data"),
                    kind: VolumeKind::Fixed,
                    label: None,
                },
            ],
        };
        let probe = FakeProbe::new()
            .with("/home/player/Games", 200 * GB, 500 * GB)
            .with("/mnt/games", 400 * GB, 1000 * GB)
            .with("/media/usb/Kingston/Games", 32 * GB, 64 * GB);
        let suggestion = suggest_for(
            &discovery,
            &probe,
            &LocalSettings::for_app_data(Path::new("/home/player/.local/share/ludex")),
            Path::new("/home/player/.local/share/ludex"),
        );

        let paths: Vec<&str> = suggestion
            .suggestions
            .iter()
            .map(|item| item.path.as_str())
            .collect();
        assert!(paths.contains(&"/home/player/Games"));
        // A mount already named `games` is proposed as-is.
        assert!(paths.contains(&"/mnt/games"));
        assert!(paths.contains(&"/media/usb/Kingston/Games"));
        assert!(!paths.contains(&"/srv/data/Games"));
        // The existing data dir stays available as the fallback.
        assert!(paths.contains(&"/home/player/.local/share/ludex/Games"));
        assert_eq!(suggestion.recommended_path.as_deref(), Some("/mnt/games"));
        assert_eq!(suggestion.platform, Platform::Linux);
    }

    #[test]
    fn macos_suggests_home_and_volumes() {
        let discovery = Discovery {
            platform: Platform::MacOs,
            home: Some(PathBuf::from("/Users/player")),
            mounts: vec![Mount {
                root: PathBuf::from("/Volumes/External"),
                kind: VolumeKind::Fixed,
                label: None,
            }],
        };
        let probe = FakeProbe::new()
            .with("/Users/player/Games", 10 * GB, 100 * GB)
            .with("/Volumes/External/Games", 800 * GB, 2000 * GB);
        let suggestion = suggest_for(
            &discovery,
            &probe,
            &LocalSettings::for_app_data(Path::new(
                "/Users/player/Library/Application Support/ludex",
            )),
            Path::new("/Users/player/Library/Application Support/ludex"),
        );

        let paths: Vec<&str> = suggestion
            .suggestions
            .iter()
            .map(|item| item.path.as_str())
            .collect();
        assert!(paths.contains(&"/Users/player/Games"));
        assert!(paths.contains(&"/Volumes/External/Games"));
        assert_eq!(
            suggestion.recommended_path.as_deref(),
            Some("/Volumes/External/Games")
        );
        assert_eq!(suggestion.platform, Platform::MacOs);
    }

    #[test]
    fn the_current_root_is_listed_once() {
        // The default install folder is also the home candidate on Linux.
        let discovery = Discovery {
            platform: Platform::Linux,
            home: Some(PathBuf::from("/home/player")),
            mounts: Vec::new(),
        };
        let probe = FakeProbe::new().with("/home/player/Games", 100 * GB, 500 * GB);
        let mut settings = LocalSettings::for_app_data(Path::new("/home/player/.config/ludex"));
        settings.install_dir = "/home/player/Games".into();
        let suggestion = suggest_for(
            &discovery,
            &probe,
            &settings,
            Path::new("/home/player/.config/ludex"),
        );

        assert_eq!(
            suggestion
                .suggestions
                .iter()
                .filter(|item| item.path == "/home/player/Games")
                .count(),
            1
        );
        let current = suggestion
            .suggestions
            .iter()
            .find(|item| item.path == "/home/player/Games")
            .unwrap();
        assert!(current.current);
        assert_eq!(
            suggestion.recommended_path.as_deref(),
            Some("/home/player/Games")
        );
    }

    #[test]
    fn existing_settings_are_not_modified() {
        let discovery = windows_discovery(vec![("C:\\", VolumeKind::Fixed)]);
        let probe = FakeProbe::new().with("C:\\Games", 100 * GB, 500 * GB);
        let mut settings = LocalSettings::for_app_data(Path::new("C:\\data"));
        settings.locations = vec![crate::settings::Location {
            id: "keep".into(),
            label: "Keep".into(),
            path: "D:\\My Games".into(),
            kind: crate::settings::LocationKind::Install,
            default: true,
        }];
        settings.install_dir = "D:\\My Games".into();
        let before = serde_json::to_value(&settings).unwrap();

        let _ = suggest_for(&discovery, &probe, &settings, Path::new("C:\\data"));

        assert_eq!(serde_json::to_value(&settings).unwrap(), before);
        assert_eq!(settings.install_path(), PathBuf::from("D:\\My Games"));
    }

    #[test]
    fn nearly_full_threshold_covers_a_small_absolute_share() {
        assert!(nearly_full(5 * GB, Some(1000 * GB)));
        assert!(nearly_full(100 * GB, Some(2000 * GB)));
        assert!(!nearly_full(100 * GB, Some(1000 * GB)));
        // Unknown total: only the absolute floor applies.
        assert!(nearly_full(2 * GB, None));
        assert!(!nearly_full(20 * GB, None));
    }
}
