//! Which kind of Linux install is running (#312).
//!
//! The Tauri updater decides how to install from a value compiled into the
//! binary at bundle time: a build produced for the `.rpm` bundle keeps calling
//! `rpm` even when the file was hand-placed in `/opt/ludex` on a machine with
//! no `rpm` at all. The plugin then reports its generic
//! "Failed to install package" and nothing explains why.
//!
//! This module answers the question at *update time*, from the running system:
//!
//! - `$APPIMAGE` set: the file the updater must replace is the AppImage in the
//!   mount's environment; the plugin already handles it.
//! - the executable belongs to a `dpkg` or `rpm` package: the package manager
//!   owns the file. Ludex lets the plugin try (it knows the deb/rpm flow) and
//!   names the package and the upgrade command in the error if it cannot.
//! - anything else (a bare ELF, a hand-extracted rpm/deb payload): a *
//!   portable* install. Ludex replaces the executable itself, atomically, and
//!   says exactly what blocked it when the folder is not writable.
//!
//! `install.sh`'s record (`~/.local/share/ludex/install.json`) is used to
//! resolve the real file behind a wrapper or a symlink; it never overrides the
//! package database, and a stale record from another prefix is ignored.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Package manager that owns a system install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Dpkg,
    Rpm,
}

impl PackageManager {
    /// Short name used in logs and messages.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dpkg => "dpkg",
            Self::Rpm => "rpm",
        }
    }

    /// Command a user can run to upgrade the package themselves.
    #[must_use]
    pub fn upgrade_command(self, package: &str) -> String {
        match self {
            Self::Dpkg => format!("sudo apt install --only-upgrade {package}"),
            Self::Rpm => format!("sudo dnf upgrade {package}"),
        }
    }
}

/// A package that owns the running executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageOwner {
    pub manager: PackageManager,
    pub package: String,
}

impl PackageOwner {
    /// Sentence appended to an updater failure: names the package, the manager
    /// and the command that actually upgrades it.
    #[must_use]
    pub fn hint(&self) -> String {
        format!(
            "This Ludex belongs to the {} package {}, so the update has to go through it: run `{}`, then start Ludex again.",
            self.manager.as_str(),
            self.package,
            self.manager.upgrade_command(&self.package)
        )
    }
}

/// How the running Ludex is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// Running from an AppImage (`$APPIMAGE`).
    AppImage { image: PathBuf },
    /// The executable belongs to a dpkg/rpm package.
    PackageManaged { owner: PackageOwner, exe: PathBuf },
    /// A plain executable Ludex can replace itself.
    Portable { target: PathBuf },
    /// Windows NSIS/MSI or a macOS bundle: the plugin installs it.
    Bundled,
}

impl InstallKind {
    /// Stable label for logs and the update attempt summary.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AppImage { .. } => "appimage",
            Self::PackageManaged { owner, .. } => owner.manager.as_str(),
            Self::Portable { .. } => "portable",
            Self::Bundled => "bundled",
        }
    }
}

/// Everything detection reads, split out so it can be tested without the
/// machine it runs on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallFacts {
    /// `$APPIMAGE`, when set and non-empty.
    pub appimage: Option<PathBuf>,
    /// The running executable, symlinks already resolved.
    pub exe: Option<PathBuf>,
    /// Package that owns the executable, from `dpkg-query`/`rpm`.
    pub owner: Option<PackageOwner>,
    /// `install.sh`'s record, when it exists and parses.
    pub record: Option<InstallRecord>,
}

/// `~/.local/share/ludex/install.json`, written by `install.sh`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallRecord {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    /// `appimage` or `deb`.
    #[serde(default)]
    pub method: Option<String>,
    /// Install prefix (`/opt/ludex` with `--dir`).
    #[serde(default)]
    pub prefix: Option<String>,
    /// Every file the installer wrote.
    #[serde(default)]
    pub files: Vec<String>,
}

impl InstallRecord {
    /// The executable among the recorded files.
    #[must_use]
    pub fn binary(&self) -> Option<&Path> {
        self.files.iter().map(Path::new).find(|path| {
            path.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name == "ludex" || name.ends_with(".AppImage"))
        })
    }

    /// True when the record covers `exe`, either by listing it or because it
    /// lives under the recorded prefix.
    #[must_use]
    pub fn owns(&self, exe: &Path) -> bool {
        if self
            .files
            .iter()
            .any(|file| same_file(Path::new(file), exe))
        {
            return true;
        }
        self.prefix
            .as_deref()
            .filter(|prefix| !prefix.is_empty())
            .is_some_and(|prefix| exe.starts_with(prefix))
    }
}

/// Decide the install kind from what was observed.
///
/// `None` means the running executable could not be determined; the caller
/// keeps the plugin path, which reports its own error.
#[must_use]
pub fn detect_kind(facts: &InstallFacts) -> Option<InstallKind> {
    if let Some(image) = facts
        .appimage
        .as_ref()
        .filter(|path| !path.as_os_str().is_empty())
    {
        return Some(InstallKind::AppImage {
            image: image.clone(),
        });
    }
    let exe = facts.exe.as_ref()?;
    if let Some(owner) = &facts.owner {
        return Some(InstallKind::PackageManaged {
            owner: owner.clone(),
            exe: exe.clone(),
        });
    }
    Some(InstallKind::Portable {
        target: portable_target(exe, facts.record.as_ref()),
    })
}

/// File a portable update replaces: the recorded binary when the record owns
/// the running executable, the executable itself otherwise.
#[must_use]
pub fn portable_target(exe: &Path, record: Option<&InstallRecord>) -> PathBuf {
    let Some(record) = record.filter(|record| record.owns(exe)) else {
        return exe.to_path_buf();
    };
    match record.binary() {
        Some(binary) if same_file(binary, exe) => binary.to_path_buf(),
        _ => exe.to_path_buf(),
    }
}

/// `install.sh`'s record path from the XDG variables.
#[must_use]
pub fn record_path_from(xdg_data_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    let base = xdg_data_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".local").join("share"))
        })?;
    Some(base.join("ludex").join("install.json"))
}

/// Record path of this session.
#[must_use]
pub fn record_path() -> Option<PathBuf> {
    record_path_from(
        std::env::var_os("XDG_DATA_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// Read and parse an install record. A missing or malformed file is `None`:
/// the record is a hint, never a requirement.
#[must_use]
pub fn read_record(path: &Path) -> Option<InstallRecord> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Observe the running machine.
#[must_use]
pub fn probe() -> InstallFacts {
    let appimage = std::env::var_os("APPIMAGE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let exe = std::env::current_exe().ok().map(|path| {
        // `/usr/bin/ludex` is usually a symlink into `/opt/ludex`; the updater
        // must replace the file at the end of the link, not the link.
        let resolved = std::fs::canonicalize(&path).unwrap_or(path);
        crate::paths::normalize_native_path(&resolved)
    });
    let record = record_path().as_deref().and_then(read_record);
    #[cfg(target_os = "linux")]
    let owner = if appimage.is_none() {
        exe.as_deref().and_then(package_owner)
    } else {
        None
    };
    #[cfg(not(target_os = "linux"))]
    let owner = None;
    InstallFacts {
        appimage,
        exe,
        owner,
        record,
    }
}

/// Detect the install kind on this machine.
///
/// `Bundled` is the fallback: when the running executable cannot be read, the
/// plugin's own install flow (and its error) is still better than doing
/// nothing.
#[must_use]
pub fn detect() -> InstallKind {
    #[cfg(not(target_os = "linux"))]
    {
        InstallKind::Bundled
    }
    #[cfg(target_os = "linux")]
    {
        let facts = probe();
        let kind = detect_kind(&facts);
        tracing::info!(
            kind = kind.as_ref().map_or("unknown", InstallKind::as_str),
            appimage = facts
                .appimage
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            exe = facts
                .exe
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            package = facts
                .owner
                .as_ref()
                .map(|owner| owner.package.as_str())
                .unwrap_or("none"),
            record = facts
                .record
                .as_ref()
                .map(|record| record.method.as_deref().unwrap_or("unknown"))
                .unwrap_or("none"),
            "install kind detected"
        );
        kind.unwrap_or(InstallKind::Bundled)
    }
}

/// Package that owns `exe`, asked from the package database (Linux only).
#[cfg(target_os = "linux")]
#[must_use]
pub fn package_owner(exe: &Path) -> Option<PackageOwner> {
    query_dpkg(exe).or_else(|| query_rpm(exe))
}

#[cfg(target_os = "linux")]
fn query_dpkg(exe: &Path) -> Option<PackageOwner> {
    let output = std::process::Command::new("dpkg-query")
        .arg("-S")
        .arg(exe)
        .output()
        .or_else(|_| {
            std::process::Command::new("dpkg")
                .arg("-S")
                .arg(exe)
                .output()
        })
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let expected = exe.to_string_lossy();
    text.lines().find_map(|line| {
        // "ludex: /usr/bin/ludex" or "ludex, ludex-bin: /opt/ludex/ludex".
        let (packages, path) = line.split_once(": ")?;
        if path.trim() != expected {
            return None;
        }
        let package = packages.split(',').next()?.trim();
        if package.is_empty() || package.starts_with("diversion") {
            return None;
        }
        Some(PackageOwner {
            manager: PackageManager::Dpkg,
            package: package.to_string(),
        })
    })
}

#[cfg(target_os = "linux")]
fn query_rpm(exe: &Path) -> Option<PackageOwner> {
    let output = std::process::Command::new("rpm")
        .arg("-qf")
        .arg(exe)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let package = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if package.is_empty() || package.contains("not owned by any package") {
        return None;
    }
    Some(PackageOwner {
        manager: PackageManager::Rpm,
        package,
    })
}

/// Same file on disk, following symlinks when both resolve.
fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(method: &str, prefix: &str, files: &[&str]) -> InstallRecord {
        InstallRecord {
            version: Some("0.4.24".to_string()),
            channel: Some("stable".to_string()),
            method: Some(method.to_string()),
            prefix: Some(prefix.to_string()),
            files: files.iter().map(|file| (*file).to_string()).collect(),
        }
    }

    fn facts(
        appimage: Option<&str>,
        exe: Option<&str>,
        owner: Option<PackageOwner>,
    ) -> InstallFacts {
        InstallFacts {
            appimage: appimage.map(PathBuf::from),
            exe: exe.map(PathBuf::from),
            owner,
            record: None,
        }
    }

    #[test]
    fn appimage_wins_over_every_other_signal() {
        let observed = InstallFacts {
            owner: Some(PackageOwner {
                manager: PackageManager::Rpm,
                package: "ludex".to_string(),
            }),
            ..facts(
                Some("/opt/ludex/ludex.AppImage"),
                Some("/opt/ludex/ludex.AppImage"),
                None,
            )
        };
        assert_eq!(
            detect_kind(&observed),
            Some(InstallKind::AppImage {
                image: PathBuf::from("/opt/ludex/ludex.AppImage")
            })
        );
        // An empty APPIMAGE (an exported but unset shell variable) is no signal.
        assert!(matches!(
            detect_kind(&facts(None, Some("/opt/ludex/ludex"), None)),
            Some(InstallKind::Portable { .. })
        ));
        assert!(detect_kind(&facts(
            None,
            Some("/opt/ludex/ludex"),
            Some(PackageOwner {
                manager: PackageManager::Dpkg,
                package: "ludex".to_string()
            })
        ))
        .is_some_and(|kind| kind.as_str() == "dpkg"));
    }

    #[test]
    fn the_package_database_marks_a_managed_install() {
        let owner = PackageOwner {
            manager: PackageManager::Rpm,
            package: "ludex-0.4.24-1.x86_64".to_string(),
        };
        let kind = detect_kind(&facts(None, Some("/usr/bin/ludex"), Some(owner.clone())))
            .expect("a managed install is still an install");
        assert_eq!(kind.as_str(), "rpm");
        let expected = InstallKind::PackageManaged {
            owner: owner.clone(),
            exe: PathBuf::from("/usr/bin/ludex"),
        };
        assert_eq!(kind, expected);
        let hint = owner.hint();
        assert!(hint.contains("rpm"), "{hint}");
        assert!(hint.contains("dnf upgrade"), "{hint}");
        assert!(hint.contains("ludex-0.4.24-1.x86_64"), "{hint}");
        assert!(PackageManager::Dpkg
            .upgrade_command("ludex")
            .contains("apt install --only-upgrade ludex"));
    }

    #[test]
    fn a_bare_elf_is_portable() {
        // The exact shape of #312: `/usr/bin/ludex` -> `/opt/ludex/ludex`,
        // no APPIMAGE, no dpkg/rpm owner, no install record.
        let kind = detect_kind(&facts(None, Some("/opt/ludex/ludex"), None))
            .expect("the executable is known");
        assert_eq!(
            kind,
            InstallKind::Portable {
                target: PathBuf::from("/opt/ludex/ludex")
            }
        );
        assert_eq!(kind.as_str(), "portable");
        // Without an executable there is nothing to decide.
        assert_eq!(detect_kind(&facts(None, None, None)), None);
    }

    #[test]
    fn the_install_record_only_redirects_to_the_same_file() {
        let recorded = record(
            "appimage",
            "/opt/ludex",
            &["/opt/ludex/ludex.AppImage", "/opt/ludex/ludex"],
        );
        let exe = Path::new("/opt/ludex/ludex");
        assert!(recorded.owns(exe));
        assert_eq!(
            recorded.binary(),
            Some(Path::new("/opt/ludex/ludex.AppImage"))
        );
        // The recorded binary is a different file here, so the running file is
        // the one replaced: the record never points the update elsewhere.
        assert_eq!(portable_target(exe, Some(&recorded)), exe);
        // A record for another prefix does not claim this executable.
        let stale = record("appimage", "/home/x/.local", &["/home/x/.local/bin/ludex"]);
        assert!(!stale.owns(exe));
        assert_eq!(portable_target(exe, Some(&stale)), exe);
        // No record at all is the common portable case.
        assert_eq!(portable_target(exe, None), exe);
    }

    #[test]
    fn the_record_path_follows_xdg() {
        assert_eq!(
            record_path_from(Some(OsStr::new("/data")), Some(OsStr::new("/home/x"))),
            Some(PathBuf::from("/data/ludex/install.json"))
        );
        assert_eq!(
            record_path_from(None, Some(OsStr::new("/home/x"))),
            Some(PathBuf::from("/home/x/.local/share/ludex/install.json"))
        );
        // An empty XDG_DATA_HOME falls back to HOME, an empty HOME is unknown.
        assert_eq!(
            record_path_from(Some(OsStr::new("")), Some(OsStr::new("/home/x"))),
            Some(PathBuf::from("/home/x/.local/share/ludex/install.json"))
        );
        assert_eq!(record_path_from(None, None), None);
    }

    #[test]
    fn a_record_is_schema_tolerant() {
        let parsed: InstallRecord = serde_json::from_str(
            r#"{"version":"0.4.24","channel":"stable","method":"deb","prefix":"/usr","files":[]}"#,
        )
        .unwrap();
        assert_eq!(parsed.method.as_deref(), Some("deb"));
        assert!(parsed.binary().is_none());
        // install.sh writes exactly this shape.
        let parsed: InstallRecord = serde_json::from_str(
            r#"{"version":"0.4.24","channel":"stable","method":"appimage","prefix":"/home/x/.local","installedAt":"2026-11-19T00:00:00Z","files":["/home/x/.local/bin/ludex","/home/x/.local/share/applications/ludex.desktop"]}"#,
        )
        .unwrap();
        assert_eq!(parsed.binary(), Some(Path::new("/home/x/.local/bin/ludex")));
        assert!(parsed.owns(Path::new("/home/x/.local/bin/ludex")));
        assert!(!parsed.owns(Path::new("/opt/ludex/ludex")));
        // Unknown fields and a missing method do not break detection.
        let parsed: InstallRecord = serde_json::from_str(r#"{"files":[]}"#).unwrap();
        assert_eq!(parsed.method, None);
        assert!(parsed.binary().is_none());
    }
}
