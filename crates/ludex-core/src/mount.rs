//! Mount awareness for library roots (#290).
//!
//! A library does not have to live on the root filesystem of the machine: a
//! NAS export, an external SSD or a second disk mounted through fstab are all
//! valid roots. This module holds the pure parts of that support so the server
//! and the desktop share one implementation and one set of rules:
//!
//! - [`parse_mountinfo`] / [`select_mount`] resolve the mount that backs a
//!   path from `/proc/self/mountinfo` text (fixtures in tests, the real file
//!   in production).
//! - [`automount_plan`] decides how to wake an absent mount: the matching
//!   `.automount` unit when systemd generated one, otherwise the fstab entry
//!   flagged `_netdev` (or `noauto` / `x-systemd.automount`).
//! - [`MountIdentity`] fingerprints a mount (device id + mount-point marker)
//!   so a reconnected root resumes from the stored index instead of walking
//!   the whole tree again.
//! - [`preflight_requirement`] and [`check_free_space`] implement the
//!   per-library free-space floor and the estimate taken before the first
//!   byte of an install.
//!
//! Nothing here performs I/O: the probes live in the server (`crate::mounts`)
//! and the desktop app, which inject their own fixtures in tests.

use std::path::Path;

/// Free space kept free on a library by default: 5 GiB.
pub const DEFAULT_MIN_FREE_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// Extraction overhead + staging, applied to the archive size before the first
/// byte of an install: archive × 1.1.
pub const PREFLIGHT_OVERHEAD_PERCENT: u64 = 10;

/// One byte in GiB, for the messages the user reads.
pub const GIB: u64 = 1024 * 1024 * 1024;

/// One line of `/proc/self/mountinfo`, reduced to what a library needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    /// Where the filesystem is mounted, unescaped (`/mnt/nas`).
    pub mount_point: String,
    /// Filesystem type (`ext4`, `nfs4`, `cifs`, `fuse.sshfs`…).
    pub fs_type: String,
    /// Device or share (`/dev/sdb1`, `nas:/export`).
    pub source: String,
    /// Mount options and super options, merged.
    pub options: Vec<String>,
}

impl MountEntry {
    /// True when this entry is the machine's root filesystem.
    #[must_use]
    pub fn is_root_filesystem(&self) -> bool {
        self.mount_point == "/"
    }

    /// True when the entry carries `option` (mount or super options).
    #[must_use]
    pub fn has_option(&self, option: &str) -> bool {
        self.options.iter().any(|known| known == option)
    }

    /// True when this is a network filesystem, mounted or configured.
    #[must_use]
    pub fn is_network(&self) -> bool {
        matches!(
            self.fs_type.as_str(),
            "nfs"
                | "nfs4"
                | "cifs"
                | "smbfs"
                | "smb3"
                | "fuse.sshfs"
                | "fuse.rclone"
                | "9p"
                | "ceph"
                | "glusterfs"
                | "afs"
        ) || self.has_option("_netdev")
    }
}

/// Parses `/proc/self/mountinfo` text.
///
/// Each line is `id parent major:minor root mount_point options [optional
/// fields...] - fs_type source super_options`. Mount points and sources are
/// octal-escaped (`\040` for a space); the optional fields are skipped.
#[must_use]
pub fn parse_mountinfo(text: &str) -> Vec<MountEntry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let separator = fields.iter().position(|field| *field == "-");
        let Some(separator) = separator else {
            continue;
        };
        if fields.len() < 5 || fields.len() < separator + 3 {
            continue;
        }
        let mount_point = unescape_mount_field(fields[4]);
        let fs_type = fields[separator + 1].to_string();
        let source = unescape_mount_field(fields[separator + 2]);
        let mut options: Vec<String> = Vec::new();
        for option in fields[5].split(',') {
            if !option.is_empty() && !options.iter().any(|known| known == option) {
                options.push(option.to_string());
            }
        }
        for option in fields[separator + 3..].join(",").split(',') {
            if !option.is_empty() && !options.iter().any(|known| known == option) {
                options.push(option.to_string());
            }
        }
        entries.push(MountEntry {
            mount_point,
            fs_type,
            source,
            options,
        });
    }
    entries
}

/// Decodes the octal escapes `/proc/self/mountinfo` and `/etc/fstab` use.
#[must_use]
pub fn unescape_mount_field(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let octal = &raw[index + 1..index + 4];
            if octal.bytes().all(|byte| (b'0'..=b'7').contains(&byte)) {
                if let Ok(value) = u8::from_str_radix(octal, 8) {
                    out.push(value);
                    index += 4;
                    continue;
                }
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// The mount that backs `path`: the longest mount point that is an ancestor.
///
/// Component boundaries are respected, so `/mnt/seagate2` is not resolved to a
/// `/mnt/seagate` mount. A missing path still resolves to its nearest mounted
/// ancestor, which is what lets the caller tell "folder absent on a healthy
/// mount" from "mount absent".
#[must_use]
pub fn select_mount<'a>(mounts: &'a [MountEntry], path: &Path) -> Option<&'a MountEntry> {
    let needle = normalize_path(&path.to_string_lossy());
    mounts
        .iter()
        .filter(|mount| mount_contains(&mount.mount_point, &needle))
        .max_by_key(|mount| normalize_path(&mount.mount_point).len())
}

/// True when the path resolves to the root filesystem (or to no mount at all).
#[must_use]
pub fn on_root_filesystem(mounts: &[MountEntry], path: &Path) -> bool {
    match select_mount(mounts, path) {
        Some(mount) => mount.is_root_filesystem(),
        None => true,
    }
}

fn normalize_path(path: &str) -> String {
    let mut value = path.replace('\\', "/");
    while value.len() > 1 && value.ends_with('/') {
        value.pop();
    }
    if value.is_empty() {
        "/".to_string()
    } else {
        value
    }
}

fn mount_contains(mount_point: &str, path: &str) -> bool {
    let mount = normalize_path(mount_point);
    if mount == "/" {
        return true;
    }
    path == mount
        || path
            .strip_prefix(&mount)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// How an absent mount can be woken up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomountKind {
    /// A systemd `.automount` unit exists for the mount point.
    SystemdUnit,
    /// The fstab entry is flagged `_netdev` / `noauto` / `x-systemd.automount`.
    FstabMount,
}

/// One way to trigger an automount.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomountPlan {
    /// Whether systemd or a plain `mount` should trigger it.
    pub kind: AutomountKind,
    /// Mount point the plan targets (`/mnt/nas`).
    pub mount_point: String,
    /// Unit name for [`AutomountKind::SystemdUnit`]:
    /// `mnt-nas.automount`.
    pub unit: Option<String>,
    /// Why this plan was chosen, for logs and diagnostics.
    pub reason: &'static str,
}

impl AutomountPlan {
    /// Human sentence describing the attempt.
    #[must_use]
    pub fn describe(&self) -> String {
        match self.kind {
            AutomountKind::SystemdUnit => format!(
                "starting {} to mount {}",
                self.unit.as_deref().unwrap_or("the automount unit"),
                self.mount_point
            ),
            AutomountKind::FstabMount => {
                format!("mounting the fstab entry for {}", self.mount_point)
            }
        }
    }
}

/// One fstab line, reduced to the fields that matter here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FstabEntry {
    /// Device or share (`nas.local:/export`).
    pub source: String,
    /// Where it is mounted (`/mnt/nas`).
    pub mount_point: String,
    /// Mount options from the fourth field.
    pub options: Vec<String>,
}

impl FstabEntry {
    /// True when the entry is a network mount or asks systemd to automount.
    #[must_use]
    pub fn is_automountable(&self) -> bool {
        self.options.iter().any(|option| {
            matches!(
                option.as_str(),
                "_netdev" | "noauto" | "x-systemd.automount" | "x-systemd.device-timeout=0"
            ) || option.starts_with("x-systemd.automount")
        })
    }
}

/// Parses `/etc/fstab`, skipping comments and blank lines.
#[must_use]
pub fn parse_fstab(text: &str) -> Vec<FstabEntry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = trimmed.split_whitespace().collect();
        if fields.len() < 4 {
            continue;
        }
        entries.push(FstabEntry {
            source: unescape_mount_field(fields[0]),
            mount_point: unescape_mount_field(fields[1]),
            options: fields[3].split(',').map(str::to_string).collect(),
        });
    }
    entries
}

/// Escapes a mount point the way `systemd-escape --path` does, so a path can
/// be matched against the `.automount` units on the machine.
///
/// `"/mnt/nas"` becomes `"mnt-nas"`; an existing `-` is escaped as `\x2d`.
#[must_use]
pub fn systemd_escape_mount_point(path: &str) -> String {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return "-".to_string();
    }
    let mut out = String::with_capacity(trimmed.len());
    for character in trimmed.chars() {
        match character {
            '/' => out.push('-'),
            '-' => out.push_str("\\x2d"),
            value
                if value.is_ascii_alphanumeric()
                    || value == '_'
                    || value == '.'
                    || value == ':' =>
            {
                out.push(value);
            }
            value => {
                let mut buffer = [0_u8; 4];
                for byte in value.encode_utf8(&mut buffer).as_bytes() {
                    out.push_str(&format!("\\x{byte:02x}"));
                }
            }
        }
    }
    out
}

/// The configured mount point (fstab or systemd) that would serve `path`.
///
/// Returns the deepest configured mount point that is an ancestor of `path`,
/// whether or not it is currently mounted.
#[must_use]
pub fn configured_mount_point(
    fstab: &[FstabEntry],
    units: &[String],
    path: &Path,
) -> Option<String> {
    let needle = normalize_path(&path.to_string_lossy());
    fstab
        .iter()
        .map(|entry| Some(entry.mount_point.clone()))
        .chain(units.iter().map(|unit| mount_point_from_unit(unit)))
        .flatten()
        .filter(|mount| mount_contains(mount, &needle))
        .max_by_key(|mount| normalize_path(mount).len())
}

/// Picks the automount to try for a path on an absent mount.
///
/// A unit whose name matches the escaped mount point wins: starting it makes
/// systemd mount the share without the app knowing the fstab syntax. Without a
/// unit, an fstab entry flagged `_netdev` (or automount-related options) is
/// mounted directly. `None` means the mount is not automountable and the root
/// should stay offline.
#[must_use]
pub fn automount_plan(
    mounts: &[MountEntry],
    fstab: &[FstabEntry],
    units: &[String],
    path: &Path,
) -> Option<AutomountPlan> {
    let needle = normalize_path(&path.to_string_lossy());
    let mut candidates: Vec<(String, Option<String>)> = Vec::new();

    for entry in fstab {
        if !entry.is_automountable() || !mount_contains(&entry.mount_point, &needle) {
            continue;
        }
        if is_mounted(mounts, &entry.mount_point) {
            continue;
        }
        let wanted = format!(
            "{}.automount",
            systemd_escape_mount_point(&entry.mount_point)
        );
        let unit = units.iter().find_map(|unit| {
            let name = normalize_unit(unit);
            (name == wanted).then_some(name)
        });
        candidates.push((entry.mount_point.clone(), unit));
    }

    for unit in units {
        let name = normalize_unit(unit);
        let Some(mount_point) = mount_point_from_unit(&name) else {
            continue;
        };
        if !mount_contains(&mount_point, &needle) || is_mounted(mounts, &mount_point) {
            continue;
        }
        // A unit without an fstab entry is still usable: systemd generated it.
        candidates.push((mount_point, Some(name)));
    }

    candidates.sort_by_key(|(mount_point, _)| std::cmp::Reverse(normalize_path(mount_point).len()));
    let (mount_point, unit) = candidates.into_iter().next()?;
    let unit = unit.or_else(|| {
        units.iter().find_map(|candidate| {
            let name = normalize_unit(candidate);
            (name == format!("{}.automount", systemd_escape_mount_point(&mount_point)))
                .then_some(name)
        })
    });
    Some(match unit {
        Some(unit) => AutomountPlan {
            kind: AutomountKind::SystemdUnit,
            mount_point,
            unit: Some(unit),
            reason: "a matching .automount unit is present",
        },
        None => AutomountPlan {
            kind: AutomountKind::FstabMount,
            mount_point,
            unit: None,
            reason: "the fstab entry is flagged for automount",
        },
    })
}

fn normalize_unit(unit: &str) -> String {
    let name = unit.trim();
    if name.ends_with(".automount") {
        name.to_string()
    } else {
        format!("{name}.automount")
    }
}

/// Rebuilds a mount point from an escaped `.automount` unit name.
fn mount_point_from_unit(unit: &str) -> Option<String> {
    let name = unit.trim().trim_end_matches(".automount");
    if name.is_empty() || name.contains('@') {
        return None;
    }
    if name == "-" {
        return Some("/".to_string());
    }
    let mut decoded = String::with_capacity(name.len() + 1);
    decoded.push('/');
    let bytes = name.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'-' => {
                decoded.push('/');
                index += 1;
            }
            b'\\'
                if index + 3 < bytes.len()
                    && bytes[index + 1] == b'x'
                    && (bytes[index + 2] as char).is_ascii_hexdigit()
                    && (bytes[index + 3] as char).is_ascii_hexdigit() =>
            {
                let hex = &name[index + 2..index + 4];
                match u8::from_str_radix(hex, 16) {
                    Ok(value) => {
                        decoded.push(value as char);
                        index += 4;
                    }
                    Err(_) => {
                        decoded.push('\\');
                        index += 1;
                    }
                }
            }
            value => {
                decoded.push(value as char);
                index += 1;
            }
        }
    }
    Some(decoded)
}

fn is_mounted(mounts: &[MountEntry], mount_point: &str) -> bool {
    let wanted = normalize_path(mount_point);
    mounts
        .iter()
        .any(|mount| normalize_path(&mount.mount_point) == wanted)
}

/// Identity of one mount, stable across a replug of the same filesystem.
///
/// `device_id` is the OS device id of the mount point; `marker` is the
/// mount-point mtime in Unix seconds (network mounts keep no inode identity,
/// the mtime moves when they remount). A changed identity forces a rescan; an
/// unchanged one lets the root reuse its stored index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MountIdentity {
    /// OS device id of the filesystem (`stat().st_dev`).
    pub device_id: u64,
    /// Mount-point mtime in Unix seconds, when available.
    pub marker: Option<i64>,
}

impl MountIdentity {
    /// True when the stored identity no longer describes the mounted root.
    #[must_use]
    pub fn changed_since(&self, stored: Option<MountIdentity>) -> bool {
        match stored {
            Some(stored) => stored != *self,
            None => true,
        }
    }

    /// Compact `device:marker` form stored in the database.
    #[must_use]
    pub fn encode(&self) -> String {
        format!(
            "{}:{}",
            self.device_id,
            self.marker
                .map(|value| value.to_string())
                .unwrap_or_default()
        )
    }

    /// Parses [`MountIdentity::encode`] output. Malformed input is `None`, which
    /// forces a rescan rather than trusting a broken fingerprint.
    #[must_use]
    pub fn decode(raw: &str) -> Option<Self> {
        let (device, marker) = raw.split_once(':')?;
        let device_id = device.parse().ok()?;
        let marker = if marker.is_empty() {
            None
        } else {
            Some(marker.parse().ok()?)
        };
        Some(Self { device_id, marker })
    }
}

/// True when an offline-then-reconnected root may skip its walk.
///
/// Only a root that was offline and came back with the same identity reuses
/// its index: a root that stayed online always walks, so new archives are
/// indexed, and a changed mount is always rescanned.
#[must_use]
pub fn may_reuse_index(
    was_offline: bool,
    stored: Option<MountIdentity>,
    current: &MountIdentity,
) -> bool {
    was_offline && !current.changed_since(stored)
}

/// Bytes an install needs before its first byte: the archive plus 10 % for
/// extraction overhead and staging.
#[must_use]
pub fn preflight_requirement(archive_bytes: u64) -> u64 {
    let overhead = archive_bytes
        .saturating_mul(PREFLIGHT_OVERHEAD_PERCENT)
        .saturating_add(99)
        / 100;
    archive_bytes.saturating_add(overhead)
}

/// How far below the free-space floor a volume is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreeSpaceShortfall {
    /// Bytes missing for the install plus the floor to fit.
    pub missing_bytes: u64,
    /// The floor that applies to this library.
    pub min_free_bytes: u64,
    /// What the install itself needs.
    pub required_bytes: u64,
    /// What the filesystem reports.
    pub free_bytes: u64,
}

impl FreeSpaceShortfall {
    /// User-facing message naming the missing gigabytes.
    #[must_use]
    pub fn message(&self, where_: &Path) -> String {
        format!(
            "{} is below its free-space floor of {}: this install needs {} more free space ({} required including the floor, {} available). Free space or move the library to a larger volume, then resume.",
            where_.display(),
            human_gib(self.min_free_bytes),
            human_gib(self.missing_bytes),
            human_bytes(self.required_bytes.saturating_add(self.min_free_bytes)),
            human_bytes(self.free_bytes)
        )
    }
}

/// Checks `free_bytes` against an install requirement plus the library floor.
pub fn check_free_space(
    free_bytes: u64,
    required_bytes: u64,
    min_free_bytes: u64,
) -> Result<(), FreeSpaceShortfall> {
    let needed = required_bytes.saturating_add(min_free_bytes);
    if free_bytes >= needed {
        return Ok(());
    }
    Err(FreeSpaceShortfall {
        missing_bytes: needed - free_bytes,
        min_free_bytes,
        required_bytes,
        free_bytes,
    })
}

/// Formats bytes as GiB with one decimal (`5.0 GiB`).
#[must_use]
pub fn human_gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / GIB as f64)
}

/// Formats bytes the way the desktop and server messages already do.
#[must_use]
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Message a root shows when its mount is absent.
#[must_use]
pub fn offline_message(path: &Path, mount_point: Option<&str>) -> String {
    match mount_point {
        Some(mount) => format!(
            "The library at {} is offline: its mount {} is not mounted. Reconnect the drive or share, then resume the install. The indexed games were kept.",
            path.display(),
            mount
        ),
        None => format!(
            "The library at {} is offline: the drive or share is not mounted. Reconnect it, then resume the install. The indexed games were kept.",
            path.display()
        ),
    }
}

/// The OS surface [`ensure`] needs.
///
/// Production code implements it with `/proc/self/mountinfo`, `/etc/fstab`,
/// the systemd unit directory and `systemctl`/`mount`; tests inject fixtures so
/// no real mount, fstab or systemctl is involved.
pub trait SystemProbe {
    /// Contents of `/proc/self/mountinfo`.
    fn mountinfo(&self) -> Result<String, String>;
    /// Contents of `/etc/fstab` (empty on platforms without one).
    fn fstab(&self) -> String;
    /// Names of the `.automount` units available on the machine.
    fn automount_units(&self) -> Vec<String>;
    /// Triggers one plan: `systemctl start <unit>` or `mount <target>`.
    fn run_plan(&self, plan: &AutomountPlan) -> Result<(), String>;
    /// True when the path is an existing directory.
    fn path_exists(&self, path: &std::path::Path) -> bool;
    /// Device id + mount-point marker of the path's filesystem.
    fn identity(&self, path: &std::path::Path) -> Option<MountIdentity>;
}

/// State of a library root or install folder after the mount check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountStatus {
    /// The path can be used; `identity` fingerprints the mount.
    Online {
        /// Mount point serving the path.
        mount_point: String,
        /// Device id + mount-point marker.
        identity: MountIdentity,
    },
    /// The path must stay indexed and untouched; `message` is shown to users.
    Offline {
        /// Mount point that should have served the path, when known.
        mount_point: Option<String>,
        /// Human sentence naming the problem.
        message: String,
        /// True when a *configured mount* is absent, false when the folder is
        /// simply missing on a healthy mount. An I/O error is only treated as
        /// "the drive went away" in the first case.
        absent_mount: bool,
    },
}

impl MountStatus {
    /// True when the path can be used.
    #[must_use]
    pub fn is_online(&self) -> bool {
        matches!(self, Self::Online { .. })
    }

    /// Mount point, when one could be resolved.
    #[must_use]
    pub fn mount_point(&self) -> Option<&str> {
        match self {
            Self::Online { mount_point, .. } => Some(mount_point),
            Self::Offline { mount_point, .. } => mount_point.as_deref(),
        }
    }

    /// The offline message, when offline.
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Online { .. } => None,
            Self::Offline { message, .. } => Some(message),
        }
    }
}

/// Outcome of [`ensure`]: the state, the plan that was tried and its result.
#[derive(Debug, Clone)]
pub struct EnsuredMount {
    /// Final state after the optional automount attempt.
    pub status: MountStatus,
    /// Plan that was attempted, when the mount was absent.
    pub plan: Option<AutomountPlan>,
    /// Error of the failed attempt, for the root's `last_error` detail.
    pub attempt_error: Option<String>,
}

impl EnsuredMount {
    /// True when an automount was tried.
    #[must_use]
    pub fn attempted(&self) -> bool {
        self.plan.is_some()
    }
}

/// Resolves the mount backing `path` without side effects.
///
/// Used for listings (Settings → Locations) where triggering systemctl on every
/// refresh would be surprising. [`ensure`] adds the one-shot automount.
#[must_use]
pub fn status(probe: &dyn SystemProbe, path: &std::path::Path) -> MountStatus {
    let Ok(text) = probe.mountinfo() else {
        return if probe.path_exists(path) {
            MountStatus::Online {
                mount_point: path.display().to_string(),
                identity: probe.identity(path).unwrap_or(MountIdentity {
                    device_id: 0,
                    marker: None,
                }),
            }
        } else {
            MountStatus::Offline {
                mount_point: None,
                message: offline_message(path, None),
                absent_mount: false,
            }
        };
    };
    let mounts = parse_mountinfo(&text);
    let fstab = parse_fstab(&probe.fstab());
    let units = probe.automount_units();
    let configured = configured_mount_point(&fstab, &units, path);
    let mounted = select_mount(&mounts, path).cloned();
    let absent_configured = configured
        .as_deref()
        .filter(|mount| *mount != "/")
        .filter(|mount| !is_mounted(&mounts, mount));
    if absent_configured.is_none() && probe.path_exists(path) {
        mount_status(mounted.as_ref(), path, probe)
    } else {
        offline_status(
            path,
            mounted.as_ref(),
            &fstab,
            absent_configured.map(str::to_string).as_deref(),
            probe,
        )
    }
}

/// Resolves the mount backing `path`, trying an automount once when absent.
///
/// This is the one decision both the server scanner and the desktop install
/// pipeline use: an absent configured mount makes the path offline (never
/// scanned as empty), the matching automount is triggered once, and the mount
/// table is re-read before giving up.
#[must_use]
pub fn ensure(probe: &dyn SystemProbe, path: &std::path::Path) -> EnsuredMount {
    let mounts = match probe.mountinfo() {
        Ok(text) => parse_mountinfo(&text),
        Err(error) => {
            return if probe.path_exists(path) {
                EnsuredMount {
                    status: MountStatus::Online {
                        mount_point: path.display().to_string(),
                        identity: probe.identity(path).unwrap_or(MountIdentity {
                            device_id: 0,
                            marker: None,
                        }),
                    },
                    plan: None,
                    attempt_error: None,
                }
            } else {
                EnsuredMount {
                    status: MountStatus::Offline {
                        mount_point: None,
                        message: format!(
                            "The folder at {} is offline: {error} and the folder is missing. Reconnect the drive or share, then resume.",
                            path.display()
                        ),
                        absent_mount: false,
                    },
                    plan: None,
                    attempt_error: None,
                }
            };
        }
    };
    let fstab = parse_fstab(&probe.fstab());
    let units = probe.automount_units();
    let configured = configured_mount_point(&fstab, &units, path);
    let mounted = select_mount(&mounts, path).cloned();
    let absent_configured = configured
        .as_deref()
        .filter(|mount| *mount != "/")
        .filter(|mount| !is_mounted(&mounts, mount))
        .map(str::to_string);

    if absent_configured.is_none() && probe.path_exists(path) {
        return EnsuredMount {
            status: mount_status(mounted.as_ref(), path, probe),
            plan: None,
            attempt_error: None,
        };
    }

    // The mount is absent: try to wake it once.
    if let Some(plan) = automount_plan(&mounts, &fstab, &units, path) {
        let attempt_error = probe.run_plan(&plan).err();
        // The mount table may lag a moment behind `systemctl start`; re-read
        // once. The caller re-checks on the next scan if it is still absent.
        let remounted = probe
            .mountinfo()
            .ok()
            .map(|text| parse_mountinfo(&text))
            .unwrap_or_default();
        let mounted = select_mount(&remounted, path).cloned();
        let absent = configured
            .as_deref()
            .filter(|mount| *mount != "/")
            .filter(|mount| !is_mounted(&remounted, mount))
            .map(str::to_string);
        let status = if absent.is_none() && probe.path_exists(path) {
            mount_status(mounted.as_ref(), path, probe)
        } else {
            offline_status(path, mounted.as_ref(), &fstab, absent.as_deref(), probe)
        };
        return EnsuredMount {
            status,
            plan: Some(plan),
            attempt_error,
        };
    }

    EnsuredMount {
        status: offline_status(
            path,
            mounted.as_ref(),
            &fstab,
            absent_configured.as_deref(),
            probe,
        ),
        plan: None,
        attempt_error: None,
    }
}

fn mount_status(
    mounted: Option<&MountEntry>,
    path: &std::path::Path,
    probe: &dyn SystemProbe,
) -> MountStatus {
    let mount_point = mounted
        .map(|mount| mount.mount_point.clone())
        .unwrap_or_else(|| path.display().to_string());
    let identity = probe
        .identity(if mount_point == "/" {
            path
        } else {
            std::path::Path::new(&mount_point)
        })
        .or_else(|| probe.identity(path))
        .unwrap_or(MountIdentity {
            device_id: 0,
            marker: None,
        });
    MountStatus::Online {
        mount_point,
        identity,
    }
}

fn offline_status(
    path: &std::path::Path,
    mounted: Option<&MountEntry>,
    fstab: &[FstabEntry],
    absent_configured: Option<&str>,
    probe: &dyn SystemProbe,
) -> MountStatus {
    let mount_point = absent_configured.map(str::to_string).or_else(|| {
        mounted
            .filter(|mount| !mount.is_root_filesystem())
            .map(|mount| mount.mount_point.clone())
    });
    let message = if let Some(mount) = absent_configured {
        offline_message(path, Some(mount))
    } else if !probe.path_exists(path) {
        format!(
            "The folder at {} is offline: the folder is missing. Reconnect the drive or share, then resume. Indexed games were kept.",
            path.display()
        )
    } else {
        let flagged = fstab.iter().any(|entry| {
            entry.mount_point == path.display().to_string() && entry.is_automountable()
        });
        let hint = if flagged {
            " (the fstab entry is flagged, but the mount did not come up)"
        } else {
            ""
        };
        format!(
            "The folder at {} is offline: it is on the root filesystem while its mount is absent{hint}. Reconnect the drive or share, then resume.",
            path.display()
        )
    };
    MountStatus::Offline {
        mount_point,
        message,
        absent_mount: absent_configured.is_some(),
    }
}

/// True when the path currently resolves to a mounted, readable folder.
#[must_use]
pub fn is_online(probe: &dyn SystemProbe, path: &std::path::Path) -> bool {
    ensure(probe, path).status.is_online()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const MOUNTINFO: &str = "\
23 1 259:1 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw
45 23 0:35 / /mnt/nas rw,relatime shared:22 - nfs4 nas.local:/export rw,vers=4.2
46 23 8:17 / /media/usb rw,nosuid,nodev shared:5 - vfat /dev/sdb1 rw,fmask=0022
47 23 0:41 / /run/user/1000/gvfs rw,nosuid,nodev shared:9 - fuse.gvfsd-fuse gvfsd-fuse rw
48 23 259:5 / /mnt/games\\040archive rw,relatime shared:12 - ext4 /dev/sdc1 rw";

    fn mounts() -> Vec<MountEntry> {
        parse_mountinfo(MOUNTINFO)
    }

    /// The same machine before the NAS export is mounted: the fstab entry and
    /// the generated `.automount` unit exist, `/mnt/nas` does not.
    fn mounts_with_nas_absent() -> Vec<MountEntry> {
        parse_mountinfo(
            &MOUNTINFO
                .lines()
                .filter(|line| !line.contains("/mnt/nas "))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    #[test]
    fn mountinfo_parses_escapes_and_super_options() {
        let entries = mounts();
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[1].fs_type, "nfs4");
        assert_eq!(entries[1].source, "nas.local:/export");
        assert!(entries[1].has_option("vers=4.2"));
        assert!(entries[1].is_network());
        assert_eq!(entries[4].mount_point, "/mnt/games archive");
        assert!(!entries[4].is_network());
        assert_eq!(unescape_mount_field("/a\\040b\\011c"), "/a b\tc");
        // Non-ASCII paths arrive as UTF-8 bytes plus octal escapes for spaces.
        assert_eq!(
            unescape_mount_field("/mnt/Jeux\\040vid\\303\\251os"),
            "/mnt/Jeux vidéos"
        );
    }

    #[test]
    fn the_deepest_ancestor_mount_wins_on_component_boundaries() {
        let entries = mounts();
        let picked =
            select_mount(&entries, &PathBuf::from("/mnt/nas/games/Doom.zip")).expect("a mount");
        assert_eq!(picked.mount_point, "/mnt/nas");
        let usb = select_mount(&entries, &PathBuf::from("/media/usb")).expect("a mount");
        assert_eq!(usb.mount_point, "/media/usb");
        // `/mnt/nas2` is not inside `/mnt/nas`.
        let root = select_mount(&entries, &PathBuf::from("/mnt/nas2/x")).expect("a mount");
        assert!(root.is_root_filesystem());
        assert!(on_root_filesystem(&entries, &PathBuf::from("/srv/games")));
        assert!(!on_root_filesystem(&entries, &PathBuf::from("/mnt/nas/x")));
    }

    #[test]
    fn automount_prefers_a_matching_unit() {
        let entries = mounts_with_nas_absent();
        let fstab = parse_fstab(
            "# comment\nnas.local:/export /mnt/nas nfs4 _netdev,noauto 0 0\n/dev/sdb1 /media/usb vfat defaults 0 0\n",
        );
        assert_eq!(fstab.len(), 2);
        assert!(fstab[0].is_automountable());
        assert!(!fstab[1].is_automountable());
        let plan = automount_plan(
            &entries,
            &fstab,
            &["mnt-nas.automount".to_string()],
            &PathBuf::from("/mnt/nas/games"),
        )
        .expect("a plan");
        assert_eq!(plan.kind, AutomountKind::SystemdUnit);
        assert_eq!(plan.unit.as_deref(), Some("mnt-nas.automount"));
        assert_eq!(plan.mount_point, "/mnt/nas");
    }

    #[test]
    fn automount_falls_back_to_the_fstab_entry() {
        let entries = mounts_with_nas_absent();
        let fstab = parse_fstab("nas.local:/export /mnt/nas nfs4 _netdev 0 0\n");
        let plan =
            automount_plan(&entries, &fstab, &[], &PathBuf::from("/mnt/nas")).expect("a plan");
        assert_eq!(plan.kind, AutomountKind::FstabMount);
        assert_eq!(plan.mount_point, "/mnt/nas");
        assert!(plan.describe().contains("/mnt/nas"));
    }

    #[test]
    fn no_automount_without_a_unit_or_a_flagged_entry() {
        let entries = mounts_with_nas_absent();
        let fstab = parse_fstab("/dev/sdb1 /media/usb vfat defaults 0 0\n");
        assert!(automount_plan(&entries, &fstab, &[], &PathBuf::from("/media/usb")).is_none());
        // Already mounted: nothing to trigger.
        let mounted_fstab = parse_fstab("nas.local:/export /mnt/nas nfs4 _netdev 0 0\n");
        assert!(
            automount_plan(&mounts(), &mounted_fstab, &[], &PathBuf::from("/mnt/nas")).is_none()
        );
    }

    #[test]
    fn unit_names_round_trip_through_the_escape() {
        assert_eq!(systemd_escape_mount_point("/mnt/nas"), "mnt-nas");
        assert_eq!(systemd_escape_mount_point("/"), "-");
        assert_eq!(
            systemd_escape_mount_point("/srv/my-disk"),
            "srv-my\\x2ddisk"
        );
        let unit = "srv-my\\x2ddisk.automount";
        assert_eq!(mount_point_from_unit(unit).as_deref(), Some("/srv/my-disk"));
        assert_eq!(mount_point_from_unit("-.automount").as_deref(), Some("/"));
        assert!(mount_point_from_unit("foo@.automount").is_none());
    }

    #[test]
    fn generated_units_are_discovered_without_fstab() {
        let entries = mounts_with_nas_absent();
        let plan = automount_plan(
            &entries,
            &[],
            &["mnt-nas.automount".to_string()],
            &PathBuf::from("/mnt/nas"),
        )
        .expect("a plan");
        assert_eq!(plan.kind, AutomountKind::SystemdUnit);
        assert_eq!(
            configured_mount_point(
                &[],
                &["mnt-nas.automount".to_string()],
                Path::new("/mnt/nas/a")
            )
            .as_deref(),
            Some("/mnt/nas")
        );
    }

    #[test]
    fn identities_compare_and_round_trip() {
        let identity = MountIdentity {
            device_id: 259,
            marker: Some(1_700_000_000),
        };
        assert_eq!(MountIdentity::decode(&identity.encode()), Some(identity));
        assert!(identity.changed_since(None));
        assert!(!identity.changed_since(Some(identity)));
        assert!(MountIdentity::decode("broken").is_none());
        assert!(may_reuse_index(true, Some(identity), &identity));
        assert!(!may_reuse_index(false, Some(identity), &identity));
        let moved = MountIdentity {
            device_id: 260,
            marker: Some(1_700_000_000),
        };
        assert!(!may_reuse_index(true, Some(identity), &moved));
    }

    #[test]
    fn the_floor_and_the_preflight_estimate_add_up() {
        let archive = 10 * GIB;
        assert_eq!(preflight_requirement(archive), 11 * GIB);
        assert_eq!(preflight_requirement(0), 0);
        assert_eq!(preflight_requirement(1), 2);

        let required = preflight_requirement(archive);
        assert!(check_free_space(
            required + DEFAULT_MIN_FREE_BYTES,
            required,
            DEFAULT_MIN_FREE_BYTES
        )
        .is_ok());
        let shortfall = check_free_space(required, required, DEFAULT_MIN_FREE_BYTES).unwrap_err();
        assert_eq!(shortfall.missing_bytes, DEFAULT_MIN_FREE_BYTES);
        let message = shortfall.message(Path::new("/mnt/nas"));
        assert!(message.contains("5.0 GiB"));
        assert!(message.contains("below its free-space floor"));
        assert_eq!(human_gib(5 * GIB), "5.0 GiB");
        assert_eq!(human_bytes(1536), "1.5 KiB");
    }

    /// Injected probe: fixture text, a fake systemctl and a fake mount table
    /// that `run_plan` rewrites the way a real automount would.
    pub(super) struct FixtureProbe {
        pub(super) mountinfo: std::cell::RefCell<String>,
        pub(super) fstab: String,
        pub(super) units: Vec<String>,
        pub(super) existing: Vec<PathBuf>,
        pub(super) runs: std::cell::RefCell<Vec<String>>,
        /// Mountinfo to install after a successful plan.
        pub(super) mounted_after: Option<String>,
        pub(super) fail_run: bool,
    }

    impl FixtureProbe {
        pub(super) fn new(mountinfo: &str, fstab: &str, units: &[&str]) -> Self {
            Self {
                mountinfo: std::cell::RefCell::new(mountinfo.to_string()),
                fstab: fstab.to_string(),
                units: units.iter().map(|unit| (*unit).to_string()).collect(),
                existing: Vec::new(),
                runs: std::cell::RefCell::new(Vec::new()),
                mounted_after: None,
                fail_run: false,
            }
        }

        pub(super) fn with_dir(mut self, path: &str) -> Self {
            self.existing.push(PathBuf::from(path));
            self
        }

        pub(super) fn with_mount_after(mut self, mountinfo: &str) -> Self {
            self.mounted_after = Some(mountinfo.to_string());
            self
        }

        pub(super) fn runs(&self) -> Vec<String> {
            self.runs.borrow().clone()
        }
    }

    impl SystemProbe for FixtureProbe {
        fn mountinfo(&self) -> Result<String, String> {
            Ok(self.mountinfo.borrow().clone())
        }

        fn fstab(&self) -> String {
            self.fstab.clone()
        }

        fn automount_units(&self) -> Vec<String> {
            self.units.clone()
        }

        fn run_plan(&self, plan: &AutomountPlan) -> Result<(), String> {
            self.runs.borrow_mut().push(
                plan.unit
                    .clone()
                    .unwrap_or_else(|| plan.mount_point.clone()),
            );
            if self.fail_run {
                return Err("systemctl: unit not found".to_string());
            }
            if let Some(text) = &self.mounted_after {
                *self.mountinfo.borrow_mut() = text.clone();
            }
            Ok(())
        }

        fn path_exists(&self, path: &std::path::Path) -> bool {
            self.existing.iter().any(|known| known == path)
        }

        fn identity(&self, path: &std::path::Path) -> Option<MountIdentity> {
            self.existing
                .iter()
                .position(|known| known == path)
                .map(|index| MountIdentity {
                    device_id: 100 + index as u64,
                    marker: Some(1_700_000_000),
                })
        }
    }

    const ROOT_ONLY: &str = "23 1 259:1 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw";
    const WITH_NAS: &str = "23 1 259:1 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw
45 23 0:35 / /mnt/nas rw,relatime shared:22 - nfs4 nas.local:/export rw,vers=4.2";

    #[test]
    fn an_absent_mount_with_an_automount_unit_comes_online() {
        let probe = FixtureProbe::new(
            ROOT_ONLY,
            "nas.local:/export /mnt/nas nfs4 _netdev 0 0\n",
            &["mnt-nas.automount"],
        )
        .with_dir("/mnt/nas")
        .with_mount_after(WITH_NAS);
        let ensured = ensure(&probe, Path::new("/mnt/nas"));
        assert!(ensured.status.is_online(), "{ensured:?}");
        assert_eq!(
            ensured.plan.expect("a plan").unit.as_deref(),
            Some("mnt-nas.automount")
        );
        assert_eq!(probe.runs(), vec!["mnt-nas.automount".to_string()]);
        assert_eq!(ensured.status.mount_point(), Some("/mnt/nas"));
        match ensured.status {
            MountStatus::Online { identity, .. } => assert_eq!(identity.device_id, 100),
            MountStatus::Offline { .. } => panic!("expected online"),
        }
    }

    #[test]
    fn an_absent_mount_without_a_unit_uses_fstab() {
        let probe = FixtureProbe::new(
            ROOT_ONLY,
            "/dev/sdb1 /media/ssd ext4 _netdev,noauto 0 0\n",
            &[],
        )
        .with_dir("/media/ssd")
        .with_mount_after(
            "23 1 259:1 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw
46 23 8:17 / /media/ssd rw,relatime shared:5 - ext4 /dev/sdb1 rw",
        );
        let ensured = ensure(&probe, Path::new("/media/ssd"));
        assert!(ensured.status.is_online());
        assert_eq!(probe.runs(), vec!["/media/ssd".to_string()]);
        assert_eq!(
            ensured.plan.expect("a plan").kind,
            AutomountKind::FstabMount
        );
    }

    #[test]
    fn an_empty_mount_point_on_the_root_filesystem_stays_offline() {
        // The folder exists (mount point) but the NFS export is not mounted:
        // scanning it would see an empty library and flag every game missing.
        let probe = FixtureProbe::new(
            ROOT_ONLY,
            "nas.local:/export /mnt/nas nfs4 _netdev 0 0\n",
            &[],
        )
        .with_dir("/mnt/nas");
        let ensured = ensure(&probe, Path::new("/mnt/nas"));
        assert!(!ensured.status.is_online());
        // The fstab entry was flagged, so one attempt was made and failed.
        assert!(ensured.attempted());
        let message = ensured.status.message().expect("offline message");
        assert!(message.contains("/mnt/nas"));
        assert!(message.contains("were kept"));
    }

    #[test]
    fn a_missing_folder_on_a_healthy_mount_is_offline() {
        let probe = FixtureProbe::new(WITH_NAS, "", &[]);
        let ensured = ensure(&probe, Path::new("/mnt/nas/games"));
        assert!(!ensured.status.is_online());
        assert_eq!(ensured.status.mount_point(), Some("/mnt/nas"));
        assert!(probe.runs().is_empty());
    }

    #[test]
    fn a_plain_directory_on_the_root_filesystem_is_online() {
        let probe = FixtureProbe::new(ROOT_ONLY, "", &[]).with_dir("/srv/games");
        let ensured = ensure(&probe, Path::new("/srv/games"));
        assert!(ensured.status.is_online());
        assert_eq!(ensured.status.mount_point(), Some("/"));
    }

    #[test]
    fn a_failed_automount_keeps_the_attempt_error_and_offline_state() {
        let mut probe = FixtureProbe::new(
            ROOT_ONLY,
            "nas.local:/export /mnt/nas nfs4 _netdev 0 0\n",
            &[],
        )
        .with_dir("/mnt/nas");
        probe.fail_run = true;
        let ensured = ensure(&probe, Path::new("/mnt/nas"));
        assert!(!ensured.status.is_online());
        assert!(ensured
            .attempt_error
            .as_deref()
            .is_some_and(|error| error.contains("unit not found")));
        assert!(ensured.attempted());
    }

    #[test]
    fn is_online_reports_the_same_decision() {
        let online = FixtureProbe::new(WITH_NAS, "", &[]).with_dir("/mnt/nas");
        assert!(is_online(&online, Path::new("/mnt/nas")));
        let offline = FixtureProbe::new(
            ROOT_ONLY,
            "nas.local:/export /mnt/nas nfs4 _netdev 0 0\n",
            &[],
        )
        .with_dir("/mnt/nas");
        assert!(!is_online(&offline, Path::new("/mnt/nas")));
    }

    #[test]
    fn the_offline_message_names_the_mount_and_keeps_the_index() {
        let message = offline_message(Path::new("/mnt/nas/games"), Some("/mnt/nas"));
        assert!(message.contains("/mnt/nas/games"));
        assert!(message.contains("offline"));
        assert!(message.contains("were kept"));
        assert!(offline_message(Path::new("/media/usb"), None).contains("/media/usb"));
    }
}
