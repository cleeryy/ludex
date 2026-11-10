//! Named install/download/temp locations (#99).
//!
//! Settings own the list; this module turns it into usable paths: it probes
//! free space and writability without creating anything, orders the picker and
//! guards an install against a full disk. A location that is missing (unplugged
//! USB drive, unmounted share) is reported as such and only fails the job that
//! asked for it — the queue and the rest of the settings stay usable.

use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::settings::{LocalSettings, Location, LocationKind};
use crate::util::human_bytes;

/// Space kept free on an install/download volume. Extraction needs roughly the
/// archive again, and the OS needs room to breathe.
pub const FREE_SPACE_RESERVE_BYTES: u64 = 256 * 1024 * 1024;

/// One location with live probe data, shown in Settings → Locations and in the
/// per-install picker.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationStatus {
    pub id: String,
    pub label: String,
    pub path: String,
    pub kind: LocationKind,
    pub default: bool,
    /// True when the folder itself exists.
    pub exists: bool,
    /// True when Ludex can create a file inside the folder (or its nearest
    /// existing parent, for a folder that does not exist yet).
    pub writable: bool,
    pub free_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    /// Human note: missing drive, read-only folder, same volume as installs…
    pub note: Option<String>,
    /// Set by `check_location` when the caller asked about a download size.
    pub required_bytes: Option<u64>,
    pub free_space_ok: Option<bool>,
}

/// Resolved paths for one install job.
#[derive(Debug, Clone)]
pub struct InstallLocationPlan {
    pub install_root: PathBuf,
    pub download_dir: PathBuf,
    /// The archive and the extracted files live on different volumes.
    pub cross_filesystem: bool,
    /// Peak usage note shown in the UI: archive + extracted copy.
    pub peak_note: String,
}

/// Trim and validate a user-entered location. Paths are not created here.
pub fn validate(label: &str, path: &str) -> Result<(), String> {
    if label.trim().is_empty() {
        return Err("Give the location a name.".to_string());
    }
    if label.chars().count() > 60 {
        return Err("Keep the location name under 60 characters.".to_string());
    }
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Choose a folder for this location.".to_string());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("The folder path contains control characters.".to_string());
    }
    if !Path::new(trimmed).is_absolute() {
        return Err(format!("Use an absolute folder path (got “{trimmed}”)."));
    }
    let path = Path::new(trimmed);
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("The folder path must not contain “..”.".to_string());
    }
    if path.parent().is_none() {
        return Err("Choose a folder inside a drive, not the drive root.".to_string());
    }
    Ok(())
}

fn probe_writable(directory: &Path) -> bool {
    let Ok(existing) = crate::paths::nearest_existing(directory).ok_or(()) else {
        return false;
    };
    if !existing.is_dir() {
        return false;
    }
    let probe = existing.join(format!(
        ".ludex-write-test-{}",
        uuid::Uuid::new_v4().simple()
    ));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Probe a location without creating it.
pub fn probe(location: &Location) -> LocationStatus {
    let path = PathBuf::from(location.path.trim());
    let exists = path.is_dir();
    let writable = probe_writable(&path);
    let existing = crate::paths::nearest_existing(&path);
    let free_bytes = existing
        .as_deref()
        .and_then(|dir| fs2::available_space(dir).ok());
    let total_bytes = existing
        .as_deref()
        .and_then(|dir| fs2::total_space(dir).ok());

    let note = if location.path.trim().is_empty() {
        Some("No folder is set for this location.".to_string())
    } else if !exists && writable {
        Some("The folder will be created when it is first used.".to_string())
    } else if !exists {
        Some(
            "The folder is missing and its parent is not writable. Is the drive plugged in?"
                .to_string(),
        )
    } else if !writable {
        Some("This folder is read-only for your user.".to_string())
    } else {
        None
    };

    LocationStatus {
        id: location.id.clone(),
        label: location.label.clone(),
        path: location.path.clone(),
        kind: location.kind,
        default: location.default,
        exists,
        writable,
        free_bytes,
        total_bytes,
        note,
        required_bytes: None,
        free_space_ok: None,
    }
}

/// Probe every location in settings. Never fails: an unplugged drive must not
/// break the Settings screen.
pub fn list(settings: &LocalSettings) -> Vec<LocationStatus> {
    settings.locations.iter().map(probe).collect()
}

/// Probe one path (the Settings manager's "Check" button).
pub fn probe_path(path: &str) -> Result<LocationStatus, String> {
    validate("Location", path)?;
    Ok(probe(&Location {
        id: String::new(),
        label: "Location".to_string(),
        path: path.trim().to_string(),
        kind: LocationKind::Install,
        default: false,
    }))
}

/// Picker order: the default for the requested kind first, then writable
/// locations by free space (roomiest first), then read-only, then missing ones.
pub fn picker_order(mut statuses: Vec<LocationStatus>) -> Vec<LocationStatus> {
    statuses.sort_by(|left, right| {
        let rank = |status: &LocationStatus| {
            if status.default {
                0
            } else if status.writable {
                1
            } else if status.exists {
                2
            } else {
                3
            }
        };
        rank(left)
            .cmp(&rank(right))
            .then_with(|| {
                right
                    .free_bytes
                    .unwrap_or(0)
                    .cmp(&left.free_bytes.unwrap_or(0))
            })
            .then_with(|| left.label.to_lowercase().cmp(&right.label.to_lowercase()))
    });
    statuses
}

/// Reject a job that cannot fit on the selected volume.
///
/// `free` is what the filesystem reports now. The caller adds the archive size
/// when the archive and the extracted game share the volume.
pub fn guard_free_space(required_bytes: u64, free_bytes: u64) -> Result<(), String> {
    let needed = required_bytes.saturating_add(FREE_SPACE_RESERVE_BYTES);
    if free_bytes < needed {
        return Err(format!(
            "Not enough free space: {required} needed plus a {reserve} reserve, only {free} available on this location.",
            required = human_bytes(required_bytes),
            reserve = human_bytes(FREE_SPACE_RESERVE_BYTES),
            free = human_bytes(free_bytes),
        ));
    }
    Ok(())
}

/// True when both paths resolve to the same device/volume.
pub fn same_filesystem(left: &Path, right: &Path) -> bool {
    let left = crate::paths::nearest_existing(left);
    let right = crate::paths::nearest_existing(right);
    let (Some(left), Some(right)) = (left, right) else {
        return false;
    };
    if left == right {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(&left), std::fs::metadata(&right)) {
            (Ok(left), Ok(right)) => left.dev() == right.dev(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        // Windows: compare the volume prefix (`C:\`, `\\server\share\`).
        let volume = |path: &Path| {
            let mut components = path.components();
            match components.next() {
                Some(Component::Prefix(prefix)) => {
                    prefix.as_os_str().to_string_lossy().to_lowercase()
                }
                _ => String::new(),
            }
        };
        let left_volume = volume(&left);
        !left_volume.is_empty() && left_volume == volume(&right)
    }
}

/// Resolve the install and download folders for a job.
///
/// `install_location` and `download_location` are location ids from the picker;
/// `legacy_install`/`legacy_download` are direct paths from an older UI. The
/// download fallback keeps the archive on the install volume.
pub fn plan_install(
    settings: &LocalSettings,
    install_location: Option<&str>,
    download_location: Option<&str>,
    legacy_install: Option<&str>,
    legacy_download: Option<&str>,
    stream: bool,
) -> Result<InstallLocationPlan, String> {
    let install_root = match install_location.filter(|value| !value.trim().is_empty()) {
        Some(id) => settings.resolve_location(Some(id), LocationKind::Install)?,
        None => match legacy_install
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(path) => PathBuf::from(path),
            None => settings.install_path(),
        },
    };
    if install_root.as_os_str().is_empty() {
        return Err("Choose an install location in Settings → Locations.".to_string());
    }

    let download_dir = match download_location.filter(|value| !value.trim().is_empty()) {
        Some(id) => settings.resolve_location(Some(id), LocationKind::Download)?,
        None => {
            if let Some(path) = legacy_download
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                PathBuf::from(path)
            } else if let Some(location) = settings.default_location(LocationKind::Download) {
                PathBuf::from(location.path.trim())
            } else if !settings.download_dir.trim().is_empty() {
                settings.download_path()
            } else {
                install_root.join(".ludex-tmp")
            }
        }
    };

    let cross_filesystem = !same_filesystem(&install_root, &download_dir);
    let peak_note = if stream {
        "Streamed: files are unpacked while downloading, so no archive is stored.".to_string()
    } else if cross_filesystem {
        format!(
            "The archive is downloaded to {} and unpacked into {}. Peak usage is archive + extracted copy on two volumes.",
            download_dir.display(),
            install_root.display()
        )
    } else {
        format!(
            "Peak usage on this volume is roughly twice the archive size: the archive plus the extracted files in {}.",
            install_root.display()
        )
    };

    Ok(InstallLocationPlan {
        install_root,
        download_dir,
        cross_filesystem,
        peak_note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Location;

    fn location(id: &str, label: &str, path: &str, kind: LocationKind, default: bool) -> Location {
        Location {
            id: id.to_string(),
            label: label.to_string(),
            path: path.to_string(),
            kind,
            default,
        }
    }

    #[test]
    fn validate_rejects_relative_roots_and_traversal() {
        assert!(validate("Games", "/srv/games").is_ok());
        assert!(validate("", "/srv/games").is_err());
        assert!(validate("Games", "games").is_err());
        assert!(validate("Games", "/srv/../etc").is_err());
        assert!(validate("Games", "/").is_err());
        assert!(validate("Games", "   ").is_err());
        assert!(validate("Games", "/srv/ga\u{0}mes").is_err());
    }

    #[test]
    fn picker_puts_the_default_first_then_roomy_writable() {
        let statuses = vec![
            LocationStatus {
                id: "usb".into(),
                label: "USB".into(),
                path: "/mnt/usb".into(),
                kind: LocationKind::Install,
                default: false,
                exists: false,
                writable: false,
                free_bytes: None,
                total_bytes: None,
                note: None,
                required_bytes: None,
                free_space_ok: None,
            },
            LocationStatus {
                id: "small".into(),
                label: "Small".into(),
                path: "/srv/small".into(),
                kind: LocationKind::Install,
                default: false,
                exists: true,
                writable: true,
                free_bytes: Some(10),
                total_bytes: Some(20),
                note: None,
                required_bytes: None,
                free_space_ok: None,
            },
            LocationStatus {
                id: "default".into(),
                label: "Default".into(),
                path: "/srv/games".into(),
                kind: LocationKind::Install,
                default: true,
                exists: true,
                writable: true,
                free_bytes: Some(1),
                total_bytes: Some(2),
                note: None,
                required_bytes: None,
                free_space_ok: None,
            },
            LocationStatus {
                id: "big".into(),
                label: "Big".into(),
                path: "/srv/big".into(),
                kind: LocationKind::Install,
                default: false,
                exists: true,
                writable: true,
                free_bytes: Some(100),
                total_bytes: Some(200),
                note: None,
                required_bytes: None,
                free_space_ok: None,
            },
            LocationStatus {
                id: "ro".into(),
                label: "Read only".into(),
                path: "/mnt/ro".into(),
                kind: LocationKind::Install,
                default: false,
                exists: true,
                writable: false,
                free_bytes: Some(500),
                total_bytes: None,
                note: None,
                required_bytes: None,
                free_space_ok: None,
            },
        ];

        let ordered: Vec<String> = picker_order(statuses)
            .into_iter()
            .map(|status| status.id)
            .collect();
        assert_eq!(ordered, vec!["default", "big", "small", "ro", "usb"]);
    }

    #[test]
    fn free_space_guard_keeps_a_reserve() {
        let needed = 1024 * 1024 * 1024u64;
        assert!(guard_free_space(needed, needed + FREE_SPACE_RESERVE_BYTES).is_ok());
        assert!(guard_free_space(needed, needed).is_err());
        assert!(guard_free_space(needed, 0).is_err());
    }

    #[test]
    fn missing_locations_are_reported_not_fatal() {
        let unique = format!("/tmp/ludex-missing-{}", uuid::Uuid::new_v4().simple());
        let status = probe(&location(
            "missing",
            "Missing",
            &unique,
            LocationKind::Install,
            true,
        ));
        assert!(!status.exists);
        assert!(status.note.is_some());
        // Listing never fails, whatever the drive state.
        let settings = LocalSettings {
            locations: vec![location(
                "missing",
                "Missing",
                &unique,
                LocationKind::Install,
                true,
            )],
            ..LocalSettings::default()
        };
        assert_eq!(list(&settings).len(), 1);
    }

    #[test]
    fn same_filesystem_detects_the_same_temp_volume() {
        let root = std::env::temp_dir();
        assert!(same_filesystem(&root, &root.join("ludex-test-subdir")));
    }

    #[test]
    fn plan_defaults_to_the_install_volume_for_downloads() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.locations = vec![location(
            "games",
            "Games",
            "/games",
            LocationKind::Install,
            true,
        )];
        settings.normalize(Path::new("/data"));

        let plan = plan_install(&settings, None, None, None, None, false).unwrap();
        assert_eq!(plan.install_root, PathBuf::from("/games"));
        assert_eq!(plan.download_dir, PathBuf::from("/games/.ludex-tmp"));
    }

    #[test]
    fn plan_uses_named_locations_and_notes_cross_volume() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.locations = vec![
            location("games", "Games", "/games", LocationKind::Install, true),
            location("fast", "Fast", "/mnt/fast", LocationKind::Download, true),
        ];
        settings.normalize(Path::new("/data"));

        let plan = plan_install(&settings, Some("games"), Some("fast"), None, None, false).unwrap();
        assert_eq!(plan.install_root, PathBuf::from("/games"));
        assert_eq!(plan.download_dir, PathBuf::from("/mnt/fast"));
        assert!(plan.peak_note.contains("two volumes") || plan.peak_note.contains("Peak usage"));

        let unknown = plan_install(&settings, Some("gone"), None, None, None, false);
        assert!(unknown.unwrap_err().contains("no longer exists"));
    }

    #[test]
    fn probe_path_rejects_relative_paths() {
        assert!(probe_path("relative/games").is_err());
        assert!(probe_path("/tmp").is_ok());
    }
}
