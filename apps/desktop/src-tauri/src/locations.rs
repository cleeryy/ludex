//! Named install/download/temp locations (#99).
//!
//! Settings own the list; this module turns it into usable paths: it probes
//! free space and writability without creating anything, orders the picker and
//! guards an install against a full disk. A location that is missing (unplugged
//! USB drive, unmounted share) is reported as such and only fails the job that
//! asked for it — the queue and the rest of the settings stay usable.

use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::settings::{
    DownloadStaging, LocalSettings, Location, LocationKind, DEFAULT_MIN_FREE_BYTES,
};
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
    /// Free space kept free on this library (#290), in bytes.
    pub min_free_bytes: u64,
    /// Mount point serving the folder, when a mount is mounted.
    pub mount_point: Option<String>,
    /// Set when the folder's mount is absent (#290): the drive is unplugged or
    /// the share is not mounted. The library is not deleted; it comes back
    /// when the mount does.
    pub offline: Option<String>,
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

pub(crate) fn probe_writable(directory: &Path) -> bool {
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

/// Probe a location without creating it or triggering an automount.
pub fn probe(location: &Location) -> LocationStatus {
    let path = PathBuf::from(location.path.trim());
    let mount = crate::mounts::status(&path);
    let mount_point = mount.mount_point().map(str::to_string);
    let offline = mount.message().map(str::to_string);
    let exists = path.is_dir();
    // An unmounted mount point is a folder on the root filesystem: claiming it
    // is writable would let a job write into the root disk by mistake.
    let writable = offline.is_none() && probe_writable(&path);
    let existing = crate::paths::nearest_existing(&path);
    let (free_bytes, total_bytes) = if offline.is_some() {
        (None, None)
    } else {
        (
            existing
                .as_deref()
                .and_then(|dir| fs2::available_space(dir).ok()),
            existing
                .as_deref()
                .and_then(|dir| fs2::total_space(dir).ok()),
        )
    };

    let note = if location.path.trim().is_empty() {
        Some("No folder is set for this location.".to_string())
    } else if let Some(message) = offline.clone() {
        Some(message)
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
        min_free_bytes: location.min_free_bytes,
        mount_point,
        offline,
    }
}

/// Probe every location in settings. Never fails: an unplugged drive must not
/// break the Settings screen.
pub fn list(settings: &LocalSettings) -> Vec<LocationStatus> {
    settings.locations.iter().map(probe).collect()
}

/// Probe one path (the Settings manager's "Check" button).
#[cfg(test)]
pub fn probe_path(path: &str) -> Result<LocationStatus, String> {
    probe_path_with(path, DEFAULT_MIN_FREE_BYTES, false)
}

/// Probe a path and, when `trigger` is set, try its automount once (#290).
pub fn probe_path_with(
    path: &str,
    min_free_bytes: u64,
    trigger: bool,
) -> Result<LocationStatus, String> {
    validate("Location", path)?;
    let location = Location {
        id: String::new(),
        label: "Location".to_string(),
        path: path.trim().to_string(),
        kind: LocationKind::Install,
        default: false,
        min_free_bytes,
    };
    if trigger {
        // Start the `.automount` (or mount the `_netdev` entry) before the
        // caller decides the location is missing.
        let _ = crate::mounts::ensure(Path::new(&location.path));
    }
    Ok(probe(&location))
}

/// Check a path for an install: tries the automount, then reports state.
pub fn ensure_path(path: &Path) -> Result<LocationStatus, String> {
    probe_path_with(&path.to_string_lossy(), DEFAULT_MIN_FREE_BYTES, true)
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
#[cfg(test)]
pub fn guard_free_space(required_bytes: u64, free_bytes: u64) -> Result<(), String> {
    guard_free_space_with(required_bytes, free_bytes, FREE_SPACE_RESERVE_BYTES)
}

/// [`guard_free_space`] with the library's own floor (#290).
///
/// The floor is at least the historical 256 MiB reserve, so a location whose
/// floor is disabled still keeps the OS room to breathe.
pub fn guard_free_space_with(
    required_bytes: u64,
    free_bytes: u64,
    min_free_bytes: u64,
) -> Result<(), String> {
    let floor = min_free_bytes.max(FREE_SPACE_RESERVE_BYTES);
    let needed = required_bytes.saturating_add(floor);
    if free_bytes < needed {
        let missing = needed - free_bytes;
        return Err(format!(
            "Not enough free space: {required} needed plus a {floor} free-space floor, only {free} available on this location — {missing} short. Free space or move the library, then resume.",
            required = human_bytes(required_bytes),
            floor = human_bytes(floor),
            free = human_bytes(free_bytes),
            missing = human_bytes(missing),
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
/// download folder follows `downloadStaging` (#226): `installVolume` stages the
/// archive next to the install root, unless this job explicitly picked a
/// download location or path, which always wins.
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

    // An explicit choice for this job (the install dialog's picker) is never
    // overridden by the global staging policy.
    let explicit_download = match download_location.filter(|value| !value.trim().is_empty()) {
        Some(id) => Some(settings.resolve_location(Some(id), LocationKind::Download)?),
        None => legacy_download
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
    };
    let staged_on_install_volume =
        explicit_download.is_none() && settings.download_staging == DownloadStaging::InstallVolume;
    let download_dir =
        explicit_download.unwrap_or_else(|| settings.effective_download_dir(&install_root));

    let cross_filesystem = !same_filesystem(&install_root, &download_dir);
    let peak_note = if stream {
        "Streamed: files are unpacked while downloading, so no archive is stored.".to_string()
    } else if staged_on_install_volume {
        format!(
            "The archive is staged in {} on the install volume so extraction reads it locally; peak usage there is roughly twice the archive size.{}",
            download_dir.display(),
            if settings.keep_archives {
                " The archive is kept in that folder afterwards."
            } else {
                " The archive is deleted after extraction."
            }
        )
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
            min_free_bytes: crate::settings::DEFAULT_MIN_FREE_BYTES,
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
                min_free_bytes: crate::settings::DEFAULT_MIN_FREE_BYTES,
                mount_point: None,
                offline: None,
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
                min_free_bytes: crate::settings::DEFAULT_MIN_FREE_BYTES,
                mount_point: None,
                offline: None,
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
                min_free_bytes: crate::settings::DEFAULT_MIN_FREE_BYTES,
                mount_point: None,
                offline: None,
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
                min_free_bytes: crate::settings::DEFAULT_MIN_FREE_BYTES,
                mount_point: None,
                offline: None,
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
                min_free_bytes: crate::settings::DEFAULT_MIN_FREE_BYTES,
                mount_point: None,
                offline: None,
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
    fn plan_stages_on_the_install_volume_when_asked() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.locations = vec![
            location("games", "Games", "/games", LocationKind::Install, true),
            location("nas", "NAS", "/mnt/nas", LocationKind::Download, true),
        ];
        settings.normalize(Path::new("/data"));
        // #235: the default stages on the install volume, so a NAS download
        // location is not written, hashed and extracted over SMB.
        let default = plan_install(&settings, None, None, None, None, false).unwrap();
        assert_eq!(default.install_root, PathBuf::from("/games"));
        assert_eq!(default.download_dir, PathBuf::from("/games/.ludex-tmp"));
        assert!(default.peak_note.contains("install volume"));

        // An explicit `configured` choice uses the download location again.
        settings.download_staging = DownloadStaging::Configured;
        let configured = plan_install(&settings, None, None, None, None, false).unwrap();
        assert_eq!(configured.download_dir, PathBuf::from("/mnt/nas"));

        // #226: staging on the install volume overrides the configured one.
        settings.download_staging = DownloadStaging::InstallVolume;
        let plan = plan_install(&settings, None, None, None, None, false).unwrap();
        assert_eq!(plan.install_root, PathBuf::from("/games"));
        assert_eq!(plan.download_dir, PathBuf::from("/games/.ludex-tmp"));
        assert!(plan.peak_note.contains("install volume"));
        assert!(plan.peak_note.contains("deleted after extraction"));

        // A kept archive is mentioned where it stays.
        settings.keep_archives = true;
        let kept = plan_install(&settings, None, None, None, None, false).unwrap();
        assert!(kept.peak_note.contains("kept in that folder"));

        // An explicit per-job choice still wins over the policy.
        let picked = plan_install(&settings, None, Some("nas"), None, None, false).unwrap();
        assert_eq!(picked.download_dir, PathBuf::from("/mnt/nas"));
        assert!(!picked.peak_note.contains("install volume"));
        let legacy =
            plan_install(&settings, None, None, None, Some("D:\\Temp\\Ludex"), false).unwrap();
        assert_eq!(legacy.download_dir, PathBuf::from("D:\\Temp\\Ludex"));
    }

    #[test]
    fn probe_path_rejects_relative_paths() {
        assert!(probe_path("relative/games").is_err());
        assert!(probe_path("/tmp").is_ok());
    }
}
