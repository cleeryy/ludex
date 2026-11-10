//! Steam / Proton / UMU / Wine compatibility tool detection (#90).
//!
//! Nothing here is required at build time: every path is optional and a missing
//! tool is reported in [`CompatScan`] instead of failing. Detection is split
//! from discovery ([`ScanPaths`]) so fixture directory trees can be scanned in
//! tests without a real Steam installation.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::linux_gaming::{self, LaunchMode};
use crate::settings::LocalSettings;

/// What a detected tool is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    /// `umu-run` from umu-launcher.
    Umu,
    /// A Proton directory (`proton` script + `files/`).
    Proton,
    /// A system Wine binary.
    Wine,
}

impl ToolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolKind::Umu => "umu",
            ToolKind::Proton => "proton",
            ToolKind::Wine => "wine",
        }
    }
}

/// One detected compatibility tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompatTool {
    /// Stable id used by per-game launch options (`kind:path`).
    pub id: String,
    /// Human name: `GE-Proton9-20`, `Proton - Experimental`, `umu-run`, `wine`.
    pub name: String,
    pub kind: ToolKind,
    /// Version from the tool's `version` file, when it ships one.
    pub version: Option<String>,
    /// Executable: `umu-run`, the `proton` script or the `wine` binary.
    pub path: String,
    /// Proton directory (the folder containing the `proton` script).
    pub proton_path: Option<String>,
    /// Where the tool was found: `path`, `steam`, `compatibilitytools.d`,
    /// `override`.
    pub source: String,
    /// Steam library root that owns the tool, when it came from Steam.
    pub steam_root: Option<String>,
}

/// Manual overrides from Settings → Compatibility.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompatOverrides {
    /// `umu-run` executable or its folder.
    pub umu: String,
    /// Proton directory, or the `proton` script inside it.
    pub proton: String,
    /// `wine` executable.
    pub wine: String,
}

impl CompatOverrides {
    pub fn from_settings(settings: &LocalSettings) -> Self {
        Self {
            umu: settings.compat_umu_path.clone(),
            proton: settings.compat_proton_path.clone(),
            wine: settings.compat_wine_path.clone(),
        }
    }
}

/// Everything detection looks at. Built from the environment in [`scan`] and
/// from fixture trees in tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanPaths {
    pub home: Option<PathBuf>,
    pub steam_roots: Vec<PathBuf>,
    /// Directories searched for plain executables (`umu-run`, `wine`,
    /// `winetricks`), like `PATH`.
    pub path_dirs: Vec<PathBuf>,
    pub overrides: CompatOverrides,
}

/// Detection result, serialised for `compat_scan`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompatScan {
    pub platform: String,
    /// True on Linux, where the compatibility panel is relevant.
    pub linux: bool,
    pub steam_roots: Vec<String>,
    /// UMU first, then Proton (GE-Proton, Experimental, stable), then Wine.
    pub tools: Vec<CompatTool>,
    pub umu: Option<String>,
    pub proton: Option<String>,
    pub wine: Option<String>,
    pub winetricks: Option<String>,
    /// A `protonfixes` checkout or package, when one is visible.
    pub protonfixes: Option<String>,
    /// Launch mode new games get by default.
    pub default_mode: LaunchMode,
    /// `STEAM_COMPAT_CLIENT_INSTALL_PATH` used for Proton/UMU launches.
    pub steam_client_path: Option<String>,
    pub suggested_verbs: Vec<String>,
    pub warnings: Vec<String>,
}

/// Environment-based detection, using the configured Steam root override.
pub fn scan(settings: &LocalSettings) -> CompatScan {
    let home = home_dir();
    let steam_roots = crate::steam::discover_roots(Some(settings.steam_root.as_str()));
    let path_dirs = std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default();

    let paths = ScanPaths {
        home,
        steam_roots,
        path_dirs,
        overrides: CompatOverrides::from_settings(settings),
    };
    let mut result = scan_with(&paths);
    result.default_mode = settings.default_launch_mode;
    result
}

/// Detect tools in an explicit set of locations.
pub fn scan_with(paths: &ScanPaths) -> CompatScan {
    let mut scan = CompatScan {
        platform: std::env::consts::OS.to_string(),
        linux: cfg!(all(unix, not(target_os = "macos"))),
        steam_roots: paths
            .steam_roots
            .iter()
            .map(|root| root.to_string_lossy().to_string())
            .collect(),
        tools: Vec::new(),
        umu: None,
        proton: None,
        wine: None,
        winetricks: None,
        protonfixes: None,
        default_mode: LaunchMode::Auto,
        steam_client_path: paths
            .steam_roots
            .first()
            .map(|root| root.to_string_lossy().to_string()),
        suggested_verbs: linux_gaming::suggested_verbs(),
        warnings: Vec::new(),
    };

    let mut seen: BTreeSet<(ToolKind, String)> = BTreeSet::new();
    let mut has_umu = false;
    let mut has_wine = false;

    // 1. Manual overrides win: the user pointed at this exact binary/folder.
    if let Some(path) = resolve_override(&paths.overrides.umu, "umu-run", None) {
        add_umu(&mut scan, &mut seen, path, "override");
        has_umu = true;
    } else if !paths.overrides.umu.trim().is_empty() {
        scan.warnings.push(format!(
            "The configured UMU path does not exist: {}",
            paths.overrides.umu.trim()
        ));
    }
    if let Some(proton) = resolve_proton_override(&paths.overrides.proton) {
        add_proton(&mut scan, &mut seen, &proton, "override", None);
    } else if !paths.overrides.proton.trim().is_empty() {
        scan.warnings.push(format!(
            "The configured Proton path is not a Proton folder: {}",
            paths.overrides.proton.trim()
        ));
    }
    if let Some(wine) = resolve_override(&paths.overrides.wine, "wine", Some("wine64")) {
        add_wine(&mut scan, &mut seen, wine, "override");
        has_wine = true;
    } else if !paths.overrides.wine.trim().is_empty() {
        scan.warnings.push(format!(
            "The configured Wine path does not exist: {}",
            paths.overrides.wine.trim()
        ));
    }

    // 2. `umu-run` from PATH and the usual per-user/system locations.
    if !has_umu {
        if let Some(path) = find_in_dirs(&paths.path_dirs, "umu-run").or_else(|| {
            let candidates = [
                paths
                    .home
                    .as_ref()
                    .map(|home| home.join(".local/bin/umu-run")),
                paths
                    .home
                    .as_ref()
                    .map(|home| home.join(".local/share/umu/umu-run")),
                Some(PathBuf::from("/usr/local/bin/umu-run")),
                Some(PathBuf::from("/usr/bin/umu-run")),
            ];
            first_file(candidates)
        }) {
            add_umu(&mut scan, &mut seen, path, "path");
        }
    }

    // 3. Proton installations: compatibilitytools.d and steamapps/common.
    for root in &paths.steam_roots {
        let root_label = Some(root.to_string_lossy().to_string());
        let compat_dir = root.join("compatibilitytools.d");
        for dir in child_dirs(&compat_dir) {
            if dir.join("proton").is_file() {
                add_proton(
                    &mut scan,
                    &mut seen,
                    &dir,
                    "compatibilitytools.d",
                    root_label.clone(),
                );
            }
        }
        let common = root.join("steamapps/common");
        for dir in child_dirs(&common) {
            let name = dir
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if (name.contains("proton") || name.starts_with("ge-")) && dir.join("proton").is_file()
            {
                add_proton(&mut scan, &mut seen, &dir, "steam", root_label.clone());
            }
        }
    }

    // 4. System Wine.
    if !has_wine {
        if let Some(path) = find_in_dirs(&paths.path_dirs, "wine")
            .or_else(|| find_in_dirs(&paths.path_dirs, "wine64"))
            .or_else(|| {
                first_file([
                    Some(PathBuf::from("/usr/bin/wine")),
                    Some(PathBuf::from("/usr/local/bin/wine")),
                    paths.home.as_ref().map(|home| home.join(".local/bin/wine")),
                ])
            })
        {
            add_wine(&mut scan, &mut seen, path, "path");
        }
    }

    // 5. Winetricks and protonfixes are optional add-ons, not tools.
    scan.winetricks = find_in_dirs(&paths.path_dirs, "winetricks")
        .or_else(|| {
            first_file([
                Some(PathBuf::from("/usr/bin/winetricks")),
                Some(PathBuf::from("/usr/local/bin/winetricks")),
                paths
                    .home
                    .as_ref()
                    .map(|home| home.join(".local/bin/winetricks")),
            ])
        })
        .map(|path| path.to_string_lossy().to_string());
    scan.protonfixes = paths
        .home
        .as_ref()
        .and_then(|home| {
            first_dir([
                Some(home.join(".local/share/protonfixes")),
                Some(home.join(".config/protonfixes")),
            ])
        })
        .or_else(|| find_in_dirs(&paths.path_dirs, "protonfixes"))
        .map(|path| path.to_string_lossy().to_string());

    sort_tools(&mut scan.tools);
    scan.umu = first_of(&scan.tools, ToolKind::Umu).map(|tool| tool.path.clone());
    scan.proton = first_of(&scan.tools, ToolKind::Proton).map(|tool| tool.path.clone());
    scan.wine = first_of(&scan.tools, ToolKind::Wine).map(|tool| tool.path.clone());

    if scan.steam_client_path.is_none() {
        scan.steam_client_path = paths
            .home
            .as_ref()
            .map(|home| home.join(".local/share/Steam"))
            .filter(|path| path.is_dir())
            .or_else(|| {
                paths
                    .home
                    .as_ref()
                    .map(|home| home.join(".steam/steam"))
                    .filter(|path| path.is_dir())
            })
            .map(|path| path.to_string_lossy().to_string());
    }

    scan
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn child_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn find_in_dirs(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    dirs.iter()
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn first_file(candidates: impl IntoIterator<Item = Option<PathBuf>>) -> Option<PathBuf> {
    candidates.into_iter().flatten().find(|path| path.is_file())
}

fn first_dir(candidates: impl IntoIterator<Item = Option<PathBuf>>) -> Option<PathBuf> {
    candidates.into_iter().flatten().find(|path| path.is_dir())
}

/// A configured path may name the executable or its folder.
fn resolve_override(value: &str, primary: &str, alternate: Option<&str>) -> Option<PathBuf> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let path = PathBuf::from(value);
    if path.is_file() {
        return Some(path);
    }
    if path.is_dir() {
        let direct = path.join(primary);
        if direct.is_file() {
            return Some(direct);
        }
        if let Some(alternate) = alternate {
            let candidate = path.join(alternate);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// The Proton override may be the folder or the script inside it.
fn resolve_proton_override(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let path = PathBuf::from(value);
    if path.is_file()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name == "proton")
            .unwrap_or(false)
    {
        return path.parent().map(PathBuf::from);
    }
    if path.join("proton").is_file() {
        return Some(path);
    }
    None
}

fn add_umu(
    scan: &mut CompatScan,
    seen: &mut BTreeSet<(ToolKind, String)>,
    path: PathBuf,
    source: &str,
) {
    let path = canonical(&path);
    let key = (ToolKind::Umu, path.to_string_lossy().to_string());
    if !seen.insert(key) {
        return;
    }
    scan.tools.push(CompatTool {
        id: format!("{}:{}", ToolKind::Umu.as_str(), path.display()),
        name: "umu-run".to_string(),
        kind: ToolKind::Umu,
        version: None,
        path: path.to_string_lossy().to_string(),
        proton_path: None,
        source: source.to_string(),
        steam_root: None,
    });
}

fn add_wine(
    scan: &mut CompatScan,
    seen: &mut BTreeSet<(ToolKind, String)>,
    path: PathBuf,
    source: &str,
) {
    let path = canonical(&path);
    let key = (ToolKind::Wine, path.to_string_lossy().to_string());
    if !seen.insert(key) {
        return;
    }
    scan.tools.push(CompatTool {
        id: format!("{}:{}", ToolKind::Wine.as_str(), path.display()),
        name: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("wine")
            .to_string(),
        kind: ToolKind::Wine,
        version: wine_version(&path),
        path: path.to_string_lossy().to_string(),
        proton_path: None,
        source: source.to_string(),
        steam_root: None,
    });
}

fn add_proton(
    scan: &mut CompatScan,
    seen: &mut BTreeSet<(ToolKind, String)>,
    dir: &Path,
    source: &str,
    steam_root: Option<String>,
) {
    let dir = canonical(dir);
    let key = (ToolKind::Proton, dir.to_string_lossy().to_string());
    if !seen.insert(key) {
        return;
    }
    let script = dir.join("proton");
    let name = dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Proton")
        .to_string();
    let version = std::fs::read_to_string(dir.join("version"))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    scan.tools.push(CompatTool {
        id: format!("{}:{}", ToolKind::Proton.as_str(), dir.display()),
        name,
        kind: ToolKind::Proton,
        version,
        path: script.to_string_lossy().to_string(),
        proton_path: Some(dir.to_string_lossy().to_string()),
        source: source.to_string(),
        steam_root,
    });
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn wine_version(path: &Path) -> Option<String> {
    // `wine --version` is a subprocess; only do it for a regular file and never
    // fail detection when it cannot run.
    if !path.is_file() {
        return None;
    }
    let output = std::process::Command::new(path)
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn first_of(tools: &[CompatTool], kind: ToolKind) -> Option<&CompatTool> {
    tools.iter().find(|tool| tool.kind == kind)
}

fn sort_tools(tools: &mut [CompatTool]) {
    tools.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| proton_rank(right).cmp(&proton_rank(left)))
            .then_with(|| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            })
    });
}

/// GE-Proton first, then Experimental/Hotfix, then numbered stable builds.
fn proton_rank(tool: &CompatTool) -> u32 {
    let name = tool.name.to_ascii_lowercase();
    if name.contains("ge-proton") || name.starts_with("ge-") {
        300
    } else if name.contains("experimental") {
        200
    } else if name.contains("hotfix") {
        190
    } else if tool.kind == ToolKind::Wine {
        0
    } else {
        100
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-compat-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn fixture() -> (PathBuf, ScanPaths) {
        let root = temp_dir("steam");
        let steam = root.join("steam");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();

        write(
            &steam.join("steamapps/common/Proton - Experimental/proton"),
            "#!/bin/sh\n",
        );
        write(
            &steam.join("steamapps/common/Proton - Experimental/version"),
            "proton-exp-9.0\n",
        );
        write(
            &steam.join("steamapps/common/Proton 9.0/proton"),
            "#!/bin/sh\n",
        );
        write(
            &steam.join("compatibilitytools.d/GE-Proton9-20/proton"),
            "#!/bin/sh\n",
        );
        write(
            &steam.join("compatibilitytools.d/GE-Proton9-20/version"),
            "GE-Proton9-20\n",
        );
        // Not a Proton folder: no `proton` script.
        std::fs::create_dir_all(steam.join("compatibilitytools.d/NotAProton")).unwrap();

        write(&bin.join("umu-run"), "#!/bin/sh\n");
        write(&bin.join("wine"), "#!/bin/sh\n");
        write(&bin.join("winetricks"), "#!/bin/sh\n");

        let paths = ScanPaths {
            home: Some(root.join("home")),
            steam_roots: vec![steam],
            path_dirs: vec![bin],
            overrides: CompatOverrides::default(),
        };
        (root, paths)
    }

    #[test]
    fn scans_steam_proton_umu_and_wine_from_fixture_trees() {
        let (root, paths) = fixture();
        let scan = scan_with(&paths);

        assert!(scan.linux);
        assert_eq!(scan.steam_roots.len(), 1);
        assert_eq!(scan.tools.len(), 5, "tools: {:?}", scan.tools);

        let names: Vec<&str> = scan.tools.iter().map(|tool| tool.name.as_str()).collect();
        assert_eq!(names[0], "umu-run");
        assert_eq!(names[1], "GE-Proton9-20", "GE-Proton must rank first");
        assert_eq!(names[2], "Proton - Experimental");
        assert_eq!(names[3], "Proton 9.0");
        assert_eq!(names[4], "wine");
        assert_eq!(
            scan.tools[1].source, "compatibilitytools.d",
            "compatibilitytools.d is reported as the source"
        );
        assert_eq!(scan.tools[1].version.as_deref(), Some("GE-Proton9-20"));
        assert_eq!(scan.tools[2].version.as_deref(), Some("proton-exp-9.0"));
        assert_eq!(scan.tools[0].kind, ToolKind::Umu);
        assert_eq!(scan.tools[3].kind, ToolKind::Proton);
        assert_eq!(
            scan.tools.last().map(|tool| tool.kind),
            Some(ToolKind::Wine),
            "wine is appended after Proton tools"
        );
        assert_eq!(scan.umu.as_deref(), Some(scan.tools[0].path.as_str()));
        assert!(scan.winetricks.is_some());
        assert!(scan.tools.iter().all(|tool| !tool.path.is_empty()));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn wine_is_the_last_resort_tool() {
        let (root, mut paths) = fixture();
        // Remove Proton tools so only Wine is left.
        std::fs::remove_dir_all(root.join("steam/steamapps/common")).unwrap();
        std::fs::remove_dir_all(root.join("steam/compatibilitytools.d")).unwrap();
        paths.path_dirs = vec![root.join("bin")];

        let scan = scan_with(&paths);
        let kinds: Vec<ToolKind> = scan.tools.iter().map(|tool| tool.kind).collect();
        assert_eq!(kinds, vec![ToolKind::Umu, ToolKind::Wine]);
        assert!(scan.proton.is_none());
        assert!(scan.wine.is_some());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn overrides_win_and_invalid_ones_warn() {
        let (root, mut paths) = fixture();
        let custom = root.join("custom/umu-run");
        write(&custom, "#!/bin/sh\n");
        paths.overrides = CompatOverrides {
            umu: custom.to_string_lossy().to_string(),
            proton: root
                .join("steam/compatibilitytools.d/GE-Proton9-20")
                .to_string_lossy()
                .to_string(),
            wine: "/does/not/exist/wine".to_string(),
        };

        let scan = scan_with(&paths);
        assert_eq!(scan.tools[0].source, "override");
        assert_eq!(
            scan.tools[0].path,
            custom.canonicalize().unwrap().to_string_lossy()
        );
        assert_eq!(scan.tools[1].source, "override");
        assert!(scan
            .warnings
            .iter()
            .any(|warning| warning.contains("Wine path")));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_empty_machine_reports_no_tools_without_failing() {
        let root = temp_dir("empty");
        let scan = scan_with(&ScanPaths {
            home: Some(root.join("home")),
            steam_roots: Vec::new(),
            path_dirs: Vec::new(),
            overrides: CompatOverrides::default(),
        });
        assert!(scan.tools.is_empty());
        assert!(scan.umu.is_none());
        assert!(scan.winetricks.is_none());
        // The fallback client path must not invent a root that does not exist.
        assert!(scan.steam_client_path.is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn proton_override_accepts_the_script_itself() {
        let (root, mut paths) = fixture();
        let script = root.join("steam/compatibilitytools.d/GE-Proton9-20/proton");
        paths.overrides.proton = script.to_string_lossy().to_string();
        paths.steam_roots.clear();
        paths.path_dirs.clear();

        let scan = scan_with(&paths);
        assert_eq!(scan.tools.len(), 1);
        assert_eq!(scan.tools[0].kind, ToolKind::Proton);
        assert_eq!(scan.tools[0].source, "override");

        std::fs::remove_dir_all(&root).ok();
    }
}
