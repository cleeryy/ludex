//! Launcher discovery and import (#116).
//!
//! Ludex never downloads games another launcher installed. This module reads
//! the metadata those launchers already keep on disk (Steam manifests, Epic's
//! `LauncherInstalled.dat`, GOG Galaxy's SQLite database, itch's `butler.db`,
//! Heroic's caches, Lutris' `pga.db`, Windows registry keys and the Start menu)
//! and imports the games the user selects as *external* entries that launch
//! through the owning client.
//!
//! Discovery is best effort. Every parser is pure and works on a fixture, so
//! CI covers each file format without a launcher installed; the few collectors
//! that need a real client (Windows registry, PowerShell, a live SQLite
//! database) only *read* what the client wrote, and a failure is a warning, not
//! an error.
//!
//! Imported entries keep `source` = the launcher id and a `launchUri`. Their
//! install folder belongs to the launcher: uninstalling the Ludex entry only
//! removes the index row (see `commands::uninstall_game`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::steam::{self, TitleIndex};

/// Launchers Ludex can read on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Launcher {
    Steam,
    Epic,
    Gog,
    Ea,
    Ubisoft,
    Xbox,
    Itch,
    Heroic,
    Lutris,
}

impl Launcher {
    /// Every source Ludex knows, in display order (#155).
    pub const ALL: [Launcher; 9] = [
        Launcher::Steam,
        Launcher::Epic,
        Launcher::Gog,
        Launcher::Ea,
        Launcher::Ubisoft,
        Launcher::Xbox,
        Launcher::Itch,
        Launcher::Heroic,
        Launcher::Lutris,
    ];

    /// Stable id stored in `installed_games.source` and sent by the web UI.
    pub fn id(self) -> &'static str {
        match self {
            Launcher::Steam => "steam",
            Launcher::Epic => "epic",
            Launcher::Gog => "gog",
            Launcher::Ea => "ea",
            Launcher::Ubisoft => "ubisoft",
            Launcher::Xbox => "xbox",
            Launcher::Itch => "itch",
            Launcher::Heroic => "heroic",
            Launcher::Lutris => "lutris",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Launcher::Steam => "Steam",
            Launcher::Epic => "Epic Games",
            Launcher::Gog => "GOG Galaxy",
            Launcher::Ea => "EA app",
            Launcher::Ubisoft => "Ubisoft Connect",
            Launcher::Xbox => "Xbox / Microsoft Store",
            Launcher::Itch => "itch.io",
            Launcher::Heroic => "Heroic",
            Launcher::Lutris => "Lutris",
        }
    }

    /// Parse the id sent back by the UI.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "steam" => Some(Launcher::Steam),
            "epic" => Some(Launcher::Epic),
            "gog" => Some(Launcher::Gog),
            "ea" | "origin" => Some(Launcher::Ea),
            "ubisoft" | "uplay" => Some(Launcher::Ubisoft),
            "xbox" | "microsoft" => Some(Launcher::Xbox),
            "itch" | "itchio" => Some(Launcher::Itch),
            "heroic" => Some(Launcher::Heroic),
            "lutris" => Some(Launcher::Lutris),
            _ => None,
        }
    }

    /// Whether this launcher exists on the current platform at all.
    pub fn supported_here(self) -> bool {
        match self {
            Launcher::Steam | Launcher::Epic | Launcher::Gog | Launcher::Ea | Launcher::Ubisoft => {
                true
            }
            Launcher::Xbox => cfg!(windows),
            Launcher::Itch | Launcher::Heroic => true,
            Launcher::Lutris => cfg!(unix),
        }
    }
}

/// The launcher sources the user enabled in Settings → Integrations (#155).
///
/// A disabled source is never probed: no manifest, registry key or SQLite
/// database is opened for it. The scan still reports its `LauncherSource` with
/// `disabled: true`, so the preview can say "Disabled" instead of "Not
/// detected". Games already imported from it stay in the local index: the set
/// only decides what a *scan* reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LauncherSet {
    enabled: BTreeSet<Launcher>,
}

impl LauncherSet {
    /// Every source: the default for a fresh install and for a settings file
    /// written before #155.
    pub fn all() -> Self {
        Self {
            enabled: Launcher::ALL.into_iter().collect(),
        }
    }

    /// Build a set from the ids stored in settings or sent by the web UI.
    /// Unknown ids are ignored instead of failing the scan.
    pub fn from_ids<I, S>(ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            enabled: ids
                .into_iter()
                .filter_map(|id| Launcher::parse(id.as_ref()))
                .collect(),
        }
    }

    pub fn contains(&self, launcher: Launcher) -> bool {
        self.enabled.contains(&launcher)
    }

    /// Enabled ids in display order.
    pub fn ids(&self) -> Vec<&'static str> {
        Launcher::ALL
            .into_iter()
            .filter(|launcher| self.contains(*launcher))
            .map(Launcher::id)
            .collect()
    }
}

/// Canonical ids for a settings value (#155): known sources only, deduplicated
/// and in display order, so the file stays stable across saves.
pub fn canonical_ids<I, S>(values: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    LauncherSet::from_ids(values)
        .ids()
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// A game another launcher installed on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredGame {
    pub source: Launcher,
    /// Id inside the launcher (Steam app id, Epic AppName, GOG product id…).
    pub external_id: String,
    pub name: String,
    /// Install folder, when the launcher records one.
    pub install_dir: Option<String>,
    /// URI that starts the game through its launcher.
    pub launch_uri: Option<String>,
    pub size_bytes: u64,
    pub last_played: Option<i64>,
    /// Extra launcher-side detail shown next to the title.
    pub detail: Option<String>,
    /// Why the entry cannot be imported as-is.
    pub issue: Option<String>,
}

impl DiscoveredGame {
    /// Stable local id used when the game has no Ludex counterpart.
    ///
    /// Steam keeps its historical `-appId` scheme (#13); every other launcher
    /// gets a deterministic negative id derived from `source` + `external_id`.
    pub fn local_id(&self) -> i64 {
        match self.source {
            Launcher::Steam => self
                .external_id
                .parse::<u32>()
                .map(steam::local_game_id)
                .unwrap_or_else(|_| hash_local_id(self.source, &self.external_id)),
            _ => hash_local_id(self.source, &self.external_id),
        }
    }
}

/// Deterministic negative id in `[-2e9, -1e9)`, disjoint from Steam's `-appId`
/// for realistic app ids and from every positive server id.
pub fn hash_local_id(source: Launcher, external_id: &str) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in source
        .id()
        .bytes()
        .chain(std::iter::once(0))
        .chain(external_id.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    -(1_000_000_000 + (hash % 1_000_000_000) as i64)
}

/// What one launcher contributed to a scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherSource {
    pub id: Launcher,
    pub label: &'static str,
    pub found: usize,
    /// False when nothing could be read (not installed, unsupported platform,
    /// or disabled by the user).
    pub available: bool,
    /// True when the user turned this source off in Settings → Integrations
    /// (#155). Nothing was read for it; `found` is always 0.
    pub disabled: bool,
    pub detail: String,
}

/// Everything a scan found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherScan {
    /// Files and folders that were read, in order.
    pub roots: Vec<String>,
    pub games: Vec<DiscoveredGame>,
    pub sources: Vec<LauncherSource>,
    pub warnings: Vec<String>,
}

/// Optional filesystem roots. Tests fill them with fixtures; production
/// detection fills them from the environment.
#[derive(Debug, Clone, Default)]
pub struct ScanRoots {
    pub steam: Option<PathBuf>,
    pub program_data: Option<PathBuf>,
    pub app_data: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub config_home: Option<PathBuf>,
    pub data_home: Option<PathBuf>,
}

impl ScanRoots {
    /// Detect the usual locations for the current platform.
    pub fn detect(steam_root: Option<&str>) -> Self {
        let env = |key: &str| std::env::var_os(key).map(PathBuf::from);
        let home = env("HOME").or_else(|| env("USERPROFILE"));
        let config_home =
            env("XDG_CONFIG_HOME").or_else(|| home.as_ref().map(|home| home.join(".config")));
        let data_home =
            env("XDG_DATA_HOME").or_else(|| home.as_ref().map(|home| home.join(".local/share")));
        Self {
            steam: steam::discover_roots(steam_root).into_iter().next(),
            program_data: env("PROGRAMDATA"),
            app_data: env("APPDATA"),
            home,
            config_home,
            data_home,
        }
    }
}

/* ------------------------------- collectors ------------------------------ */

struct Collector {
    scan: LauncherScan,
}

impl Collector {
    fn new() -> Self {
        Self {
            scan: LauncherScan::default(),
        }
    }

    fn read(&mut self, path: &Path) -> Option<String> {
        self.scan.roots.push(path.to_string_lossy().to_string());
        match std::fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(error) => {
                self.warn(format!("Could not read {}: {error}", path.display()));
                None
            }
        }
    }

    fn warn(&mut self, message: String) {
        if !self
            .scan
            .warnings
            .iter()
            .any(|existing| existing == &message)
        {
            self.scan.warnings.push(message);
        }
    }

    fn push(&mut self, mut game: DiscoveredGame) {
        // The launcher may report a folder that was deleted behind its back.
        if let Some(dir) = game.install_dir.as_deref() {
            if !dir.trim().is_empty() && !Path::new(dir).is_dir() {
                self.warn(format!(
                    "{} is listed by {} but its folder is missing: {dir}",
                    game.name,
                    game.source.label()
                ));
                return;
            }
        }
        if game.launch_uri.is_none() && game.issue.is_none() {
            game.issue = Some(format!(
                "{} does not expose a launch URI for this entry; paste one to import it.",
                game.source.label()
            ));
        }
        if self.scan.games.iter().any(|existing| {
            existing.source == game.source && existing.external_id == game.external_id
        }) {
            return;
        }
        // Two clients can report the same installation (Epic and Heroic, for
        // example). Keep the first, which follows `Launcher`'s order.
        if let Some(dir) = game.install_dir.as_deref() {
            let normalized = normalize_path(dir);
            if self.scan.games.iter().any(|existing| {
                existing
                    .install_dir
                    .as_deref()
                    .map(normalize_path)
                    .is_some_and(|other| other == normalized)
            }) {
                return;
            }
        }
        self.scan.games.push(game);
    }
}

/// Last path component, for both `/` and `\` separated paths. Registry values
/// are Windows paths even when the parser runs on Linux in a test.
fn path_basename(path: &str) -> Option<&str> {
    path.trim()
        .rsplit(['/', '\\'])
        .find(|part| !part.trim().is_empty())
}

/// Linux paths are case sensitive, Windows and macOS are not.
fn normalize_path(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(['/', '\\']).replace('\\', "/");
    if cfg!(windows) || cfg!(target_os = "macos") {
        trimmed.to_ascii_lowercase()
    } else {
        trimmed
    }
}

/// Read every enabled launcher reachable from `roots` (#155).
///
/// Sources outside `enabled` are not touched at all: their scanners never run,
/// so no file, registry key or database is read for them.
pub fn scan(roots: &ScanRoots, enabled: &LauncherSet) -> LauncherScan {
    let mut collector = Collector::new();
    let mut sources: Vec<LauncherSource> = Vec::with_capacity(Launcher::ALL.len());
    for id in Launcher::ALL {
        if enabled.contains(id) {
            sources.push(scan_source(id, &mut collector, roots));
        } else {
            sources.push(disabled_status(id));
        }
    }

    let mut scan = collector.scan;
    // Assign the counts once all collectors are done.
    for source in &mut sources {
        source.found = scan
            .games
            .iter()
            .filter(|game| game.source == source.id)
            .count();
    }
    scan.sources = sources;
    scan.games.sort_by(|left, right| {
        left.source.cmp(&right.source).then_with(|| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
        })
    });
    scan
}

/// Run the collector of one enabled source.
fn scan_source(id: Launcher, collector: &mut Collector, roots: &ScanRoots) -> LauncherSource {
    match id {
        Launcher::Steam => scan_steam(collector, roots),
        Launcher::Epic => scan_epic(collector, roots),
        Launcher::Gog => scan_gog(collector, roots),
        Launcher::Ea => scan_ea(collector, roots),
        Launcher::Ubisoft => scan_ubisoft(collector),
        Launcher::Xbox => scan_xbox(collector),
        Launcher::Itch => scan_itch(collector, roots),
        Launcher::Heroic => scan_heroic(collector, roots),
        Launcher::Lutris => scan_lutris(collector, roots),
    }
}

fn source_status(id: Launcher, available: bool, detail: impl Into<String>) -> LauncherSource {
    LauncherSource {
        id,
        label: id.label(),
        found: 0,
        available,
        disabled: false,
        detail: detail.into(),
    }
}

/// Status of a source the user turned off: nothing was read for it.
fn disabled_status(id: Launcher) -> LauncherSource {
    LauncherSource {
        id,
        label: id.label(),
        found: 0,
        available: false,
        disabled: true,
        detail: "Disabled in Settings → Integrations".to_string(),
    }
}

fn scan_steam(collector: &mut Collector, roots: &ScanRoots) -> LauncherSource {
    let Some(root) = roots.steam.as_ref() else {
        return source_status(Launcher::Steam, false, "Steam was not detected");
    };
    let scan = steam::scan(std::slice::from_ref(root));
    collector.scan.roots.extend(scan.roots);
    for warning in scan.warnings {
        collector.warn(warning);
    }
    for game in scan.games {
        collector.push(DiscoveredGame {
            source: Launcher::Steam,
            external_id: game.app_id.to_string(),
            name: game.name,
            install_dir: Some(game.install_dir),
            launch_uri: Some(steam::launch_uri(game.app_id)),
            size_bytes: game.size_bytes,
            last_played: game.last_played,
            detail: Some(format!("app {}", game.app_id)),
            issue: None,
        });
    }
    source_status(Launcher::Steam, true, root.display().to_string())
}

fn scan_epic(collector: &mut Collector, roots: &ScanRoots) -> LauncherSource {
    let Some(program_data) = roots.program_data.as_ref() else {
        return source_status(Launcher::Epic, false, "Windows ProgramData was not found");
    };
    let installed_path = program_data.join("Epic/UnrealEngineLauncher/LauncherInstalled.dat");
    let manifest_dir = program_data.join("Epic/EpicGamesLauncher/Data/Manifests");

    let mut by_app: BTreeMap<String, EpicEntry> = BTreeMap::new();
    if let Some(text) = collector.read(&installed_path) {
        match parse_epic_installed(&text) {
            Ok(entries) => {
                for entry in entries {
                    by_app.insert(entry.app_name.clone(), entry);
                }
            }
            Err(error) => collector.warn(format!(
                "Could not parse {}: {error}",
                installed_path.display()
            )),
        }
    }

    // Manifests carry the catalog namespace and item id the launcher URI needs.
    if let Ok(entries) = std::fs::read_dir(&manifest_dir) {
        collector
            .scan
            .roots
            .push(manifest_dir.to_string_lossy().to_string());
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("item") {
                continue;
            }
            let Some(text) = collector.read(&path) else {
                continue;
            };
            match parse_epic_manifest(&text) {
                Ok(Some(manifest)) => {
                    let merged = by_app
                        .entry(manifest.app_name.clone())
                        .or_insert(EpicEntry {
                            app_name: manifest.app_name.clone(),
                            display_name: manifest.display_name.clone(),
                            install_location: manifest.install_location.clone(),
                            namespace: None,
                            catalog_item_id: None,
                        });
                    merged.namespace = manifest.namespace.clone().or(merged.namespace.take());
                    merged.catalog_item_id = manifest
                        .catalog_item_id
                        .clone()
                        .or(merged.catalog_item_id.take());
                    if merged.install_location.is_none() {
                        merged.install_location = manifest.install_location;
                    }
                    if merged.display_name.trim().is_empty() {
                        merged.display_name = manifest.display_name;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    collector.warn(format!("Could not parse {}: {error}", path.display()))
                }
            }
        }
    }

    let available = !by_app.is_empty() || installed_path.exists() || manifest_dir.is_dir();
    for entry in by_app.into_values() {
        let uri = epic_uri(&entry);
        let name = if entry.display_name.trim().is_empty() {
            entry.app_name.clone()
        } else {
            entry.display_name.clone()
        };
        collector.push(DiscoveredGame {
            source: Launcher::Epic,
            external_id: entry.app_name.clone(),
            name,
            install_dir: entry.install_location.clone(),
            launch_uri: Some(uri),
            size_bytes: 0,
            last_played: None,
            detail: entry
                .namespace
                .as_deref()
                .map(|namespace| format!("Epic · {namespace}")),
            issue: None,
        });
    }

    source_status(
        Launcher::Epic,
        available,
        if available {
            "LauncherInstalled.dat".to_string()
        } else {
            "Epic Games Launcher was not detected".to_string()
        },
    )
}

fn scan_gog(collector: &mut Collector, roots: &ScanRoots) -> LauncherSource {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut available = false;

    if let Some(program_data) = roots.program_data.as_ref() {
        let database = program_data.join("GOG.com/Galaxy/storage/galaxy-2.0.db");
        if database.is_file() {
            available = true;
            collector
                .scan
                .roots
                .push(database.to_string_lossy().to_string());
            match read_gog_database(&database) {
                Ok(games) => {
                    for game in games {
                        seen.insert(game.external_id.clone());
                        collector.push(game);
                    }
                }
                Err(error) => {
                    collector.warn(format!("Could not read {}: {error}", database.display()))
                }
            }
        }
    }

    #[cfg(windows)]
    for key in [
        r"HKLM\SOFTWARE\WOW6432Node\GOG.com\Games",
        r"HKLM\SOFTWARE\GOG.com\Games",
        r"HKCU\Software\GOG.com\Games",
    ] {
        let Some(text) = query_registry(key) else {
            continue;
        };
        available = true;
        collector.scan.roots.push(format!("registry {key}"));
        match parse_gog_registry(&text) {
            Ok(games) => {
                for game in games {
                    if seen.insert(game.external_id.clone()) {
                        collector.push(game);
                    }
                }
            }
            Err(error) => collector.warn(format!("Could not parse registry {key}: {error}")),
        }
    }

    source_status(
        Launcher::Gog,
        available,
        if !available {
            "GOG Galaxy was not detected".to_string()
        } else {
            format!(
                "{} install{}",
                seen.len(),
                if seen.len() == 1 { "" } else { "s" }
            )
        },
    )
}

fn scan_ea(collector: &mut Collector, roots: &ScanRoots) -> LauncherSource {
    #[cfg(windows)]
    let mut found = 0usize;
    #[cfg(not(windows))]
    let found = 0usize;

    #[cfg(windows)]
    for key in [
        r"HKLM\SOFTWARE\WOW6432Node\Electronic Arts\EA Games",
        r"HKLM\SOFTWARE\Electronic Arts\EA Games",
    ] {
        let Some(text) = query_registry(key) else {
            continue;
        };
        collector.scan.roots.push(format!("registry {key}"));
        match parse_ea_registry(&text) {
            Ok(games) => {
                found += games.len();
                for game in games {
                    collector.push(game);
                }
            }
            Err(error) => collector.warn(format!("Could not parse registry {key}: {error}")),
        }
    }

    // The Origin-era client wrote offer ids here; the EA app still reads them.
    if let Some(program_data) = roots.program_data.as_ref() {
        let local_content = program_data.join("Origin/LocalContent");
        if let Ok(entries) = std::fs::read_dir(&local_content) {
            collector
                .scan
                .roots
                .push(local_content.to_string_lossy().to_string());
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("mfst") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                    continue;
                };
                let Some(offer) = stem.strip_prefix("Origin.").or(Some(stem)) else {
                    continue;
                };
                // The manifest does not carry a title; the offer id is the only
                // thing we can show, so leave this to the registry entry.
                let _ = offer;
            }
        }
    }

    source_status(
        Launcher::Ea,
        found > 0,
        if found == 0 {
            "EA app was not detected".to_string()
        } else {
            format!("{found} install{}", if found == 1 { "" } else { "s" })
        },
    )
}

fn scan_ubisoft(collector: &mut Collector) -> LauncherSource {
    #[cfg(windows)]
    let mut found = 0usize;
    #[cfg(not(windows))]
    let found = 0usize;
    #[cfg(not(windows))]
    let _ = collector;

    #[cfg(windows)]
    for key in [
        r"HKCU\Software\Ubisoft\Launcher\Installs",
        r"HKLM\SOFTWARE\WOW6432Node\Ubisoft\Launcher\Installs",
        r"HKLM\SOFTWARE\Ubisoft\Launcher\Installs",
    ] {
        let Some(text) = query_registry(key) else {
            continue;
        };
        collector.scan.roots.push(format!("registry {key}"));
        match parse_ubisoft_registry(&text) {
            Ok(games) => {
                found += games.len();
                for game in games {
                    collector.push(game);
                }
            }
            Err(error) => collector.warn(format!("Could not parse registry {key}: {error}")),
        }
    }

    source_status(
        Launcher::Ubisoft,
        found > 0,
        if found == 0 {
            if cfg!(windows) {
                "Ubisoft Connect was not detected".to_string()
            } else {
                "Windows only".to_string()
            }
        } else {
            format!("{found} install{}", if found == 1 { "" } else { "s" })
        },
    )
}

fn scan_xbox(collector: &mut Collector) -> LauncherSource {
    #[cfg(windows)]
    {
        let mut found = 0usize;
        // Names come from the Start menu: the Appx manifest usually stores a
        // resource key, not a display name.
        let mut names: BTreeMap<String, String> = BTreeMap::new();
        if let Some(text) = query_start_apps() {
            collector
                .scan
                .roots
                .push("Start menu (PowerShell)".to_string());
            match parse_start_apps(&text) {
                Ok(apps) => {
                    for app in apps {
                        names.insert(app.app_id.clone(), app.name);
                    }
                }
                Err(error) => {
                    collector.warn(format!("Could not parse the Start menu list: {error}"))
                }
            }
        }

        // The installed packages add entries the Start menu does not list and
        // give each one its real application id.
        if let Some(text) = query_appx_packages() {
            collector
                .scan
                .roots
                .push("Appx packages (PowerShell)".to_string());
            match parse_appx_packages(&text) {
                Ok(packages) => {
                    for package in packages {
                        let Some(location) = package.install_location.as_deref() else {
                            continue;
                        };
                        let manifest = Path::new(location).join("AppxManifest.xml");
                        let Some(xml) = collector.read(&manifest) else {
                            continue;
                        };
                        let Some(mut app) = parse_appx_manifest(&xml, &package.package_family_name)
                        else {
                            continue;
                        };
                        if let Some(name) = names.get(&app.app_id) {
                            app.name = name.clone();
                        }
                        let app_id = app.app_id;
                        found += 1;
                        collector.push(DiscoveredGame {
                            source: Launcher::Xbox,
                            external_id: app_id.clone(),
                            name: app.name,
                            install_dir: Some(location.to_string()),
                            launch_uri: Some(shell_apps_uri(&app_id)),
                            size_bytes: 0,
                            last_played: None,
                            detail: Some("Microsoft Store".to_string()),
                            issue: None,
                        });
                    }
                }
                Err(error) => {
                    collector.warn(format!("Could not parse the Appx package list: {error}"))
                }
            }
        }

        // Fall back to the Start menu when the manifests are not readable
        // (`WindowsApps` is ACL-restricted for a regular user).
        if found == 0 {
            for (app_id, name) in names {
                found += 1;
                collector.push(DiscoveredGame {
                    source: Launcher::Xbox,
                    external_id: app_id.clone(),
                    name,
                    install_dir: None,
                    launch_uri: Some(shell_apps_uri(&app_id)),
                    size_bytes: 0,
                    last_played: None,
                    detail: Some("Start menu".to_string()),
                    issue: None,
                });
            }
        }

        if !collector.scan.roots.is_empty() && found > 0 {
            return source_status(
                Launcher::Xbox,
                true,
                format!(
                    "{found} Microsoft Store entr{}",
                    if found == 1 { "y" } else { "ies" }
                ),
            );
        }
        return source_status(
            Launcher::Xbox,
            false,
            "PowerShell was not available".to_string(),
        );
    }
    #[cfg(not(windows))]
    {
        let _ = collector;
        source_status(Launcher::Xbox, false, "Windows only".to_string())
    }
}

fn scan_itch(collector: &mut Collector, roots: &ScanRoots) -> LauncherSource {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(app_data) = roots.app_data.as_ref() {
        candidates.push(app_data.join("itch/db/butler.db"));
    }
    if let Some(config_home) = roots.config_home.as_ref() {
        candidates.push(config_home.join("itch/db/butler.db"));
    }
    if let Some(home) = roots.home.as_ref() {
        candidates.push(home.join("Library/Application Support/itch/db/butler.db"));
    }

    for database in candidates {
        if !database.is_file() {
            continue;
        }
        collector
            .scan
            .roots
            .push(database.to_string_lossy().to_string());
        match read_itch_database(&database) {
            Ok(games) => {
                for game in games {
                    collector.push(game);
                }
                return source_status(Launcher::Itch, true, database.display().to_string());
            }
            Err(error) => collector.warn(format!("Could not read {}: {error}", database.display())),
        }
    }
    source_status(Launcher::Itch, false, "itch was not detected".to_string())
}

fn scan_heroic(collector: &mut Collector, roots: &ScanRoots) -> LauncherSource {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(config_home) = roots.config_home.as_ref() {
        candidates.push(config_home.join("heroic"));
    }
    if let Some(app_data) = roots.app_data.as_ref() {
        candidates.push(app_data.join("heroic"));
    }
    if let Some(home) = roots.home.as_ref() {
        candidates.push(home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic"));
    }

    for root in candidates {
        if !root.is_dir() {
            continue;
        }
        collector
            .scan
            .roots
            .push(root.to_string_lossy().to_string());
        for (relative, runner) in [
            ("store_cache/legendary_library.json", "epic"),
            ("store_cache/gog_library.json", "gog"),
        ] {
            let path = root.join(relative);
            let Some(text) = collector.read(&path) else {
                continue;
            };
            match parse_heroic_library(&text, Some(runner)) {
                Ok(games) => {
                    for game in games {
                        collector.push(game);
                    }
                }
                Err(error) => {
                    collector.warn(format!("Could not parse {}: {error}", path.display()))
                }
            }
        }
        let sideload = root.join("sideload_apps/library.json");
        if let Some(text) = collector.read(&sideload) {
            match parse_heroic_library(&text, None) {
                Ok(games) => {
                    for game in games {
                        collector.push(game);
                    }
                }
                Err(error) => {
                    collector.warn(format!("Could not parse {}: {error}", sideload.display()))
                }
            }
        }
        return source_status(Launcher::Heroic, true, root.display().to_string());
    }
    source_status(
        Launcher::Heroic,
        false,
        "Heroic was not detected".to_string(),
    )
}

fn scan_lutris(collector: &mut Collector, roots: &ScanRoots) -> LauncherSource {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(data_home) = roots.data_home.as_ref() {
        candidates.push(data_home.join("lutris/pga.db"));
    }
    if let Some(home) = roots.home.as_ref() {
        candidates.push(home.join(".local/share/lutris/pga.db"));
        candidates.push(home.join(".var/app/net.lutris.Lutris/data/lutris/pga.db"));
    }

    for database in candidates {
        if !database.is_file() {
            continue;
        }
        collector
            .scan
            .roots
            .push(database.to_string_lossy().to_string());
        match read_lutris_database(&database) {
            Ok(games) => {
                for game in games {
                    collector.push(game);
                }
                return source_status(Launcher::Lutris, true, database.display().to_string());
            }
            Err(error) => collector.warn(format!("Could not read {}: {error}", database.display())),
        }
    }
    source_status(
        Launcher::Lutris,
        false,
        "Lutris was not detected".to_string(),
    )
}

#[cfg(windows)]
fn query_registry(key: &str) -> Option<String> {
    let output = crate::process::capture("reg")
        .args(["query", key, "/s"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(windows)]
fn query_appx_packages() -> Option<String> {
    let output = crate::process::capture("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-AppxPackage | Where-Object { -not $_.IsFramework -and -not $_.IsResourcePackage } | Select-Object Name,PackageFamilyName,InstallLocation,SignatureKind | ConvertTo-Json -Depth 3 -Compress",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(windows)]
fn query_start_apps() -> Option<String> {
    let output = crate::process::capture("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-StartApps | Select-Object Name,AppID | ConvertTo-Json -Depth 3 -Compress",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

/* -------------------------------- parsers -------------------------------- */

/// One Epic installation, from `LauncherInstalled.dat` or a `.item` manifest.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EpicEntry {
    pub app_name: String,
    pub display_name: String,
    pub install_location: Option<String>,
    pub namespace: Option<String>,
    pub catalog_item_id: Option<String>,
}

fn json_str<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// `%ProgramData%\Epic\UnrealEngineLauncher\LauncherInstalled.dat`.
///
/// The file is a JSON object with an `InstallationList` array; older builds
/// wrote a bare array. Entries without an `AppName` cannot be launched and are
/// skipped instead of failing the whole file.
pub fn parse_epic_installed(text: &str) -> Result<Vec<EpicEntry>, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("invalid JSON: {error}"))?;
    let list = value
        .get("InstallationList")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
        .ok_or_else(|| "no InstallationList array".to_string())?;

    let mut entries = Vec::new();
    for item in list {
        let Some(app_name) = json_str(item, &["AppName", "appName"]) else {
            continue;
        };
        entries.push(EpicEntry {
            app_name: app_name.to_string(),
            display_name: json_str(item, &["DisplayName", "displayName"])
                .unwrap_or(app_name)
                .to_string(),
            install_location: json_str(item, &["InstallLocation", "installLocation"])
                .map(str::to_string),
            namespace: json_str(item, &["NamespaceId", "CatalogNamespace", "namespaceId"])
                .map(str::to_string),
            catalog_item_id: json_str(item, &["ItemId", "CatalogItemId", "itemId"])
                .map(str::to_string),
        });
    }
    Ok(entries)
}

/// One `%ProgramData%\Epic\EpicGamesLauncher\Data\Manifests\*.item` file.
///
/// Engine plugins and other non-game payloads also get manifests; they are
/// filtered by their `AppCategories` list.
pub fn parse_epic_manifest(text: &str) -> Result<Option<EpicEntry>, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("invalid JSON: {error}"))?;
    if let Some(categories) = value.get("AppCategories").and_then(Value::as_array) {
        let is_game = categories
            .iter()
            .filter_map(Value::as_str)
            .any(|category| category.eq_ignore_ascii_case("games"));
        if !is_game {
            return Ok(None);
        }
    }
    let Some(app_name) = json_str(&value, &["AppName", "appName"]) else {
        return Ok(None);
    };
    Ok(Some(EpicEntry {
        app_name: app_name.to_string(),
        display_name: json_str(&value, &["DisplayName", "displayName"])
            .unwrap_or(app_name)
            .to_string(),
        install_location: json_str(&value, &["InstallLocation", "installLocation"])
            .map(str::to_string),
        namespace: json_str(&value, &["CatalogNamespace", "catalogNamespace"]).map(str::to_string),
        catalog_item_id: json_str(&value, &["CatalogItemId", "catalogItemId"]).map(str::to_string),
    }))
}

/// Percent-encode the characters that would break an Epic deep link.
fn encode_uri_part(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            ':' => encoded.push_str("%3A"),
            '/' => encoded.push_str("%2F"),
            '?' => encoded.push_str("%3F"),
            '&' => encoded.push_str("%26"),
            '#' => encoded.push_str("%23"),
            ' ' => encoded.push_str("%20"),
            other => encoded.push(other),
        }
    }
    encoded
}

/// `com.epicgames.launcher://apps/<ns>%3A<item>%3A<app>?action=launch&silent=true`.
pub fn epic_uri(entry: &EpicEntry) -> String {
    match (&entry.namespace, &entry.catalog_item_id) {
        (Some(namespace), Some(item)) => format!(
            "com.epicgames.launcher://apps/{}%3A{}%3A{}?action=launch&silent=true",
            encode_uri_part(namespace),
            encode_uri_part(item),
            encode_uri_part(&entry.app_name)
        ),
        _ => format!(
            "com.epicgames.launcher://apps/{}?action=launch&silent=true",
            encode_uri_part(&entry.app_name)
        ),
    }
}

/// `goggalaxy://openGameView/<productId>`: opens GOG Galaxy on the game, which
/// starts it when Galaxy is the owner. Galaxy has no documented bare launch
/// link, so this is the documented entry point.
pub fn gog_uri(product_id: &str) -> String {
    format!("goggalaxy://openGameView/{}", encode_uri_part(product_id))
}

/// `uplay://launch/<installId>/0`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn ubisoft_uri(install_id: &str) -> String {
    format!("uplay://launch/{}/0", encode_uri_part(install_id))
}

/// `origin://launchgame/<offerId>` (still handled by the EA app).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn ea_uri(offer_id: &str) -> String {
    format!("origin://launchgame/{}", encode_uri_part(offer_id))
}

/// `itch://caves/<caveId>/launch`.
pub fn itch_uri(cave_id: &str) -> String {
    format!("itch://caves/{}/launch", encode_uri_part(cave_id))
}

/// Heroic deep links carry the runner for store games and nothing for
/// sideloaded apps.
pub fn heroic_uri(runner: Option<&str>, app_name: &str) -> String {
    match runner.filter(|runner| !runner.trim().is_empty()) {
        Some(runner) => format!(
            "heroic://launch/{}/{}",
            encode_uri_part(runner),
            encode_uri_part(app_name)
        ),
        None => format!("heroic://launch/{}", encode_uri_part(app_name)),
    }
}

/// `lutris:rungameid/<id>`.
pub fn lutris_uri(game_id: &str) -> String {
    format!("lutris:rungameid/{}", encode_uri_part(game_id))
}

/// `shell:AppsFolder\<PackageFamilyName>!<ApplicationId>`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn shell_apps_uri(app_id: &str) -> String {
    format!("shell:AppsFolder\\{}", app_id.trim())
}

/* ------------------------------ registry --------------------------------- */

/// One key from `reg query … /s` output.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryKey {
    pub path: String,
    pub values: BTreeMap<String, String>,
}

impl RegistryKey {
    fn leaf(&self) -> String {
        self.path
            .rsplit('\\')
            .next()
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    fn value(&self, keys: &[&str]) -> Option<&str> {
        keys.iter().find_map(|key| {
            self.values
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(key))
                .map(|(_, value)| value.as_str())
        })
    }
}

/// Parse `reg query <key> /s`.
///
/// Lines are `HKEY_…` headers followed by `    Name    REG_SZ    Value` rows.
/// The default `(Default)` value and `REG_DWORD` rows are kept as strings. A
/// malformed line is ignored: one bad value must not hide the other games.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_registry_output(text: &str) -> Vec<RegistryKey> {
    let mut keys: Vec<RegistryKey> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("HKEY_") || trimmed.starts_with("HKCU\\") {
            keys.push(RegistryKey {
                path: trimmed.to_string(),
                values: BTreeMap::new(),
            });
            continue;
        }
        let Some(key) = keys.last_mut() else {
            continue;
        };
        let Some((name, value)) = split_registry_value(trimmed) else {
            continue;
        };
        key.values.insert(name, value);
    }
    keys
}

/// Registry value types `reg query` prints. Used to tell a type token from a
/// `REG_` inside a value name.
const REGISTRY_TYPES: [&str; 10] = [
    "SZ",
    "EXPAND_SZ",
    "DWORD",
    "QWORD",
    "BINARY",
    "MULTI_SZ",
    "NONE",
    "RESOURCE_LIST",
    "FULL_RESOURCE_DESCRIPTOR",
    "BIG_ENDIAN",
];

/// Split one value row into its name and value.
#[cfg_attr(not(windows), allow(dead_code))]
fn split_registry_value(line: &str) -> Option<(String, String)> {
    // `    Name    REG_SZ    Some value`
    let mut search = 0usize;
    while let Some(offset) = line[search..].find("REG_") {
        let position = search + offset;
        // The type column is its own field, so it follows whitespace.
        let preceded = line[..position]
            .chars()
            .last()
            .is_some_and(char::is_whitespace);
        if preceded {
            let rest = &line[position + "REG_".len()..];
            let (kind, value) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            if REGISTRY_TYPES
                .iter()
                .any(|known| kind.eq_ignore_ascii_case(known))
            {
                let name = line[..position].trim();
                if name.is_empty() {
                    return None;
                }
                return Some((name.to_string(), value.trim().to_string()));
            }
        }
        search = position + "REG_".len();
    }
    None
}

#[cfg_attr(not(windows), allow(dead_code))]
fn strip_quotes(value: &str) -> String {
    value.trim().trim_matches('"').trim().to_string()
}

/// GOG Galaxy registry installs (`GOG.com\Games\<id>`).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_gog_registry(text: &str) -> Result<Vec<DiscoveredGame>, String> {
    let mut games = Vec::new();
    for key in parse_registry_output(text) {
        if key.values.is_empty() {
            continue;
        }
        let product_id = key.leaf();
        if product_id.is_empty() {
            continue;
        }
        let name = key
            .value(&["gameName", "GameName", "DisplayName"])
            .map(strip_quotes)
            .filter(|name| !name.is_empty());
        let Some(name) = name else {
            continue;
        };
        let install_dir = key
            .value(&["path", "Path", "InstallLocation"])
            .map(strip_quotes)
            .filter(|path| !path.is_empty());
        games.push(DiscoveredGame {
            source: Launcher::Gog,
            external_id: product_id.clone(),
            name,
            install_dir,
            launch_uri: Some(gog_uri(&product_id)),
            size_bytes: 0,
            last_played: None,
            detail: Some(format!("GOG · {product_id}")),
            issue: None,
        });
    }
    Ok(games)
}

/// EA app / Origin registry installs (`Electronic Arts\EA Games\<game>`).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_ea_registry(text: &str) -> Result<Vec<DiscoveredGame>, String> {
    let mut games = Vec::new();
    for key in parse_registry_output(text) {
        if key.values.is_empty() {
            continue;
        }
        let leaf = key.leaf();
        if leaf.is_empty() {
            continue;
        }
        let name = key
            .value(&["DisplayName", "Product Name", "productName"])
            .map(strip_quotes)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| leaf.clone());
        let install_dir = key
            .value(&[
                "Install Dir",
                "InstallDir",
                "installDir",
                "Install Location",
            ])
            .map(strip_quotes)
            .filter(|path| !path.is_empty());
        // Origin-era keys sometimes carry the offer id that the EA app still
        // accepts; without one there is no documented launch URI.
        let offer = key
            .value(&["Content ID", "ContentID", "contentId", "OfferId"])
            .map(strip_quotes)
            .filter(|value| looks_like_ea_offer(value));
        games.push(DiscoveredGame {
            source: Launcher::Ea,
            external_id: leaf.clone(),
            name,
            install_dir,
            launch_uri: offer.as_deref().map(ea_uri),
            size_bytes: 0,
            last_played: None,
            detail: Some("EA app".to_string()),
            issue: offer.is_none().then(|| {
                "The EA app does not publish a launch id for this install. Paste an origin:// URI to import it."
                    .to_string()
            }),
        });
    }
    Ok(games)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn looks_like_ea_offer(value: &str) -> bool {
    let value = value.trim();
    value.starts_with("Origin.")
        || value.starts_with("OFB-")
        || (value.len() >= 6 && value.chars().all(|c| c.is_ascii_digit()))
}

/// Ubisoft Connect registry installs (`Ubisoft\Launcher\Installs\<id>`).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_ubisoft_registry(text: &str) -> Result<Vec<DiscoveredGame>, String> {
    let mut games = Vec::new();
    for key in parse_registry_output(text) {
        if key.values.is_empty() {
            continue;
        }
        let install_id = key.leaf();
        if install_id.is_empty() || !install_id.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let install_dir = key
            .value(&["InstallDir", "Install Dir", "installDir"])
            .map(strip_quotes)
            .filter(|path| !path.is_empty());
        let Some(install_dir) = install_dir else {
            continue;
        };
        let name = path_basename(&install_dir)
            .unwrap_or(&install_id)
            .to_string();
        games.push(DiscoveredGame {
            source: Launcher::Ubisoft,
            external_id: install_id.clone(),
            name,
            install_dir: Some(install_dir),
            launch_uri: Some(ubisoft_uri(&install_id)),
            size_bytes: 0,
            last_played: None,
            detail: Some(format!("Ubisoft · {install_id}")),
            issue: None,
        });
    }
    Ok(games)
}

/* ------------------------------ sqlite files ----------------------------- */

fn open_read_only(path: &Path) -> Result<rusqlite::Connection, String> {
    use rusqlite::OpenFlags;
    let connection = rusqlite::Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|error| format!("could not open the database: {error}"))?;
    connection
        .pragma_update(None, "query_only", true)
        .map_err(|error| format!("could not lock the database: {error}"))?;
    Ok(connection)
}

fn table_columns(connection: &rusqlite::Connection, table: &str) -> Vec<String> {
    let sql = format!("PRAGMA table_info({table})");
    let Ok(mut statement) = connection.prepare(&sql) else {
        return Vec::new();
    };
    let rows = statement.query_map([], |row| row.get::<_, String>(1));
    match rows {
        Ok(rows) => rows.filter_map(Result::ok).collect(),
        Err(_) => Vec::new(),
    }
}

fn column<'a>(columns: &'a [String], candidates: &[&str]) -> Option<&'a str> {
    candidates.iter().find_map(|candidate| {
        columns
            .iter()
            .find(|column| column.eq_ignore_ascii_case(candidate))
            .map(String::as_str)
    })
}

fn table_exists(connection: &rusqlite::Connection, table: &str) -> bool {
    connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false)
}

/// Read GOG Galaxy's `galaxy-2.0.db`.
///
/// The schema is undocumented and has changed between Galaxy versions, so the
/// columns are discovered by name and the read degrades to "no games" instead
/// of failing.
pub fn read_gog_database(path: &Path) -> Result<Vec<DiscoveredGame>, String> {
    let connection = open_read_only(path)?;
    let mut games = Vec::new();
    for table in ["Products", "InstalledProducts", "Library"] {
        if !table_exists(&connection, table) {
            continue;
        }
        let columns = table_columns(&connection, table);
        let (Some(id), Some(title)) = (
            column(&columns, &["productId", "product_id", "id", "gameId"]),
            column(&columns, &["title", "name", "productTitle"]),
        ) else {
            continue;
        };
        let install = column(
            &columns,
            &[
                "installationPath",
                "installLocation",
                "path",
                "installPath",
                "location",
            ],
        );
        let sql = match install {
            Some(install) => {
                format!("SELECT \"{id}\", \"{title}\", \"{install}\" FROM \"{table}\"")
            }
            None => format!("SELECT \"{id}\", \"{title}\", NULL FROM \"{table}\""),
        };
        let Ok(mut statement) = connection.prepare(&sql) else {
            continue;
        };
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        });
        let Ok(rows) = rows else {
            continue;
        };
        for row in rows.flatten() {
            let install_dir = row
                .2
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            games.push(DiscoveredGame {
                source: Launcher::Gog,
                external_id: row.0.clone(),
                name: row.1.trim().to_string(),
                install_dir,
                launch_uri: Some(gog_uri(&row.0)),
                size_bytes: 0,
                last_played: None,
                detail: Some(format!("GOG · {}", row.0)),
                issue: None,
            });
        }
        if !games.is_empty() {
            break;
        }
    }
    Ok(games)
}

/// Read itch's `butler.db` (`caves` joined to `games`).
pub fn read_itch_database(path: &Path) -> Result<Vec<DiscoveredGame>, String> {
    let connection = open_read_only(path)?;
    if !table_exists(&connection, "caves") {
        return Ok(Vec::new());
    }
    let cave_columns = table_columns(&connection, "caves");
    let Some(cave_id) = column(&cave_columns, &["id"]) else {
        return Ok(Vec::new());
    };
    let cave_path = column(&cave_columns, &["path", "install_path"]);
    let game_id_column = column(&cave_columns, &["game_id", "gameId"]);

    let mut titles: BTreeMap<String, String> = BTreeMap::new();
    if table_exists(&connection, "games") {
        let game_columns = table_columns(&connection, "games");
        if let (Some(id), Some(title)) = (
            column(&game_columns, &["id"]),
            column(&game_columns, &["title", "name"]),
        ) {
            let sql = format!("SELECT \"{id}\", \"{title}\" FROM games");
            if let Ok(mut statement) = connection.prepare(&sql) {
                if let Ok(rows) = statement.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                }) {
                    for row in rows.flatten() {
                        titles.insert(row.0, row.1);
                    }
                }
            }
        }
    }

    let mut games = Vec::new();
    for cave in query_caves(&connection, cave_id, cave_path, game_id_column) {
        let title = cave
            .2
            .as_ref()
            .and_then(|game_id| titles.get(game_id).cloned())
            .filter(|title| !title.trim().is_empty())
            .or_else(|| {
                cave.1
                    .as_ref()
                    .and_then(|path| path_basename(path).map(str::to_string))
            })
            .unwrap_or_else(|| format!("itch app {}", cave.0));
        games.push(DiscoveredGame {
            source: Launcher::Itch,
            external_id: cave.0.clone(),
            name: title,
            install_dir: cave.1,
            launch_uri: Some(itch_uri(&cave.0)),
            size_bytes: 0,
            last_played: None,
            detail: Some(format!("itch · cave {}", cave.0)),
            issue: None,
        });
    }
    Ok(games)
}

fn query_caves(
    connection: &rusqlite::Connection,
    cave_id: &str,
    cave_path: Option<&str>,
    game_id: Option<&str>,
) -> Vec<(String, Option<String>, Option<String>)> {
    let path_sql = cave_path
        .map(|column| format!("\"{column}\""))
        .unwrap_or_else(|| "NULL".to_string());
    let game_sql = game_id
        .map(|column| format!("\"{column}\""))
        .unwrap_or_else(|| "NULL".to_string());
    let sql = format!("SELECT \"{cave_id}\", {path_sql}, {game_sql} FROM caves");
    let Ok(mut statement) = connection.prepare(&sql) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    }) else {
        return Vec::new();
    };
    rows.flatten().collect()
}

/// Read Lutris' `pga.db` (`games` rows that are installed).
pub fn read_lutris_database(path: &Path) -> Result<Vec<DiscoveredGame>, String> {
    let connection = open_read_only(path)?;
    if !table_exists(&connection, "games") {
        return Ok(Vec::new());
    }
    let columns = table_columns(&connection, "games");
    let Some(id) = column(&columns, &["id"]) else {
        return Ok(Vec::new());
    };
    let name = column(&columns, &["name", "title"]);
    let directory = column(&columns, &["directory", "install_path", "path"]);
    let runner = column(&columns, &["runner"]);
    let installed = column(&columns, &["installed"]);

    let name_sql = name
        .map(|column| format!("\"{column}\""))
        .unwrap_or_else(|| "NULL".to_string());
    let directory_sql = directory
        .map(|column| format!("\"{column}\""))
        .unwrap_or_else(|| "NULL".to_string());
    let runner_sql = runner
        .map(|column| format!("\"{column}\""))
        .unwrap_or_else(|| "NULL".to_string());
    let where_sql = installed
        .map(|column| format!("WHERE COALESCE(\"{column}\", 1) != 0"))
        .unwrap_or_default();
    let sql = format!(
        "SELECT CAST(\"{id}\" AS TEXT), {name_sql}, {directory_sql}, {runner_sql} FROM games {where_sql}"
    );

    let Ok(mut statement) = connection.prepare(&sql) else {
        return Ok(Vec::new());
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    }) else {
        return Ok(Vec::new());
    };

    let mut games = Vec::new();
    for row in rows.flatten() {
        let name = row
            .1
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("Lutris game {}", row.0));
        games.push(DiscoveredGame {
            source: Launcher::Lutris,
            external_id: row.0.clone(),
            name,
            install_dir: row
                .2
                .map(|path| path.trim().to_string())
                .filter(|path| !path.is_empty()),
            launch_uri: Some(lutris_uri(&row.0)),
            size_bytes: 0,
            last_played: None,
            detail: row.3.map(|runner| format!("Lutris · {runner}")),
            issue: None,
        });
    }
    Ok(games)
}

/* -------------------------------- heroic --------------------------------- */

/// Read a Heroic cache (`legendary_library.json`, `gog_library.json` or
/// `sideload_apps/library.json`).
///
/// Heroic has changed its cache shape more than once: the parser accepts a
/// top-level array, `{ "games": [...] }` and `{ "library": [...] }`, and an
/// install block that is either `{ install_path, is_done }` or a plain path.
pub fn parse_heroic_library(
    text: &str,
    runner: Option<&str>,
) -> Result<Vec<DiscoveredGame>, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("invalid JSON: {error}"))?;
    let entries = value
        .get("games")
        .and_then(Value::as_array)
        .or_else(|| value.get("library").and_then(Value::as_array))
        .or_else(|| value.as_array())
        .ok_or_else(|| "no games array".to_string())?;

    let mut games = Vec::new();
    for entry in entries {
        let Some(app_name) = json_str(entry, &["app_name", "appName", "id"]) else {
            continue;
        };
        let title = json_str(entry, &["title", "name", "app_name"])
            .unwrap_or(app_name)
            .to_string();
        let install = entry.get("install");
        let install_path = install
            .and_then(|install| json_str(install, &["install_path", "installPath", "path"]))
            .or_else(|| json_str(entry, &["install_path", "installPath", "path"]))
            .map(str::to_string);
        let installed = install
            .and_then(|install| install.get("is_done").or_else(|| install.get("isDone")))
            .and_then(Value::as_bool)
            .or_else(|| entry.get("installed").and_then(Value::as_bool))
            .unwrap_or(install_path.is_some());
        if !installed {
            continue;
        }
        let effective_runner = runner
            .or_else(|| json_str(entry, &["runner"]))
            .map(str::to_string);
        games.push(DiscoveredGame {
            source: Launcher::Heroic,
            external_id: match effective_runner.as_deref() {
                Some(runner) => format!("{runner}:{app_name}"),
                None => app_name.to_string(),
            },
            name: title,
            install_dir: install_path,
            launch_uri: Some(heroic_uri(effective_runner.as_deref(), app_name)),
            size_bytes: 0,
            last_played: None,
            detail: Some(match effective_runner.as_deref() {
                Some(runner) => format!("Heroic · {runner} · {app_name}"),
                None => format!("Heroic · {app_name}"),
            }),
            issue: None,
        });
    }
    Ok(games)
}

/* ------------------------------- start menu ------------------------------ */

/// One packaged application from `Get-StartApps`.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartApp {
    pub name: String,
    pub app_id: String,
}

/// Package families that are Windows itself or a built-in app, never a game.
/// The list is deliberately short: a Start menu also lists ordinary apps, so
/// the import preview asks the user to untick non-games.
#[cfg_attr(not(windows), allow(dead_code))]
const NON_GAME_PACKAGES: [&str; 22] = [
    "microsoft.windows",
    "microsoft.storepurchaseapp",
    "microsoft.desktopappinstaller",
    "microsoft.microsoftedge",
    "microsoft.services",
    "microsoft.xbox",
    "microsoft.gamingapp",
    "microsoft.gethelp",
    "microsoft.people",
    "microsoft.windowscommunicationsapps",
    "microsoft.zune",
    "microsoft.bing",
    "microsoft.skypeapp",
    "microsoft.office",
    "microsoft.todos",
    "microsoft.yourphone",
    "microsoft.paint",
    "microsoft.screensketch",
    "microsoft.windowscamera",
    "microsoft.winget",
    "microsoft.copilot",
    "microsoft.lockapp",
];

/// Parse `Get-StartApps | Select Name,AppID | ConvertTo-Json`.
///
/// `ConvertTo-Json` emits a bare object for a single row, hence both shapes.
/// Only packaged applications (`<PackageFamilyName>!<AppId>`) are kept, and the
/// OS's own package families are skipped.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_start_apps(text: &str) -> Result<Vec<StartApp>, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("invalid JSON: {error}"))?;
    let entries: Vec<&Value> = match &value {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![&value],
        _ => return Err("expected an object or an array".to_string()),
    };

    let mut apps = Vec::new();
    for entry in entries {
        let Some(app_id) = json_str(entry, &["AppID", "appId", "AppId"]) else {
            continue;
        };
        let Some(name) = json_str(entry, &["Name", "name"]) else {
            continue;
        };
        // `!` separates the package family from the application id.
        let Some((package, application)) = app_id.split_once('!') else {
            continue;
        };
        if application.trim().is_empty() {
            continue;
        }
        let package_lower = package.to_ascii_lowercase();
        if NON_GAME_PACKAGES
            .iter()
            .any(|prefix| package_lower.starts_with(prefix))
        {
            continue;
        }
        apps.push(StartApp {
            name: name.to_string(),
            app_id: app_id.to_string(),
        });
    }
    apps.sort_by_key(|app| app.name.to_ascii_lowercase());
    apps.dedup_by(|left, right| left.app_id == right.app_id);
    Ok(apps)
}

/// One package from `Get-AppxPackage`.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppxPackage {
    pub name: String,
    pub package_family_name: String,
    pub install_location: Option<String>,
    /// `Store` for Store-distributed packages; other kinds are skipped.
    pub signature_kind: Option<String>,
}

/// Parse `Get-AppxPackage | Select-Object … | ConvertTo-Json`.
///
/// Framework and resource packages are filtered by the PowerShell query; this
/// parser additionally drops non-Store packages, which are SDK/runtime
/// leftovers rather than games.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_appx_packages(text: &str) -> Result<Vec<AppxPackage>, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("invalid JSON: {error}"))?;
    let entries: Vec<&Value> = match &value {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![&value],
        _ => return Err("expected an object or an array".to_string()),
    };

    let mut packages = Vec::new();
    for entry in entries {
        let Some(package_family_name) =
            json_str(entry, &["PackageFamilyName", "packageFamilyName"])
        else {
            continue;
        };
        let Some(name) = json_str(entry, &["Name", "name"]) else {
            continue;
        };
        let signature_kind =
            json_str(entry, &["SignatureKind", "signatureKind"]).map(str::to_string);
        if signature_kind
            .as_deref()
            .is_some_and(|kind| !kind.eq_ignore_ascii_case("store"))
        {
            continue;
        }
        // A package family id never contains `!`; that belongs to the AUMID.
        if package_family_name.contains('!') {
            continue;
        }
        let package_lower = package_family_name.to_ascii_lowercase();
        if NON_GAME_PACKAGES
            .iter()
            .any(|prefix| package_lower.starts_with(prefix))
        {
            continue;
        }
        packages.push(AppxPackage {
            name: name.to_string(),
            package_family_name: package_family_name.to_string(),
            install_location: json_str(entry, &["InstallLocation", "installLocation"])
                .map(str::to_string),
            signature_kind,
        });
    }
    Ok(packages)
}

/// Read `<Application Id="…">` and the display name from an `AppxManifest.xml`.
///
/// The display name is often an `ms-resource:` key; the caller replaces it with
/// the Start menu name when one exists. Returns `None` for a manifest without
/// an application entry (framework or resource payloads).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_appx_manifest(xml: &str, package_family_name: &str) -> Option<StartApp> {
    let application = xml.find("<Application ")?;
    let rest = &xml[application..];
    let tag_end = rest.find('>')?;
    let tag = &rest[..tag_end];
    let id = attribute_value(tag, "Id")?;
    let display = attribute_value(tag, "DisplayName")
        .or_else(|| {
            // VisualElements carries the visible name.
            let visual = xml.find("<uap:VisualElements ")?;
            let rest = &xml[visual..];
            let end = rest.find('>')?;
            attribute_value(&rest[..end], "DisplayName")
        })
        .filter(|name| !name.is_empty() && !name.starts_with("ms-resource:"))
        .unwrap_or_else(|| package_family_name.to_string());
    Some(StartApp {
        name: display,
        app_id: format!("{package_family_name}!{id}"),
    })
}

/// Read `Name="value"` from an XML start tag.
#[cfg_attr(not(windows), allow(dead_code))]
fn attribute_value(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/* --------------------------------- import -------------------------------- */

/// One import request from the UI.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportEntry {
    pub source: String,
    pub external_id: String,
    /// Manual mapping to a Ludex game; overrides title matching.
    #[serde(default)]
    pub game_id: Option<i64>,
    /// Manual launch URI, for launchers without a documented deep link.
    #[serde(default)]
    pub launch_uri: Option<String>,
}

/// A validated import target.
#[derive(Debug, Clone)]
pub struct ImportTarget {
    pub source: Launcher,
    pub external_id: String,
    pub name: String,
    pub install_dir: Option<String>,
    pub launch_uri: String,
    pub size_bytes: u64,
    pub game_id: Option<i64>,
}

/// Schemes that must never reach the OS handler: they open local files or
/// execute inline content instead of starting a launcher.
const BLOCKED_SCHEMES: [&str; 5] = ["file", "javascript", "data", "vbscript", "about"];

/// Accept the URIs the supported launchers register plus any explicit manual
/// override that is not a local-file or script scheme.
pub fn validate_launch_uri(value: &str) -> Result<String, String> {
    let uri = value.trim();
    if uri.is_empty() {
        return Err("The launch URI is empty.".to_string());
    }
    if uri.len() > 2048 {
        return Err("The launch URI is too long (2048 characters maximum).".to_string());
    }
    if uri.chars().any(|character| character.is_control()) {
        return Err("The launch URI contains control characters.".to_string());
    }
    let Some((scheme, _rest)) = uri.split_once(':') else {
        return Err(
            "The launch URI needs a scheme, for example steam://rungameid/440.".to_string(),
        );
    };
    if scheme.is_empty()
        || !scheme.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
    {
        return Err(format!("“{scheme}” is not a valid URI scheme."));
    }
    if BLOCKED_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()) {
        return Err(format!("The {scheme}: scheme cannot start a launcher."));
    }
    Ok(uri.to_string())
}

/// Resolve an import request against a fresh scan.
///
/// `enabled` is the source set the user allowed (#155): an entry whose source
/// was disabled after the preview is refused instead of imported.
pub fn resolve_import(
    entry: &ImportEntry,
    games: &[DiscoveredGame],
    matched: &BTreeMap<String, i64>,
    enabled: &LauncherSet,
) -> Result<ImportTarget, String> {
    let source = Launcher::parse(&entry.source)
        .ok_or_else(|| format!("Unknown launcher “{}”.", entry.source))?;
    if !enabled.contains(source) {
        return Err(format!(
            "{} is disabled in Settings → Integrations.",
            source.label()
        ));
    }
    let external_id = entry.external_id.trim();
    if external_id.is_empty() {
        return Err("The launcher entry has no id.".to_string());
    }
    let game = games
        .iter()
        .find(|game| game.source == source && game.external_id == external_id)
        .ok_or_else(|| {
            format!(
                "{} entry {external_id} is no longer installed; rescan and try again.",
                source.label()
            )
        })?;

    let launch_uri = match entry.launch_uri.as_deref() {
        Some(uri) => validate_launch_uri(uri)?,
        None => game.launch_uri.clone().ok_or_else(|| {
            format!(
                "{} needs a launch URI. Paste one for {}.",
                source.label(),
                game.name
            )
        })?,
    };
    if !source.supported_here() {
        return Err(format!(
            "{} is not available on this platform.",
            source.label()
        ));
    }
    if entry.game_id.is_some_and(|game_id| game_id <= 0) {
        return Err("A Ludex game id must be a positive game id.".to_string());
    }

    Ok(ImportTarget {
        source,
        external_id: external_id.to_string(),
        name: game.name.clone(),
        install_dir: game.install_dir.clone(),
        launch_uri,
        size_bytes: game.size_bytes,
        game_id: entry
            .game_id
            .or_else(|| matched.get(&entry.external_id).copied()),
    })
}

/// Match discovered games to server games by title.
///
/// The key is the launcher external id: it is unique inside one scan because
/// each launcher reports a title once.
pub fn match_titles(games: &[DiscoveredGame], server: &[(i64, String)]) -> BTreeMap<String, i64> {
    let index = TitleIndex::new(server);
    let mut matches = BTreeMap::new();
    for game in games {
        if let Some(game_id) = index.lookup(&game.name) {
            matches.insert(game.external_id.clone(), game_id);
        }
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-launchers-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn epic_dat() -> &'static str {
        r#"{
  "InstallationList": [
    {
      "InstallLocation": "C:\\Program Files\\Epic Games\\Fortnite",
      "NamespaceId": "fn",
      "ItemId": "4fe75bbc5a674f4f9b356b5c90567da5",
      "ArtifactId": "Fortnite",
      "AppVersion": "++Fortnite+Release-27.11",
      "AppName": "Fortnite"
    },
    { "InstallLocation": "D:\\Epic\\AlanWake2", "AppName": "AlanWake2" },
    { "InstallLocation": "D:\\Epic\\Broken", "AppName": "" }
  ]
}"#
    }

    #[test]
    fn epic_installed_list_is_parsed() {
        let entries = parse_epic_installed(epic_dat()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].app_name, "Fortnite");
        assert_eq!(entries[0].namespace.as_deref(), Some("fn"));
        assert_eq!(
            entries[0].catalog_item_id.as_deref(),
            Some("4fe75bbc5a674f4f9b356b5c90567da5")
        );
        assert_eq!(entries[0].display_name, "Fortnite");
        assert_eq!(
            entries[1].install_location.as_deref(),
            Some("D:\\Epic\\AlanWake2")
        );
        assert!(entries[1].namespace.is_none());
    }

    #[test]
    fn epic_installed_accepts_a_bare_array_and_reports_malformed_json() {
        let entries =
            parse_epic_installed(r#"[{"AppName":"Hades","InstallLocation":"/games/hades"}]"#)
                .unwrap();
        assert_eq!(entries.len(), 1);
        assert!(parse_epic_installed("{not json").is_err());
        assert!(parse_epic_installed(r#"{"Other":[]}"#).is_err());
    }

    #[test]
    fn epic_uri_uses_the_catalog_triplet_when_known() {
        let entry = EpicEntry {
            app_name: "Fortnite".into(),
            display_name: "Fortnite".into(),
            install_location: None,
            namespace: Some("fn".into()),
            catalog_item_id: Some("4fe75bbc5a674f4f9b356b5c90567da5".into()),
        };
        assert_eq!(
            epic_uri(&entry),
            "com.epicgames.launcher://apps/fn%3A4fe75bbc5a674f4f9b356b5c90567da5%3AFortnite?action=launch&silent=true"
        );
        let bare = EpicEntry {
            app_name: "Hades".into(),
            ..EpicEntry::default()
        };
        assert_eq!(
            epic_uri(&bare),
            "com.epicgames.launcher://apps/Hades?action=launch&silent=true"
        );
    }

    #[test]
    fn epic_manifests_skip_non_games_and_read_ids() {
        let manifest = r#"{
  "FormatVersion": 0,
  "AppName": "Sugar",
  "DisplayName": "Sugar",
  "InstallLocation": "C:\\Epic\\Sugar",
  "CatalogNamespace": "abc",
  "CatalogItemId": "def",
  "AppCategories": ["public", "games"]
}"#;
        let entry = parse_epic_manifest(manifest).unwrap().unwrap();
        assert_eq!(entry.namespace.as_deref(), Some("abc"));
        assert_eq!(entry.catalog_item_id.as_deref(), Some("def"));
        assert!(
            parse_epic_manifest(&manifest.replace("\"games\"", "\"plugins\""))
                .unwrap()
                .is_none()
        );
        assert!(parse_epic_manifest("nope").is_err());
        assert!(parse_epic_manifest(r#"{"AppCategories":["games"]}"#)
            .unwrap()
            .is_none());
    }

    const GOG_REGISTRY: &str = r#"
HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\GOG.com\Games\1207666623
    gameName    REG_SZ    The Witcher 3: Wild Hunt
    path    REG_SZ    C:\Games\The Witcher 3
    exe    REG_SZ    C:\Games\The Witcher 3\bin\x64\witcher3.exe
    launchCommand    REG_SZ    "C:\Games\The Witcher 3\bin\x64\witcher3.exe"

HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\GOG.com\Games\1423049311
    gameName    REG_SZ    Cyberpunk 2077
    path    REG_SZ    D:\GOG\Cyberpunk 2077
"#;

    #[test]
    fn gog_registry_entries_become_games() {
        let games = parse_gog_registry(GOG_REGISTRY).unwrap();
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].external_id, "1207666623");
        assert_eq!(games[0].name, "The Witcher 3: Wild Hunt");
        assert_eq!(
            games[0].install_dir.as_deref(),
            Some("C:\\Games\\The Witcher 3")
        );
        assert_eq!(
            games[0].launch_uri.as_deref(),
            Some("goggalaxy://openGameView/1207666623")
        );
        assert!(games[0].issue.is_none());
    }

    #[test]
    fn registry_output_tolerates_noise_and_empty_keys() {
        let keys =
            parse_registry_output("garbage\n\nHKCU\\Software\\Test\n    Value    REG_SZ    a  b\n");
        assert_eq!(keys.len(), 1);
        assert_eq!(
            keys[0].values.get("Value").map(String::as_str),
            Some("a  b")
        );
        assert!(parse_gog_registry("").unwrap().is_empty());
        assert!(parse_gog_registry("HKLM\\X\n    path    REG_SZ    C:\\x\n")
            .unwrap()
            .is_empty());

        // A value name may contain REG_ without being a type column.
        let keys = parse_registry_output(
            "HKEY_CURRENT_USER\\Software\\Test\n    Product REG_ID    REG_SZ    kept\n    Count    REG_DWORD    0x4\n",
        );
        assert_eq!(
            keys[0].values.get("Product REG_ID").map(String::as_str),
            Some("kept")
        );
        assert_eq!(keys[0].values.get("Count").map(String::as_str), Some("0x4"));
    }

    const EA_REGISTRY: &str = r#"
HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Electronic Arts\EA Games\Battlefield 4
    DisplayName    REG_SZ    Battlefield 4
    Install Dir    REG_SZ    C:\Program Files (x86)\Origin Games\Battlefield 4
    Content ID    REG_SZ    Origin.OFR.50.0000557

HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Electronic Arts\EA Games\Mass Effect
    Install Dir    REG_SZ    D:\EA\Mass Effect
"#;

    #[test]
    fn ea_registry_maps_offer_ids_and_flags_the_rest() {
        let games = parse_ea_registry(EA_REGISTRY).unwrap();
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].name, "Battlefield 4");
        assert_eq!(
            games[0].launch_uri.as_deref(),
            Some("origin://launchgame/Origin.OFR.50.0000557")
        );
        assert!(games[0].issue.is_none());
        assert_eq!(games[1].name, "Mass Effect");
        assert!(games[1].launch_uri.is_none());
        assert!(games[1].issue.is_some());
    }

    const UBISOFT_REGISTRY: &str = r#"
HKEY_CURRENT_USER\Software\Ubisoft\Launcher\Installs\635
    InstallDir    REG_SZ    C:\Program Files (x86)\Ubisoft\Ubisoft Game Launcher\games\Far Cry 5
    Language    REG_SZ    en-US

HKEY_CURRENT_USER\Software\Ubisoft\Launcher\Installs\NotAnId
    InstallDir    REG_SZ    D:\Ubisoft\Broken
"#;

    #[test]
    fn ubisoft_registry_uses_the_install_id() {
        let games = parse_ubisoft_registry(UBISOFT_REGISTRY).unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].external_id, "635");
        assert_eq!(games[0].name, "Far Cry 5");
        assert_eq!(games[0].launch_uri.as_deref(), Some("uplay://launch/635/0"));
    }

    #[test]
    fn start_menu_keeps_packaged_games_and_drops_the_os() {
        let json = r#"[
  {"Name":"Settings","AppID":"Microsoft.Windows.Settings_8wekyb3d8bbwe!Settings"},
  {"Name":"Xbox","AppID":"Microsoft.GamingApp_8wekyb3d8bbwe!Microsoft.Xbox.App"},
  {"Name":"Microsoft Flight Simulator","AppID":"Microsoft.FlightSimulator_8wekyb3d8bbwe!App"},
  {"Name":"Minecraft Launcher","AppID":"Microsoft.MinecraftUWP_8wekyb3d8bbwe!App"},
  {"Name":"Notepad","AppID":"C:\\Windows\\notepad.exe"},
  {"Name":"Noisy","AppID":"Microsoft.Something!  "}
]"#;
        let apps = parse_start_apps(json).unwrap();
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].name, "Microsoft Flight Simulator");
        assert_eq!(apps[1].name, "Minecraft Launcher");
        assert_eq!(
            shell_apps_uri(&apps[0].app_id),
            "shell:AppsFolder\\Microsoft.FlightSimulator_8wekyb3d8bbwe!App"
        );
    }

    #[test]
    fn start_menu_accepts_a_single_object() {
        let json = r#"{"Name":"Hades","AppID":"Supergiant.Hades_123!App"}"#;
        let apps = parse_start_apps(json).unwrap();
        assert_eq!(apps.len(), 1);
        assert!(parse_start_apps("\"not json\"").is_err());
    }

    #[test]
    fn appx_packages_keep_store_games_only() {
        let json = r#"[
  {"Name":"Microsoft.FlightSimulator","PackageFamilyName":"Microsoft.FlightSimulator_8wekyb3d8bbwe","InstallLocation":"C:\\Program Files\\WindowsApps\\Microsoft.FlightSimulator_1.0","SignatureKind":"Store"},
  {"Name":"Microsoft.SomeSdk","PackageFamilyName":"Microsoft.SomeSdk_8wekyb3d8bbwe","InstallLocation":"C:\\Program Files\\WindowsApps\\Microsoft.SomeSdk_1.0","SignatureKind":"Developer"},
  {"Name":"Settings","PackageFamilyName":"Microsoft.Windows.Settings_8wekyb3d8bbwe","InstallLocation":"C:\\Windows\\SystemApps","SignatureKind":"System"},
  {"Name":"Broken","PackageFamilyName":"NoLocation_1"}
]"#;
        let packages = parse_appx_packages(json).unwrap();
        // Store + no signature: a missing SignatureKind is kept, a Developer
        // package and the OS package family are dropped.
        assert_eq!(packages.len(), 2, "{packages:?}");
        assert_eq!(
            packages[0].package_family_name,
            "Microsoft.FlightSimulator_8wekyb3d8bbwe"
        );
        assert!(packages[1].install_location.is_none());
        assert!(parse_appx_packages("nope").is_err());
    }

    #[test]
    fn appx_manifests_yield_an_aumid() {
        let named = r#"<?xml version="1.0"?>
<Package>
  <Applications>
    <Application Id="App" Executable="game.exe" EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements DisplayName="Hades" Description="Hades" Square150x150Logo="Assets\Logo.png" />
    </Application>
  </Applications>
</Package>"#;
        let app = parse_appx_manifest(named, "Supergiant.Hades_123").unwrap();
        assert_eq!(app.app_id, "Supergiant.Hades_123!App");
        assert_eq!(app.name, "Hades");

        let resource = r#"<Package><Applications><Application Id="Game"><uap:VisualElements DisplayName="ms-resource:AppName" /></Application></Applications></Package>"#;
        let app = parse_appx_manifest(resource, "Publisher.Game_123").unwrap();
        assert_eq!(app.app_id, "Publisher.Game_123!Game");
        // A resource key is not a usable name: keep the package family.
        assert_eq!(app.name, "Publisher.Game_123");

        assert!(parse_appx_manifest("<Package />", "X.Y_1").is_none());
    }

    #[test]
    fn heroic_libraries_cover_both_cache_shapes() {
        let legendary = r#"{
  "games": [
    {"app_name":"Sugar","title":"Sugar","install":{"install_path":"/games/sugar","is_done":true}},
    {"app_name":"Uninstalled","title":"Uninstalled","install":{"is_done":false}}
  ]
}"#;
        let games = parse_heroic_library(legendary, Some("epic")).unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].external_id, "epic:Sugar");
        assert_eq!(
            games[0].launch_uri.as_deref(),
            Some("heroic://launch/epic/Sugar")
        );

        let sideload = r#"[{"app_name":"My Game","title":"My Game","install":{"install_path":"/games/mine"}}]"#;
        let games = parse_heroic_library(sideload, None).unwrap();
        assert_eq!(games[0].external_id, "My Game");
        assert_eq!(
            games[0].launch_uri.as_deref(),
            Some("heroic://launch/My%20Game")
        );
    }

    #[test]
    fn heroic_rejects_malformed_cache() {
        assert!(parse_heroic_library("{nope", Some("epic")).is_err());
        assert!(parse_heroic_library(r#"{"games":{}}"#, Some("epic")).is_err());
    }

    fn create_itch_db(path: &Path) {
        let connection = rusqlite::Connection::open(path).unwrap();
        connection
            .execute_batch(
                r#"
                CREATE TABLE games (id TEXT PRIMARY KEY, title TEXT);
                CREATE TABLE caves (id TEXT PRIMARY KEY, game_id TEXT, path TEXT);
                INSERT INTO games (id, title) VALUES ('42', 'Celeste');
                INSERT INTO caves (id, game_id, path) VALUES ('7', '42', '/games/celeste');
                INSERT INTO caves (id, game_id, path) VALUES ('8', NULL, '/games/unknown');
                "#,
            )
            .unwrap();
    }

    #[test]
    fn itch_database_joins_caves_and_games() {
        let root = temp_dir("itch");
        let database = root.join("butler.db");
        create_itch_db(&database);

        let games = read_itch_database(&database).unwrap();
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].name, "Celeste");
        assert_eq!(
            games[0].launch_uri.as_deref(),
            Some("itch://caves/7/launch")
        );
        // Without a matching game row the folder name is used.
        assert_eq!(games[1].name, "unknown");

        std::fs::remove_dir_all(&root).ok();
    }

    fn create_lutris_db(path: &Path) {
        let connection = rusqlite::Connection::open(path).unwrap();
        connection
            .execute_batch(
                r#"
                CREATE TABLE games (
                    id INTEGER PRIMARY KEY,
                    name TEXT,
                    slug TEXT,
                    directory TEXT,
                    runner TEXT,
                    installed INTEGER NOT NULL DEFAULT 1
                );
                INSERT INTO games (id, name, directory, runner, installed)
                    VALUES (3, 'Hades', '/games/hades', 'wine', 1);
                INSERT INTO games (id, name, directory, runner, installed)
                    VALUES (4, 'Gone', '/games/gone', 'linux', 0);
                "#,
            )
            .unwrap();
    }

    #[test]
    fn lutris_database_lists_only_installed_rows() {
        let root = temp_dir("lutris");
        let database = root.join("pga.db");
        create_lutris_db(&database);

        let games = read_lutris_database(&database).unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].name, "Hades");
        assert_eq!(games[0].launch_uri.as_deref(), Some("lutris:rungameid/3"));

        std::fs::remove_dir_all(&root).ok();
    }

    fn create_gog_db(path: &Path) {
        let connection = rusqlite::Connection::open(path).unwrap();
        connection
            .execute_batch(
                r#"
                CREATE TABLE Products (productId TEXT PRIMARY KEY, title TEXT, installationPath TEXT);
                INSERT INTO Products VALUES ('1207666623', 'The Witcher 3', '/games/witcher3');
                INSERT INTO Products VALUES ('1423049311', 'Cyberpunk 2077', NULL);
                "#,
            )
            .unwrap();
    }

    #[test]
    fn gog_database_is_read_by_column_name() {
        let root = temp_dir("gog");
        let database = root.join("galaxy-2.0.db");
        create_gog_db(&database);

        let games = read_gog_database(&database).unwrap();
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].name, "The Witcher 3");
        assert_eq!(games[0].install_dir.as_deref(), Some("/games/witcher3"));
        assert!(games[1].install_dir.is_none());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_or_empty_databases_do_not_fail() {
        let root = temp_dir("empty-db");
        let empty = root.join("empty.db");
        rusqlite::Connection::open(&empty).unwrap();
        assert!(read_itch_database(&empty).unwrap().is_empty());
        assert!(read_lutris_database(&empty).unwrap().is_empty());
        assert!(read_gog_database(&empty).unwrap().is_empty());
        assert!(read_itch_database(&root.join("absent.db")).is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn local_ids_are_stable_and_distinct() {
        let a = hash_local_id(Launcher::Epic, "Sugar");
        let b = hash_local_id(Launcher::Epic, "Sugar");
        let c = hash_local_id(Launcher::Gog, "Sugar");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!((-2_000_000_000..=-1_000_000_000).contains(&a));
        assert_eq!(
            DiscoveredGame {
                source: Launcher::Steam,
                external_id: "440".into(),
                name: "Team Fortress 2".into(),
                install_dir: None,
                launch_uri: None,
                size_bytes: 0,
                last_played: None,
                detail: None,
                issue: None,
            }
            .local_id(),
            steam::local_game_id(440)
        );
    }

    #[test]
    fn launch_uris_keep_their_scheme_and_reject_files() {
        assert_eq!(
            validate_launch_uri(" steam://rungameid/440 ").unwrap(),
            "steam://rungameid/440"
        );
        assert!(validate_launch_uri("steam-rungameid-440").is_err());
        assert!(validate_launch_uri("file:///etc/passwd").is_err());
        assert!(validate_launch_uri("javascript:alert(1)").is_err());
        assert!(validate_launch_uri("").is_err());
        assert!(validate_launch_uri("x:y\nz").is_err());
    }

    fn discovered(
        source: Launcher,
        external_id: &str,
        name: &str,
        install_dir: Option<&str>,
        uri: &str,
    ) -> DiscoveredGame {
        DiscoveredGame {
            source,
            external_id: external_id.into(),
            name: name.into(),
            install_dir: install_dir.map(str::to_string),
            launch_uri: Some(uri.into()),
            size_bytes: 0,
            last_played: None,
            detail: None,
            issue: None,
        }
    }

    #[test]
    fn import_resolution_prefers_manual_mapping_and_uri() {
        let games = vec![
            discovered(
                Launcher::Epic,
                "Sugar",
                "Sugar",
                Some("/games/sugar"),
                "com.epicgames.launcher://apps/Sugar?action=launch",
            ),
            DiscoveredGame {
                launch_uri: None,
                ..discovered(
                    Launcher::Ea,
                    "MassEffect",
                    "Mass Effect",
                    Some("/games/me"),
                    "",
                )
            },
        ];
        let mut matched = BTreeMap::new();
        matched.insert("Sugar".to_string(), 12i64);

        let entry = ImportEntry {
            source: "epic".into(),
            external_id: "Sugar".into(),
            game_id: None,
            launch_uri: None,
        };
        let target = resolve_import(&entry, &games, &matched, &LauncherSet::all()).unwrap();
        assert_eq!(target.game_id, Some(12));
        assert_eq!(
            target.launch_uri,
            "com.epicgames.launcher://apps/Sugar?action=launch"
        );

        let manual = ImportEntry {
            source: "epic".into(),
            external_id: "Sugar".into(),
            game_id: Some(99),
            launch_uri: Some("com.epicgames.launcher://apps/x?action=launch".into()),
        };
        let target = resolve_import(&manual, &games, &matched, &LauncherSet::all()).unwrap();
        assert_eq!(target.game_id, Some(99));

        let missing = ImportEntry {
            source: "epic".into(),
            external_id: "Nope".into(),
            game_id: None,
            launch_uri: None,
        };
        assert!(
            resolve_import(&missing, &games, &matched, &LauncherSet::all())
                .unwrap_err()
                .contains("no longer installed")
        );

        let no_uri = ImportEntry {
            source: "ea".into(),
            external_id: "MassEffect".into(),
            game_id: None,
            launch_uri: None,
        };
        assert!(
            resolve_import(&no_uri, &games, &matched, &LauncherSet::all())
                .unwrap_err()
                .contains("needs a launch URI")
        );

        let negative = ImportEntry {
            source: "epic".into(),
            external_id: "Sugar".into(),
            game_id: Some(-4),
            launch_uri: None,
        };
        assert!(
            resolve_import(&negative, &games, &matched, &LauncherSet::all())
                .unwrap_err()
                .contains("positive game id")
        );
    }

    #[test]
    fn titles_match_with_edition_suffixes() {
        let games = vec![
            discovered(Launcher::Epic, "a", "Celeste", None, "x:y"),
            discovered(Launcher::Gog, "b", "Hades II", None, "x:y"),
            discovered(Launcher::Itch, "c", "Doom", None, "x:y"),
        ];
        let server = vec![
            (1, "Celeste Game of the Year Edition".to_string()),
            (2, "Hades II".to_string()),
            (3, "Doom".to_string()),
            (4, "DOOM".to_string()),
        ];
        let matched = match_titles(&games, &server);
        assert_eq!(matched.get("a"), Some(&1));
        assert_eq!(matched.get("b"), Some(&2));
        // Ambiguous server titles are never guessed.
        assert_eq!(matched.get("c"), None);
    }

    #[test]
    fn scan_reads_linux_launchers_and_dedupes_by_folder() {
        let root = temp_dir("scan");
        let config = root.join("config");
        let data = root.join("data");
        std::fs::create_dir_all(config.join("heroic/store_cache")).unwrap();
        std::fs::create_dir_all(data.join("lutris")).unwrap();
        std::fs::create_dir_all(root.join("games/sugar")).unwrap();
        std::fs::create_dir_all(root.join("games/hades")).unwrap();

        std::fs::write(
            config.join("heroic/store_cache/legendary_library.json"),
            format!(
                r#"{{"games":[{{"app_name":"Sugar","title":"Sugar","install":{{"install_path":"{}","is_done":true}}}}]}}"#,
                root.join("games/sugar").display()
            ),
        )
        .unwrap();
        // The same Epic install is also reported by the GOG cache: the folder
        // dedupe keeps a single entry.
        std::fs::write(
            config.join("heroic/store_cache/gog_library.json"),
            format!(
                r#"{{"games":[{{"app_name":"sugar-gog","title":"Sugar","install":{{"install_path":"{}","is_done":true}}}}]}}"#,
                root.join("games/sugar").display()
            ),
        )
        .unwrap();
        create_lutris_db(&data.join("lutris/pga.db"));
        // Lutris points at /games/hades, rewrite the fixture to a real folder.
        {
            let connection = rusqlite::Connection::open(data.join("lutris/pga.db")).unwrap();
            connection
                .execute(
                    "UPDATE games SET directory = ?1 WHERE id = 3",
                    [root.join("games/hades").display().to_string()],
                )
                .unwrap();
        }

        let scan = scan(
            &ScanRoots {
                config_home: Some(config),
                data_home: Some(data),
                home: Some(root.clone()),
                ..ScanRoots::default()
            },
            &LauncherSet::all(),
        );

        assert_eq!(scan.games.len(), 2, "{:?}", scan.games);
        assert_eq!(scan.games[0].source, Launcher::Heroic);
        assert_eq!(scan.games[0].name, "Sugar");
        assert_eq!(scan.games[1].source, Launcher::Lutris);
        assert_eq!(scan.games[1].name, "Hades");
        assert!(scan
            .sources
            .iter()
            .any(|source| source.id == Launcher::Lutris && source.available));
        assert!(scan
            .sources
            .iter()
            .any(|source| source.id == Launcher::Xbox && !source.available));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_launcher_folder_is_a_warning_not_a_game() {
        let root = temp_dir("missing");
        let config = root.join("config");
        std::fs::create_dir_all(config.join("heroic/store_cache")).unwrap();
        std::fs::write(
            config.join("heroic/store_cache/legendary_library.json"),
            r#"{"games":[{"app_name":"Gone","title":"Gone","install":{"install_path":"/does/not/exist","is_done":true}}]}"#,
        )
        .unwrap();

        let scan = scan(
            &ScanRoots {
                config_home: Some(config),
                home: Some(root.clone()),
                ..ScanRoots::default()
            },
            &LauncherSet::all(),
        );
        assert!(scan.games.is_empty());
        assert!(scan
            .warnings
            .iter()
            .any(|warning| warning.contains("Gone") && warning.contains("missing")));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn malformed_launcher_files_are_warnings() {
        let root = temp_dir("malformed");
        let config = root.join("config");
        std::fs::create_dir_all(config.join("heroic/store_cache")).unwrap();
        std::fs::write(
            config.join("heroic/store_cache/legendary_library.json"),
            "{not json",
        )
        .unwrap();

        let scan = scan(
            &ScanRoots {
                config_home: Some(config),
                home: Some(root.clone()),
                ..ScanRoots::default()
            },
            &LauncherSet::all(),
        );
        assert!(scan.games.is_empty());
        assert!(scan
            .warnings
            .iter()
            .any(|warning| warning.contains("Could not parse")));

        std::fs::remove_dir_all(&root).ok();
    }

    /* --------------------- choosing the scanned sources (#155) ------------- */

    /// Fixture with one Epic, one Heroic and one Lutris game in existing
    /// folders, so a scan with every source enabled finds three games.
    fn mixed_fixture(label: &str) -> (PathBuf, ScanRoots) {
        let root = temp_dir(label);
        let config = root.join("config");
        let data = root.join("data");
        let program_data = root.join("program-data");
        for dir in [
            config.join("heroic/store_cache"),
            data.join("lutris"),
            program_data.join("Epic/UnrealEngineLauncher"),
            root.join("games/sugar"),
            root.join("games/epicgame"),
            root.join("games/hades"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(
            config.join("heroic/store_cache/legendary_library.json"),
            format!(
                r#"{{"games":[{{"app_name":"Sugar","title":"Sugar","install":{{"install_path":"{}","is_done":true}}}}]}}"#,
                root.join("games/sugar").display()
            ),
        )
        .unwrap();
        std::fs::write(
            program_data.join("Epic/UnrealEngineLauncher/LauncherInstalled.dat"),
            format!(
                r#"{{"InstallationList":[{{"InstallLocation":"{}","AppName":"EpicGame"}}]}}"#,
                root.join("games/epicgame").display()
            ),
        )
        .unwrap();
        create_lutris_db(&data.join("lutris/pga.db"));
        {
            let connection = rusqlite::Connection::open(data.join("lutris/pga.db")).unwrap();
            connection
                .execute(
                    "UPDATE games SET directory = ?1 WHERE id = 3",
                    [root.join("games/hades").display().to_string()],
                )
                .unwrap();
        }
        let roots = ScanRoots {
            program_data: Some(program_data),
            config_home: Some(config),
            data_home: Some(data),
            home: Some(root.clone()),
            ..ScanRoots::default()
        };
        (root, roots)
    }

    #[test]
    fn every_source_is_scanned_by_default() {
        let (root, roots) = mixed_fixture("all-sources");
        let scan = scan(&roots, &LauncherSet::all());

        assert_eq!(scan.games.len(), 3, "{:?}", scan.games);
        assert!(scan.roots.iter().any(|read| read.contains("heroic")));
        assert!(scan.sources.iter().all(|source| !source.disabled));
        assert_eq!(scan.sources.len(), Launcher::ALL.len());
        // Every source was read: the three with a game report it, the others
        // report "not detected" or "no games" instead of being skipped.
        for id in [Launcher::Epic, Launcher::Heroic, Launcher::Lutris] {
            let source = scan.sources.iter().find(|source| source.id == id).unwrap();
            assert!(source.available, "{id:?} should be available");
            assert_eq!(source.found, 1, "{id:?} found one game");
        }
        let epic = scan
            .sources
            .iter()
            .find(|source| source.id == Launcher::Epic)
            .unwrap();
        assert_eq!(epic.label, "Epic Games");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn disabled_sources_are_never_probed() {
        let (root, roots) = mixed_fixture("disabled-probe");
        // The same fixture is readable with every source on (asserted above):
        // with only Steam enabled nothing at all may be touched.
        let scan = scan(&roots, &LauncherSet::from_ids(["steam"]));

        assert!(scan.games.is_empty(), "{:?}", scan.games);
        assert!(scan.roots.is_empty(), "{:?}", scan.roots);
        assert!(scan.warnings.is_empty(), "{:?}", scan.warnings);

        let disabled: Vec<Launcher> = scan
            .sources
            .iter()
            .filter(|source| source.disabled)
            .map(|source| source.id)
            .collect();
        assert_eq!(disabled.len(), Launcher::ALL.len() - 1);
        assert!(!disabled.contains(&Launcher::Steam));
        for source in scan.sources.iter().filter(|source| source.disabled) {
            assert!(!source.available);
            assert_eq!(source.found, 0);
            assert_eq!(source.detail, "Disabled in Settings → Integrations");
        }

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_disabled_source_is_skipped_while_the_others_still_scan() {
        let (root, roots) = mixed_fixture("one-disabled");
        let enabled: Vec<&str> = Launcher::ALL
            .iter()
            .filter(|source| **source != Launcher::Heroic)
            .map(|source| source.id())
            .collect();
        let scan = scan(&roots, &LauncherSet::from_ids(enabled));

        assert!(!scan.roots.iter().any(|read| read.contains("heroic")));
        assert!(scan
            .games
            .iter()
            .all(|game| game.source != Launcher::Heroic));
        assert_eq!(
            scan.games.len(),
            2,
            "Epic and Lutris still scan: {:?}",
            scan.games
        );

        let heroic = scan
            .sources
            .iter()
            .find(|source| source.id == Launcher::Heroic)
            .unwrap();
        assert!(heroic.disabled);
        assert_eq!(heroic.found, 0);
        // The last scan read Heroic, so this is "disabled", not "not detected".
        assert!(!heroic.available);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_disabled_source_serializes_its_status() {
        let source = disabled_status(Launcher::Xbox);
        let json = serde_json::to_value(&source).unwrap();
        assert_eq!(json["id"], "xbox");
        assert_eq!(json["label"], "Xbox / Microsoft Store");
        assert_eq!(json["disabled"], true);
        assert_eq!(json["available"], false);
        assert_eq!(json["found"], 0);

        let detected = source_status(Launcher::Steam, true, "/steam");
        let json = serde_json::to_value(&detected).unwrap();
        assert_eq!(json["disabled"], false);
        assert_eq!(json["available"], true);
    }

    #[test]
    fn source_ids_are_canonicalized_for_settings() {
        assert_eq!(
            canonical_ids(["  Epic ", "epic", "uplay", "nope", "lutris"]),
            vec!["epic", "ubisoft", "lutris"]
        );
        assert!(canonical_ids(Vec::<String>::new()).is_empty());
        assert_eq!(canonical_ids(["steam"]), vec!["steam"]);

        assert_eq!(LauncherSet::all().ids().len(), 9);
        assert!(LauncherSet::from_ids(Vec::<String>::new()).ids().is_empty());
        assert_eq!(
            LauncherSet::from_ids(["lutris", "steam"]).ids(),
            vec!["steam", "lutris"]
        );
        assert!(LauncherSet::from_ids(["unknown"]).ids().is_empty());
    }

    #[test]
    fn import_subset_only_honours_enabled_sources() {
        let games = vec![
            discovered(
                Launcher::Epic,
                "Sugar",
                "Sugar",
                None,
                "com.epicgames.launcher://apps/Sugar?action=launch",
            ),
            discovered(
                Launcher::Gog,
                "1207666623",
                "The Witcher 3",
                None,
                "goggalaxy://openGameView/1207666623",
            ),
        ];
        let matched = BTreeMap::new();
        let epic = ImportEntry {
            source: "epic".into(),
            external_id: "Sugar".into(),
            game_id: None,
            launch_uri: None,
        };
        let gog = ImportEntry {
            source: "gog".into(),
            external_id: "1207666623".into(),
            game_id: None,
            launch_uri: None,
        };

        // Only GOG is enabled: its entry imports, Epic's is refused.
        let enabled = LauncherSet::from_ids(["gog"]);
        assert!(resolve_import(&epic, &games, &matched, &enabled)
            .unwrap_err()
            .contains("Epic Games is disabled"));
        let target = resolve_import(&gog, &games, &matched, &enabled).unwrap();
        assert_eq!(target.source, Launcher::Gog);
        assert_eq!(target.launch_uri, "goggalaxy://openGameView/1207666623");

        // Nothing enabled: every entry is refused, even a valid one.
        assert!(resolve_import(
            &gog,
            &games,
            &matched,
            &LauncherSet::from_ids(Vec::<String>::new())
        )
        .is_err());

        // Re-enabling restores the import without any other change.
        assert!(resolve_import(&epic, &games, &matched, &LauncherSet::all()).is_ok());
    }

    #[test]
    fn steam_imports_keep_the_historical_local_id() {
        let game = discovered(
            Launcher::Steam,
            "440",
            "Team Fortress 2",
            Some("/steam/tf2"),
            "steam://rungameid/440",
        );
        assert_eq!(game.local_id(), -440);
        assert_eq!(
            steam::app_id_from_uri(game.launch_uri.as_deref().unwrap()),
            Some(440)
        );
    }
}
