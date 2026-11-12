//! Versioned save-root registry (#146).
//!
//! `data/save-roots.yaml` is embedded at build time and is the resolver used by
//! save discovery: per-OS known folders, engine layout defaults, Steam Cloud
//! ground-truth roots, exclusions and the caps that keep a scan shallow.
//! `data/save-overrides.yaml` holds optional per-game observations.
//!
//! The registry contains no per-game data. It is **not** a copy of Ludusavi or
//! PCGamingWiki data: those stay fetched at runtime by `saves_manifest`.
//!
//! Rules enforced here and documented in `docs/SAVES.md`:
//! - known folders are resolved through an injectable resolver (`KnownFolderInputs`),
//!   which a Windows build fills from `SHGetKnownFolderPath`. There is no
//!   `%USERPROFILE%\\Documents` concatenation;
//! - LocalLow derives from the resolved LocalAppData parent because Windows has
//!   no `%LOCALAPPDATA%Low` variable;
//! - enumeration is shallow (<= `limits.max_depth` below a root), bounded by
//!   `limits.max_entries`, and never follows symlinks, junctions or reparse
//!   points;
//! - at or below `limits.whitelist_min_depth`, only literal segments or a
//!   game-title match are enumerated, never a broad wildcard;
//! - no save file is opened. The only file read is Steam's `remotecache.vdf`,
//!   a bounded config file used as store ground truth.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::saves_detect::Platform;

/// Embedded resolver registry. Validated by the tests in this module.
pub const REGISTRY_YAML: &str = include_str!("../../../../data/save-roots.yaml");
/// Embedded optional per-game overrides.
pub const OVERRIDES_YAML: &str = include_str!("../../../../data/save-overrides.yaml");

/// Candidate source tags. `manifest` and `manual` are produced by `saves_detect`.
pub const SOURCE_KNOWN_FOLDER: &str = "known-folder";
pub const SOURCE_ENGINE: &str = "engine";
pub const SOURCE_STORE: &str = "store";
pub const SOURCE_OVERRIDE: &str = "override";
/// Confidence tags.
pub const CONFIDENCE_CERTAIN: &str = "certain";
pub const CONFIDENCE_LIKELY: &str = "likely";
pub const CONFIDENCE_GUESS: &str = "guess";

/// Root id reported for a Wine/Proton prefix candidate.
pub const ROOT_PROTON_PREFIX: &str = "proton_prefix";

/// Upper bound on one `remotecache.vdf` read.
const MAX_REMOTECACHE_BYTES: u64 = 2 * 1024 * 1024;
/// Upper bound on VDF tokens and nodes, independent of the file size.
const MAX_VDF_TOKENS: usize = 200_000;
const MAX_VDF_NODES: usize = 20_000;
/// Upper bound on warnings collected per discovery run.
const MAX_WARNINGS: usize = 24;

/* ------------------------------------------------------------------ data */

/// Root resolver data parsed from `data/save-roots.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveRegistry {
    pub version: u32,
    #[serde(default)]
    pub updated: String,
    #[serde(default)]
    pub provenance: Vec<String>,
    #[serde(default)]
    pub confidence: Vec<String>,
    pub limits: RegistryLimits,
    #[serde(default)]
    pub exclusions: Exclusions,
    pub known_folders: KnownFolderCatalog,
    pub store_roots: StoreRoots,
    #[serde(default)]
    pub engines: Vec<EngineSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryLimits {
    pub max_depth: usize,
    pub max_entries: usize,
    pub max_candidates: usize,
    pub whitelist_min_depth: usize,
    #[serde(default = "default_store_entries")]
    pub max_store_entries: usize,
}

fn default_store_entries() -> usize {
    5_000
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exclusions {
    #[serde(default)]
    pub directory_names: Vec<String>,
    #[serde(default)]
    pub directory_suffixes: Vec<String>,
    #[serde(default)]
    pub file_suffixes: Vec<String>,
}

impl Exclusions {
    fn excludes_directory(&self, name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        self.directory_names
            .iter()
            .any(|excluded| excluded.eq_ignore_ascii_case(&lower))
            || self
                .directory_suffixes
                .iter()
                .any(|suffix| lower.ends_with(&suffix.to_ascii_lowercase()))
    }

    fn excludes_file(&self, name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        self.file_suffixes
            .iter()
            .any(|suffix| lower.ends_with(&suffix.to_ascii_lowercase()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct KnownFolderCatalog {
    #[serde(default)]
    pub windows: BTreeMap<String, KnownFolderSpec>,
    #[serde(default)]
    pub linux: BTreeMap<String, KnownFolderSpec>,
    #[serde(default)]
    pub macos: BTreeMap<String, KnownFolderSpec>,
}

impl KnownFolderCatalog {
    pub fn for_platform(&self, platform: Platform) -> &BTreeMap<String, KnownFolderSpec> {
        match platform {
            Platform::Windows => &self.windows,
            Platform::Linux => &self.linux,
            Platform::Macos => &self.macos,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnownFolderSpec {
    #[serde(default)]
    pub folder_id: Option<String>,
    #[serde(default)]
    pub guid: Option<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub redirect_env: Vec<String>,
    #[serde(default)]
    pub fallback: Option<String>,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub api: bool,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreRoots {
    pub steam: SteamStoreSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteamStoreSpec {
    /// Steam Auto-Cloud root id -> registry root id.
    #[serde(default)]
    pub cloud_roots: BTreeMap<String, String>,
    pub prefix: String,
    pub remotecache: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineSpec {
    pub id: String,
    pub label: String,
    pub provenance: String,
    /// Candidate source: `engine` for an engine layout, `known-folder` for a
    /// title folder found directly under a real OS known folder.
    #[serde(default = "default_engine_source")]
    pub source: String,
    pub confidence: String,
    #[serde(default)]
    pub patterns: PlatformPatterns,
}

fn default_engine_source() -> String {
    SOURCE_ENGINE.to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformPatterns {
    #[serde(default)]
    pub windows: Vec<String>,
    #[serde(default)]
    pub linux: Vec<String>,
    #[serde(default)]
    pub macos: Vec<String>,
}

impl PlatformPatterns {
    pub fn for_platform(&self, platform: Platform) -> &[String] {
        match platform {
            Platform::Windows => &self.windows,
            Platform::Linux => &self.linux,
            Platform::Macos => &self.macos,
        }
    }
}

/// Optional per-game observations from `data/save-overrides.yaml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveOverrides {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub overrides: Vec<SaveOverride>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveOverride {
    pub title: String,
    #[serde(default)]
    pub steam_app_id: Option<u32>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub roots: Vec<OverrideRoot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverrideRoot {
    pub root: String,
    pub path: String,
}

/* ------------------------------------------------------------------ candidates */

/// One discovered candidate. Host paths never leave the machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveCandidate {
    pub path: String,
    pub kind: String,
    /// Known-folder id (`saved_games`, `documents`, `proton_prefix`, …).
    pub root_id: String,
    /// Path below that root.
    pub subpath: String,
    /// `override`, `store`, `known-folder`, `engine`, `manifest` or `manual`.
    pub source: String,
    /// `certain`, `likely` or `guess`.
    pub confidence: String,
    pub evidence: String,
    pub size_bytes: i64,
    pub last_modified: Option<String>,
    pub first_seen: Option<String>,
    pub last_verified: Option<String>,
    /// True once the user confirmed the candidate; then it is also a save path.
    pub confirmed: bool,
    /// True when the user dismissed the candidate; detection keeps that state.
    pub ignored: bool,
    /// Last automatic promotion decision (#180): `saving`, `waiting` or
    /// `skipped`. Empty on a fresh candidate, which reads as `waiting`.
    #[serde(default)]
    pub auto_status: String,
    /// Readable reason when `auto_status` is `skipped` (too broad, symlink, …).
    #[serde(default)]
    pub auto_reason: Option<String>,
}

impl SaveCandidate {
    pub fn is_guess(&self) -> bool {
        self.confidence == CONFIDENCE_GUESS
    }
}

/// A path that is archived without waiting for a session (#180): Steam ground
/// truth, a path the user already confirmed, and an explicit user override.
/// Manifest and manual paths never reach this layer: they are save paths
/// already.
pub fn is_high_confidence(candidate: &SaveCandidate) -> bool {
    candidate.confirmed || candidate.source == SOURCE_STORE || candidate.source == SOURCE_OVERRIDE
}

/// Candidates for one installed game, as returned by the Tauri commands.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveCandidateGroup {
    pub game_id: i64,
    pub title: String,
    pub candidates: Vec<SaveCandidate>,
}

/// Settings → Saves overview. Local only.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveCandidateOverview {
    pub registry_version: u32,
    pub max_depth: usize,
    pub max_entries: usize,
    pub groups: Vec<SaveCandidateGroup>,
    pub warnings: Vec<String>,
}

/* ------------------------------------------------------------------ known folders */

/// Injectable inputs for known-folder resolution. A Windows build fills `api`
/// from `SHGetKnownFolderPath`; tests fill it directly, so no test needs a real
/// Windows shell.
#[derive(Debug, Clone, Default)]
pub struct KnownFolderInputs {
    pub env: BTreeMap<String, PathBuf>,
    pub api: BTreeMap<String, PathBuf>,
}

impl KnownFolderInputs {
    /// Read the process environment and, on Windows, ask the shell API for the
    /// known folders that have one.
    #[allow(unused_mut, unused_variables)]
    pub fn from_process(platform: Platform) -> Self {
        let mut env = BTreeMap::new();
        for (key, value) in std::env::vars_os() {
            let key = key.to_string_lossy().to_string();
            if !key.is_empty() && !value.is_empty() {
                env.insert(key, PathBuf::from(value));
            }
        }
        let mut inputs = Self {
            env,
            api: BTreeMap::new(),
        };
        #[cfg(windows)]
        if platform == Platform::Windows {
            inputs.api = windows_known_folders();
        }
        inputs
    }
}

/// Resolved roots for one platform.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedFolders {
    roots: BTreeMap<String, PathBuf>,
}

impl ResolvedFolders {
    pub fn get(&self, id: &str) -> Option<PathBuf> {
        self.roots.get(id).cloned()
    }

    pub fn values(&self) -> impl Iterator<Item = &PathBuf> {
        self.roots.values()
    }

    /// Resolved roots with their ids, for per-root diagnostics (#158).
    pub fn iter(&self) -> impl Iterator<Item = (&String, &PathBuf)> {
        self.roots.iter()
    }

    pub fn insert(&mut self, id: impl Into<String>, path: PathBuf) {
        self.roots.insert(id.into(), path);
    }
}

/// Resolve every known folder defined for `platform`. Missing folders are
/// omitted, never invented except for the documented LocalLow derivation.
pub fn resolve_known_folders(
    catalog: &KnownFolderCatalog,
    platform: Platform,
    inputs: &KnownFolderInputs,
) -> ResolvedFolders {
    let specs = catalog.for_platform(platform);
    let mut resolved = ResolvedFolders::default();
    for id in specs.keys() {
        let mut stack = Vec::new();
        if let Some(path) = resolve_folder(id, specs, inputs, &mut resolved, &mut stack) {
            resolved.insert(id.clone(), path);
        }
    }
    resolved
}

fn resolve_folder(
    id: &str,
    specs: &BTreeMap<String, KnownFolderSpec>,
    inputs: &KnownFolderInputs,
    resolved: &mut ResolvedFolders,
    stack: &mut Vec<String>,
) -> Option<PathBuf> {
    if let Some(path) = resolved.get(id) {
        return Some(path);
    }
    if stack.iter().any(|name| name == id) {
        return None;
    }
    let spec = specs.get(id)?;
    stack.push(id.to_string());
    let mut result = inputs.api.get(id).cloned();

    if result.is_none() {
        for key in &spec.env {
            if let Some(path) = inputs.env.get(key) {
                result = Some(path.clone());
                break;
            }
        }
    }

    // OneDrive Known Folder Move / GPO redirect: the environment points at the
    // sync root and Documents lives below it. This is only used when the shell
    // API did not answer; it is not a `%USERPROFILE%\\Documents` guess.
    if result.is_none() {
        for key in &spec.redirect_env {
            if let Some(root) = inputs.env.get(key) {
                let candidate = root.join("Documents");
                if candidate.is_dir() {
                    result = Some(candidate);
                    break;
                }
            }
        }
    }

    if result.is_none() {
        if let Some(pattern) = &spec.pattern {
            if let Some(text) = render_tokens(pattern, |token| {
                resolve_folder(token, specs, inputs, resolved, stack)
                    .map(|path| path.to_string_lossy().to_string())
            }) {
                result = Some(normalize_lexically(Path::new(&text)));
            }
        }
    }

    if result.is_none() {
        if let Some(fallback) = &spec.fallback {
            if let Some(text) = render_tokens(fallback, |token| {
                resolve_folder(token, specs, inputs, resolved, stack)
                    .map(|path| path.to_string_lossy().to_string())
            }) {
                result = Some(normalize_lexically(Path::new(&text)));
            }
        }
    }

    stack.pop();
    if let Some(path) = result {
        resolved.insert(id.to_string(), path.clone());
        Some(path)
    } else {
        None
    }
}

/// Replace `{root}` tokens in a resolver template. Returns `None` when any
/// referenced root cannot be resolved.
fn render_tokens<F>(template: &str, mut resolve: F) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after.find('}')?;
        let token = &after[..close];
        out.push_str(&resolve(token)?);
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(windows)]
fn windows_known_folders() -> BTreeMap<String, PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{
        FOLDERID_Documents, FOLDERID_LocalAppData, FOLDERID_LocalAppDataLow, FOLDERID_Profile,
        FOLDERID_ProgramData, FOLDERID_Public, FOLDERID_RoamingAppData, FOLDERID_SavedGames,
        SHGetKnownFolderPath, KNOWN_FOLDER_FLAG,
    };

    let folders: [(&str, &windows::core::GUID); 8] = [
        ("profile", &FOLDERID_Profile),
        ("saved_games", &FOLDERID_SavedGames),
        ("documents", &FOLDERID_Documents),
        ("roaming_app_data", &FOLDERID_RoamingAppData),
        ("local_app_data", &FOLDERID_LocalAppData),
        ("local_app_data_low", &FOLDERID_LocalAppDataLow),
        ("program_data", &FOLDERID_ProgramData),
        ("public", &FOLDERID_Public),
    ];

    let mut resolved = BTreeMap::new();
    for (id, guid) in folders {
        // SAFETY: SHGetKnownFolderPath returns a NUL-terminated, CoTaskMem
        // allocation that stays valid until CoTaskMemFree is called below.
        unsafe {
            if let Ok(raw) = SHGetKnownFolderPath(guid, KNOWN_FOLDER_FLAG(0), None) {
                if let Ok(text) = raw.to_string() {
                    if !text.is_empty() {
                        resolved.insert(id.to_string(), PathBuf::from(text));
                    }
                }
                CoTaskMemFree(Some(raw.0 as *const core::ffi::c_void));
            }
        }
    }
    resolved
}

/// Known folders for this machine, from the embedded registry.
pub fn system_known_folders(platform: Platform) -> ResolvedFolders {
    resolve_known_folders(
        &registry().known_folders,
        platform,
        &KnownFolderInputs::from_process(platform),
    )
}

/* ------------------------------------------------------------------ loading */

static REGISTRY: OnceLock<SaveRegistry> = OnceLock::new();
static OVERRIDES: OnceLock<SaveOverrides> = OnceLock::new();

/// The embedded registry. Panics only if `data/save-roots.yaml` is invalid,
/// which the test suite catches before a release is built.
pub fn registry() -> &'static SaveRegistry {
    REGISTRY.get_or_init(|| {
        SaveRegistry::parse(REGISTRY_YAML).expect("data/save-roots.yaml must be valid")
    })
}

/// The embedded per-game overrides (empty by default).
pub fn overrides() -> &'static SaveOverrides {
    OVERRIDES.get_or_init(|| {
        parse_overrides(OVERRIDES_YAML).expect("data/save-overrides.yaml must be valid")
    })
}

impl SaveRegistry {
    pub fn parse(text: &str) -> Result<Self, String> {
        let registry: SaveRegistry = serde_yaml::from_str(text)
            .map_err(|error| format!("Invalid save registry: {error}"))?;
        registry.validate()?;
        Ok(registry)
    }

    fn validate(&self) -> Result<(), String> {
        if self.version == 0 {
            return Err("Invalid save registry: version must be >= 1.".to_string());
        }
        if !(1..=6).contains(&self.limits.max_depth) {
            return Err("Invalid save registry: limits.max_depth must be 1..=6.".to_string());
        }
        if self.limits.max_entries == 0 || self.limits.max_entries > 20_000 {
            return Err("Invalid save registry: limits.max_entries must be 1..=20000.".to_string());
        }
        if self.limits.max_candidates == 0 || self.limits.max_candidates > 500 {
            return Err(
                "Invalid save registry: limits.max_candidates must be 1..=500.".to_string(),
            );
        }
        if self.limits.whitelist_min_depth < 2 {
            return Err(
                "Invalid save registry: limits.whitelist_min_depth must be >= 2.".to_string(),
            );
        }
        if self.exclusions.directory_names.is_empty() || self.exclusions.file_suffixes.is_empty() {
            return Err("Invalid save registry: exclusions must not be empty.".to_string());
        }
        for tag in [
            SOURCE_KNOWN_FOLDER,
            SOURCE_ENGINE,
            SOURCE_STORE,
            SOURCE_OVERRIDE,
            "manifest",
        ] {
            if !self.provenance.iter().any(|value| value == tag) {
                return Err(format!(
                    "Invalid save registry: provenance is missing {tag}."
                ));
            }
        }
        for tag in [CONFIDENCE_CERTAIN, CONFIDENCE_LIKELY, CONFIDENCE_GUESS] {
            if !self.confidence.iter().any(|value| value == tag) {
                return Err(format!(
                    "Invalid save registry: confidence is missing {tag}."
                ));
            }
        }

        for id in ["documents", "saved_games", "local_app_data_low", "profile"] {
            if !self.known_folders.windows.contains_key(id) {
                return Err(format!(
                    "Invalid save registry: windows known folder {id} is missing."
                ));
            }
        }
        for id in ["home", "xdg_data", "xdg_config", "xdg_state"] {
            if !self.known_folders.linux.contains_key(id) {
                return Err(format!(
                    "Invalid save registry: linux known folder {id} is missing."
                ));
            }
        }
        for id in ["home", "application_support", "containers"] {
            if !self.known_folders.macos.contains_key(id) {
                return Err(format!(
                    "Invalid save registry: macos known folder {id} is missing."
                ));
            }
        }

        for (id, spec) in &self.known_folders.windows {
            if spec.api {
                let folder_id = spec.folder_id.as_deref().unwrap_or_default();
                if !folder_id.starts_with("FOLDERID_") {
                    return Err(format!(
                        "Invalid save registry: windows.{id} needs a FOLDERID_ id."
                    ));
                }
                if !spec.guid.as_deref().map(valid_guid).unwrap_or(false) {
                    return Err(format!(
                        "Invalid save registry: windows.{id} needs a valid GUID."
                    ));
                }
            }
        }
        // These two folders are the classic wrong expansions; keep the data
        // honest so a regression is caught by the test suite.
        if !self.known_folders.windows["saved_games"].env.is_empty() {
            return Err(
                "Invalid save registry: windows.saved_games has no environment variable."
                    .to_string(),
            );
        }
        if !self.known_folders.windows["local_app_data_low"]
            .env
            .is_empty()
        {
            return Err(
                "Invalid save registry: windows.local_app_data_low has no environment variable."
                    .to_string(),
            );
        }

        for engine in &self.engines {
            if engine.id.trim().is_empty() || engine.label.trim().is_empty() {
                return Err("Invalid save registry: an engine is missing an id or label.".into());
            }
            if !self.provenance.iter().any(|tag| tag == &engine.provenance)
                || engine.provenance != SOURCE_ENGINE
            {
                return Err(format!(
                    "Invalid save registry: engine {} has an unknown provenance.",
                    engine.id
                ));
            }
            if !self.confidence.iter().any(|tag| tag == &engine.confidence) {
                return Err(format!(
                    "Invalid save registry: engine {} has an unknown confidence.",
                    engine.id
                ));
            }
            if !matches!(engine.source.as_str(), SOURCE_ENGINE | SOURCE_KNOWN_FOLDER) {
                return Err(format!(
                    "Invalid save registry: engine {} has an unknown source.",
                    engine.id
                ));
            }
            let platforms = [
                (Platform::Windows, &engine.patterns.windows),
                (Platform::Linux, &engine.patterns.linux),
                (Platform::Macos, &engine.patterns.macos),
            ];
            if platforms.iter().all(|(_, patterns)| patterns.is_empty()) {
                return Err(format!(
                    "Invalid save registry: engine {} has no pattern.",
                    engine.id
                ));
            }
            for (platform, patterns) in platforms {
                for pattern in patterns {
                    self.validate_pattern(platform, pattern).map_err(|error| {
                        format!("Invalid save registry: engine {} {error}", engine.id)
                    })?;
                }
            }
        }

        for root in self.store_roots.steam.cloud_roots.values() {
            if !self.known_folders.windows.contains_key(root)
                && !self.known_folders.linux.contains_key(root)
                && !self.known_folders.macos.contains_key(root)
            {
                return Err(format!(
                    "Invalid save registry: store root {root} is not a known folder."
                ));
            }
        }
        for template in [
            self.store_roots.steam.prefix.as_str(),
            self.store_roots.steam.remotecache.as_str(),
        ] {
            if !template.contains("{steam_root}") || !template.contains("{appid}") {
                return Err(
                    "Invalid save registry: store templates need {steam_root} and {appid}."
                        .to_string(),
                );
            }
            for token in tokens(template) {
                if !matches!(
                    token,
                    "steam_root" | "appid" | "user_id" | "proton_drive_c" | "prefix"
                ) {
                    return Err(format!(
                        "Invalid save registry: unknown store placeholder {{{token}}}."
                    ));
                }
            }
        }
        if !self.store_roots.steam.remotecache.contains("{user_id}") {
            return Err(
                "Invalid save registry: the Steam remotecache template needs {user_id}."
                    .to_string(),
            );
        }
        Ok(())
    }

    fn validate_pattern(&self, platform: Platform, pattern: &str) -> Result<(), String> {
        let segments: Vec<&str> = pattern.split('/').filter(|part| !part.is_empty()).collect();
        let Some(root) = segments.first().and_then(|segment| placeholder(segment)) else {
            return Err(format!(
                "pattern {pattern} must start with a root placeholder."
            ));
        };
        let specs = self.known_folders.for_platform(platform);
        if !specs.contains_key(root) && !matches!(root, "proton_drive_c" | "prefix") {
            return Err(format!(
                "pattern {pattern} uses an unknown {platform:?} root {{{root}}}."
            ));
        }
        let depth = segments.len().saturating_sub(1);
        if depth == 0 || depth > self.limits.max_depth {
            return Err(format!(
                "pattern {pattern} is {depth} level(s) below its root; the cap is {}.",
                self.limits.max_depth
            ));
        }
        for segment in &segments[1..] {
            for token in tokens(segment) {
                if !matches!(
                    token,
                    "product"
                        | "project"
                        | "game"
                        | "company"
                        | "publisher"
                        | "windows_user"
                        | "bundle"
                ) {
                    return Err(format!(
                        "pattern {pattern} uses the unknown placeholder {{{token}}}."
                    ));
                }
            }
        }
        Ok(())
    }
}

fn valid_guid(value: &str) -> bool {
    let trimmed = value.trim().trim_start_matches('{').trim_end_matches('}');
    trimmed.len() == 36
        && trimmed.chars().enumerate().all(|(index, character)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                character == '-'
            } else {
                character.is_ascii_hexdigit()
            }
        })
}

/// Parse and validate the optional overrides file.
pub fn parse_overrides(text: &str) -> Result<SaveOverrides, String> {
    let overrides: SaveOverrides =
        serde_yaml::from_str(text).map_err(|error| format!("Invalid save overrides: {error}"))?;
    if overrides.version == 0 {
        return Err("Invalid save overrides: version must be >= 1.".to_string());
    }
    let registry = registry();
    for entry in &overrides.overrides {
        if entry.title.trim().is_empty() {
            return Err("Invalid save overrides: an entry has no title.".to_string());
        }
        for root in &entry.roots {
            let known = registry.known_folders.windows.contains_key(&root.root)
                || registry.known_folders.linux.contains_key(&root.root)
                || registry.known_folders.macos.contains_key(&root.root);
            if !known {
                return Err(format!(
                    "Invalid save overrides: {} uses unknown root {}.",
                    entry.title, root.root
                ));
            }
            let subpath = Path::new(&root.path);
            if subpath.is_absolute()
                || subpath
                    .components()
                    .any(|component| matches!(component, Component::ParentDir))
            {
                return Err(format!(
                    "Invalid save overrides: {} has an unsafe subpath.",
                    entry.title
                ));
            }
        }
    }
    Ok(overrides)
}

/* ------------------------------------------------------------------ discovery */

/// Everything discovery needs. Built by `saves_detect::detect` from `DetectEnv`.
pub struct DiscoveryInputs<'a> {
    pub platform: Platform,
    pub user_name: &'a str,
    pub folders: &'a ResolvedFolders,
    pub prefix: Option<&'a Path>,
    pub title: &'a str,
    pub steam_app_id: Option<u32>,
    pub steam_roots: &'a [PathBuf],
    pub steam_user_ids: &'a [String],
    /// Allowlisted, canonical roots. A candidate outside them is dropped.
    pub roots: &'a [PathBuf],
    pub now: &'a str,
    pub registry: &'a SaveRegistry,
    pub overrides: &'a SaveOverrides,
}

pub struct Discovery {
    pub candidates: Vec<SaveCandidate>,
    pub warnings: Vec<String>,
}

/// Run the resolver: user overrides, Steam ground truth, then known roots and
/// engine defaults. Manifest results are handled by `saves_detect` and are
/// de-duplicated by the caller.
pub fn discover(inputs: &DiscoveryInputs<'_>) -> Discovery {
    let mut warnings = Vec::new();
    let mut candidates: Vec<SaveCandidate> = Vec::new();

    for candidate in override_candidates(inputs, &mut warnings) {
        push_candidate(&mut candidates, candidate, inputs);
    }
    for candidate in store_candidates(inputs, &mut warnings) {
        push_candidate(&mut candidates, candidate, inputs);
    }
    for candidate in engine_candidates(inputs, &mut warnings) {
        push_candidate(&mut candidates, candidate, inputs);
    }

    candidates.sort_by(|left, right| {
        confidence_rank(&left.confidence)
            .cmp(&confidence_rank(&right.confidence))
            .then(source_rank(&left.source).cmp(&source_rank(&right.source)))
            .then(left.path.cmp(&right.path))
    });
    let mut seen = BTreeSet::new();
    candidates.retain(|candidate| seen.insert(candidate.path.clone()));
    candidates.truncate(inputs.registry.limits.max_candidates);
    warnings.truncate(MAX_WARNINGS);

    Discovery {
        candidates,
        warnings,
    }
}

fn push_candidate(
    candidates: &mut Vec<SaveCandidate>,
    candidate: SaveCandidate,
    inputs: &DiscoveryInputs<'_>,
) {
    if candidates.len() >= inputs.registry.limits.max_candidates {
        return;
    }
    let path = Path::new(&candidate.path);
    if !inputs.roots.iter().any(|root| path.starts_with(root)) {
        return;
    }
    if candidates
        .iter()
        .any(|existing| existing.path == candidate.path)
    {
        return;
    }
    candidates.push(candidate);
}

fn confidence_rank(value: &str) -> u8 {
    match value {
        CONFIDENCE_CERTAIN => 0,
        CONFIDENCE_LIKELY => 1,
        CONFIDENCE_GUESS => 2,
        _ => 3,
    }
}

fn source_rank(value: &str) -> u8 {
    match value {
        SOURCE_OVERRIDE => 0,
        SOURCE_STORE => 1,
        SOURCE_KNOWN_FOLDER => 2,
        SOURCE_ENGINE => 3,
        _ => 4,
    }
}

fn override_candidates(
    inputs: &DiscoveryInputs<'_>,
    warnings: &mut Vec<String>,
) -> Vec<SaveCandidate> {
    let mut candidates = Vec::new();
    for entry in &inputs.overrides.overrides {
        let matches_app = entry
            .steam_app_id
            .zip(inputs.steam_app_id)
            .map(|(left, right)| left == right)
            .unwrap_or(false);
        let matches_title =
            title_matches(&entry.title, inputs.title) || title_matches(inputs.title, &entry.title);
        if !matches_app && !matches_title {
            continue;
        }
        for root in &entry.roots {
            let Some(root_path) = resolve_runtime_root(&root.root, inputs) else {
                warnings.push(format!(
                    "Override for {} uses root {}, which does not exist on this machine.",
                    entry.title, root.root
                ));
                continue;
            };
            let path = root_path.join(&root.path);
            if let Some(candidate) = candidate_from_path(
                &path,
                &root_path,
                &root.root,
                SOURCE_OVERRIDE,
                CONFIDENCE_CERTAIN,
                entry
                    .note
                    .clone()
                    .unwrap_or_else(|| format!("Observed override for {}", entry.title)),
                inputs,
            ) {
                candidates.push(candidate);
            }
        }
    }
    candidates
}

fn engine_candidates(
    inputs: &DiscoveryInputs<'_>,
    warnings: &mut Vec<String>,
) -> Vec<SaveCandidate> {
    let mut candidates = Vec::new();
    let mut budget = ScanBudget::new(inputs.registry.limits.max_entries);
    for engine in &inputs.registry.engines {
        for pattern in engine.patterns.for_platform(inputs.platform) {
            for (root, path) in expand_pattern(pattern, inputs, &mut budget) {
                let evidence = format!("{} default layout: {pattern}", engine.label);
                if let Some(candidate) = candidate_from_path(
                    &path,
                    &root,
                    &root_id_for(pattern),
                    &engine.source,
                    &engine.confidence,
                    evidence,
                    inputs,
                ) {
                    candidates.push(candidate);
                }
            }
        }
    }
    if budget.exhausted {
        warnings.push(
            "Save discovery stopped early because the entry cap was reached; add a path manually if a folder is missing."
                .to_string(),
        );
    }
    let _ = inputs.user_name;
    candidates
}

struct ScanBudget {
    remaining: usize,
    per_directory: usize,
    exhausted: bool,
}

impl ScanBudget {
    fn new(max_entries: usize) -> Self {
        Self {
            remaining: max_entries.saturating_mul(8).max(max_entries),
            per_directory: max_entries,
            exhausted: false,
        }
    }

    fn take(&mut self) -> bool {
        if self.remaining == 0 {
            self.exhausted = true;
            return false;
        }
        self.remaining -= 1;
        true
    }
}

fn root_id_for(pattern: &str) -> String {
    let root = pattern
        .split('/')
        .find(|segment| !segment.is_empty())
        .and_then(placeholder)
        .unwrap_or("root");
    if matches!(root, "proton_drive_c" | "prefix") {
        ROOT_PROTON_PREFIX.to_string()
    } else {
        root.to_string()
    }
}

fn expand_pattern(
    pattern: &str,
    inputs: &DiscoveryInputs<'_>,
    budget: &mut ScanBudget,
) -> Vec<(PathBuf, PathBuf)> {
    let segments: Vec<&str> = pattern.split('/').filter(|part| !part.is_empty()).collect();
    let Some(root_token) = segments.first().and_then(|segment| placeholder(segment)) else {
        return Vec::new();
    };
    let Some(root) = resolve_runtime_root(root_token, inputs) else {
        return Vec::new();
    };
    let root = root.canonicalize().unwrap_or(root);
    if !root.is_dir() {
        return Vec::new();
    }
    let mut paths = vec![root.clone()];
    for (index, segment) in segments.iter().enumerate().skip(1) {
        let depth = index;
        if depth > inputs.registry.limits.max_depth {
            return Vec::new();
        }
        paths = expand_segment(paths, segment, depth, inputs, budget);
        if paths.is_empty() {
            return Vec::new();
        }
    }

    paths
        .into_iter()
        .filter(|path| path.is_dir())
        .map(|path| (root.clone(), path))
        .collect()
}

fn expand_segment(
    parents: Vec<PathBuf>,
    segment: &str,
    depth: usize,
    inputs: &DiscoveryInputs<'_>,
    budget: &mut ScanBudget,
) -> Vec<PathBuf> {
    if let Some(name) = placeholder(segment) {
        match name {
            "product" | "project" | "game" => expand_title(parents, inputs, budget),
            "windows_user" => {
                let parents = allow_wildcard_parents(parents, depth, inputs, "users");
                expand_windows_users(parents, inputs, budget)
            }
            "bundle" => {
                let parents = allow_wildcard_parents(parents, depth, inputs, "containers");
                expand_bundles(parents, inputs, budget)
            }
            "company" | "publisher" => {
                // Broad wildcards are only allowed above the whitelist depth,
                // or directly under a literal engine marker such as unity3d.
                let parents = allow_wildcard_parents(
                    parents,
                    depth,
                    inputs,
                    "unity3d,godot,app_userdata,renpy,my games",
                );
                expand_directories(parents, inputs, budget)
            }
            _ => Vec::new(),
        }
    } else if segment.contains('{') {
        expand_templated(parents, segment, inputs, budget)
    } else {
        parents
            .into_iter()
            .map(|parent| parent.join(segment))
            .filter(|path| is_real_directory(path))
            .collect()
    }
}

/// At or below `whitelist_min_depth`, a wildcard may only be enumerated when
/// its parent is a literal engine marker from the registry patterns. That is
/// the name whitelist that keeps a scan from walking a whole home directory.
fn allow_wildcard_parents(
    parents: Vec<PathBuf>,
    depth: usize,
    inputs: &DiscoveryInputs<'_>,
    allowed_names: &str,
) -> Vec<PathBuf> {
    if depth < inputs.registry.limits.whitelist_min_depth {
        return parents;
    }
    parents
        .into_iter()
        .filter(|parent| {
            parent
                .file_name()
                .map(|name| name.to_string_lossy().to_ascii_lowercase())
                .map(|name| {
                    allowed_names
                        .split(',')
                        .any(|allowed| name == allowed.trim())
                })
                .unwrap_or(false)
        })
        .collect()
}

fn expand_title(
    parents: Vec<PathBuf>,
    inputs: &DiscoveryInputs<'_>,
    budget: &mut ScanBudget,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for parent in parents {
        for (name, path) in read_directories(&parent, inputs, budget) {
            if title_matches(&name, inputs.title) {
                out.push(path);
            }
        }
    }
    out
}

fn expand_directories(
    parents: Vec<PathBuf>,
    inputs: &DiscoveryInputs<'_>,
    budget: &mut ScanBudget,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for parent in parents {
        for (_, path) in read_directories(&parent, inputs, budget) {
            out.push(path);
        }
    }
    out
}

fn expand_windows_users(
    parents: Vec<PathBuf>,
    inputs: &DiscoveryInputs<'_>,
    budget: &mut ScanBudget,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for parent in parents {
        // The parent is already the prefix's `users` directory: the pattern
        // segment before `{windows_user}` is the literal `users`.
        let users = parent;
        let mut names: Vec<(String, PathBuf)> = read_directories(&users, inputs, budget)
            .into_iter()
            .filter(|(name, _)| {
                !matches!(
                    name.to_ascii_lowercase().as_str(),
                    "public" | "default" | "default user" | "all users"
                )
            })
            .collect();
        if names.is_empty() && users.is_dir() {
            names.push(("steamuser".to_string(), users.join("steamuser")));
        }
        names.sort_by_key(|(name, _)| {
            let lower = name.to_ascii_lowercase();
            if lower == "steamuser" {
                0
            } else if lower == inputs.user_name.to_ascii_lowercase() {
                1
            } else {
                2
            }
        });
        out.extend(names.into_iter().map(|(_, path)| path));
    }
    out
}

fn expand_bundles(
    parents: Vec<PathBuf>,
    inputs: &DiscoveryInputs<'_>,
    budget: &mut ScanBudget,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for parent in parents {
        for (name, path) in read_directories(&parent, inputs, budget) {
            if bundle_matches(&name, inputs.title) {
                out.push(path);
            }
        }
    }
    out
}

fn expand_templated(
    parents: Vec<PathBuf>,
    template: &str,
    inputs: &DiscoveryInputs<'_>,
    budget: &mut ScanBudget,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for parent in parents {
        for (name, path) in read_directories(&parent, inputs, budget) {
            if let Some(captures) = match_segment(template, &name) {
                let valid = captures.iter().all(|(token, value)| match *token {
                    "product" | "project" | "game" => title_matches(value, inputs.title),
                    "bundle" => bundle_matches(value, inputs.title),
                    "company" | "publisher" | "windows_user" => {
                        !value.is_empty() && !inputs.registry.exclusions.excludes_directory(value)
                    }
                    _ => false,
                });
                if valid {
                    out.push(path);
                }
            }
        }
    }
    out
}

/// Match a whole directory name against a segment template such as
/// `unity.{company}.{product}`. Captures are returned in template order.
fn match_segment<'a>(template: &'a str, name: &str) -> Option<Vec<(&'a str, String)>> {
    let parts = template_parts(template);
    if parts.is_empty() {
        return None;
    }
    let mut cursor = 0usize;
    let mut captures = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        match part {
            SegmentPart::Literal(literal) => {
                if !name[cursor..].starts_with(literal) {
                    return None;
                }
                cursor += literal.len();
            }
            SegmentPart::Token(token) => {
                let next_literal = parts[index + 1..].iter().find_map(|part| match part {
                    SegmentPart::Literal(literal) => Some(*literal),
                    SegmentPart::Token(_) => None,
                });
                let end = match next_literal {
                    Some(literal) => cursor + name[cursor..].find(literal)?,
                    None => name.len(),
                };
                let captured = name[cursor..end].to_string();
                if captured.is_empty() {
                    return None;
                }
                captures.push((*token, captured));
                cursor = end;
            }
        }
    }
    if cursor == name.len() {
        Some(captures)
    } else {
        None
    }
}

enum SegmentPart<'a> {
    Literal(&'a str),
    Token(&'a str),
}

fn template_parts(template: &str) -> Vec<SegmentPart<'_>> {
    let mut parts = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        if open > 0 {
            parts.push(SegmentPart::Literal(&rest[..open]));
        }
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            return Vec::new();
        };
        parts.push(SegmentPart::Token(&after[..close]));
        rest = &after[close + 1..];
    }
    if !rest.is_empty() {
        parts.push(SegmentPart::Literal(rest));
    }
    parts
}

fn read_directories(
    parent: &Path,
    inputs: &DiscoveryInputs<'_>,
    budget: &mut ScanBudget,
) -> Vec<(String, PathBuf)> {
    if !is_real_directory(parent) {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if out.len() >= budget.per_directory || !budget.take() {
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if inputs.registry.exclusions.excludes_directory(&name) {
            continue;
        }
        let path = entry.path();
        if is_link_like(&path) || !is_real_directory(&path) {
            continue;
        }
        out.push((name, path));
    }
    out.sort_by(|left, right| left.0.cmp(&right.0));
    out
}

/// True for a directory that is reachable without following a link.
fn is_real_directory(path: &Path) -> bool {
    if is_link_like(path) {
        return false;
    }
    fs::metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

fn is_link_like(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        // A path that cannot be inspected is never safe to scan.
        return true;
    };
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

fn resolve_runtime_root(root: &str, inputs: &DiscoveryInputs<'_>) -> Option<PathBuf> {
    match root {
        "proton_drive_c" | "prefix" => inputs.prefix.map(drive_c),
        "steam_root" => inputs.steam_roots.first().cloned(),
        other => inputs.folders.get(other),
    }
}

fn drive_c(prefix: &Path) -> PathBuf {
    let drive_c = prefix.join("drive_c");
    if drive_c.is_dir() || !prefix.ends_with("drive_c") {
        drive_c
    } else {
        prefix.to_path_buf()
    }
}

fn candidate_from_path(
    path: &Path,
    root: &Path,
    root_id: &str,
    source: &str,
    confidence: &str,
    evidence: String,
    inputs: &DiscoveryInputs<'_>,
) -> Option<SaveCandidate> {
    if is_link_like(path) {
        return None;
    }
    let canonical = path.canonicalize().ok()?;
    if !canonical.is_dir() {
        return None;
    }
    if let Some(name) = canonical.file_name() {
        if inputs
            .registry
            .exclusions
            .excludes_file(&name.to_string_lossy())
        {
            return None;
        }
    }
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let subpath = canonical.strip_prefix(&root).ok()?;
    if subpath.components().any(|component| match component {
        Component::Normal(name) => inputs
            .registry
            .exclusions
            .excludes_directory(&name.to_string_lossy()),
        Component::ParentDir => true,
        _ => false,
    }) {
        return None;
    }
    Some(SaveCandidate {
        path: canonical.to_string_lossy().to_string(),
        kind: "directory".to_string(),
        root_id: root_id.to_string(),
        subpath: subpath
            .components()
            .filter_map(|component| match component {
                Component::Normal(name) => Some(name.to_string_lossy().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/"),
        source: source.to_string(),
        confidence: confidence.to_string(),
        evidence,
        size_bytes: 0,
        last_modified: modified_rfc3339(&canonical),
        first_seen: Some(inputs.now.to_string()),
        last_verified: Some(inputs.now.to_string()),
        confirmed: false,
        ignored: false,
        auto_status: String::new(),
        auto_reason: None,
    })
}

fn modified_rfc3339(path: &Path) -> Option<String> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let datetime: chrono::DateTime<chrono::Utc> = modified.into();
    Some(datetime.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/* ------------------------------------------------------------------ title matching */

/// Normalise a title or folder name to compact lowercase alphanumerics.
fn compact(value: &str) -> String {
    crate::steam::normalize_title(value).replace(' ', "")
}

/// True when a directory name is the game title (with or without its edition
/// suffix). A guess is never made from a partial token.
pub fn title_matches(folder: &str, title: &str) -> bool {
    let name = compact(folder);
    if name.is_empty() {
        return false;
    }
    let direct = compact(title);
    if !direct.is_empty() && name == direct {
        return true;
    }
    let stripped_folder = compact(&crate::steam::strip_edition(folder));
    let stripped_title = compact(&crate::steam::strip_edition(title));
    (!stripped_title.is_empty() && name == stripped_title)
        || (!stripped_folder.is_empty() && stripped_folder == direct)
}

fn bundle_matches(name: &str, title: &str) -> bool {
    let bundle = compact(name);
    let title = compact(title);
    if title.len() < 4 || bundle.is_empty() {
        return false;
    }
    !matches!(title.as_str(), "game" | "games" | "save" | "saves") && bundle.contains(&title)
}

/* ------------------------------------------------------------------ store ground truth */

fn store_candidates(
    inputs: &DiscoveryInputs<'_>,
    warnings: &mut Vec<String>,
) -> Vec<SaveCandidate> {
    let Some(app_id) = inputs.steam_app_id else {
        return Vec::new();
    };
    if inputs.steam_roots.is_empty() || inputs.steam_user_ids.is_empty() {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    for root in inputs.steam_roots {
        for user_id in inputs.steam_user_ids {
            let Some(file) = render_store_path(
                &inputs.registry.store_roots.steam.remotecache,
                root,
                app_id,
                user_id,
            ) else {
                continue;
            };
            if !file.is_file() || is_link_like(&file) {
                continue;
            }
            let Ok(metadata) = fs::metadata(&file) else {
                continue;
            };
            if metadata.len() > MAX_REMOTECACHE_BYTES {
                warnings.push(format!(
                    "Skipped {}: larger than {}.",
                    file.display(),
                    crate::util::human_bytes(MAX_REMOTECACHE_BYTES)
                ));
                continue;
            }
            let Ok(text) = fs::read_to_string(&file) else {
                continue;
            };
            let mut found = parse_remotecache(&text, inputs, warnings);
            candidates.append(&mut found);
        }
    }
    candidates
}

fn parse_remotecache(
    text: &str,
    inputs: &DiscoveryInputs<'_>,
    warnings: &mut Vec<String>,
) -> Vec<SaveCandidate> {
    let tokens = tokenize_vdf(text);
    let mut index = 0;
    let mut nodes = 0;
    let tree = parse_vdf_block(&tokens, &mut index, 0, &mut nodes);
    let mut candidates = Vec::new();

    for (steam_root, registry_root) in &inputs.registry.store_roots.steam.cloud_roots {
        let mut files: Vec<String> = Vec::new();
        collect_cloud_entries(&tree, steam_root, &mut files);
        if files.is_empty() {
            continue;
        }
        let mut directories: Vec<Vec<String>> = files
            .iter()
            .filter_map(|entry| cloud_directory(entry))
            .collect();
        directories.sort();
        directories.dedup();
        if directories.is_empty() {
            continue;
        }
        let common = common_components(&directories);
        if common.is_empty() {
            warnings.push(format!(
                "Steam Cloud lists {steam_root} files for this game without a common folder; no candidate was made."
            ));
            continue;
        }
        let relative = common.join(std::path::MAIN_SEPARATOR_STR);
        for root_path in store_root_paths(registry_root, inputs) {
            let path = root_path.join(&relative);
            if let Some(candidate) = candidate_from_path(
                &path,
                &root_path,
                registry_root,
                SOURCE_STORE,
                CONFIDENCE_CERTAIN,
                format!("Steam Cloud remotecache.vdf lists files under {steam_root}"),
                inputs,
            ) {
                candidates.push(candidate);
            }
        }
    }
    candidates
}

fn render_store_path(
    template: &str,
    steam_root: &Path,
    app_id: u32,
    user_id: &str,
) -> Option<PathBuf> {
    let rendered = template
        .replace("{steam_root}", &steam_root.to_string_lossy())
        .replace("{appid}", &app_id.to_string())
        .replace("{user_id}", user_id);
    if rendered.contains('{') {
        return None;
    }
    Some(PathBuf::from(rendered))
}

fn store_root_paths(root_id: &str, inputs: &DiscoveryInputs<'_>) -> Vec<PathBuf> {
    if inputs.platform == Platform::Windows {
        return inputs.folders.get(root_id).into_iter().collect();
    }
    let prefix = inputs.prefix.map(PathBuf::from).or_else(|| {
        let app_id = inputs.steam_app_id?;
        inputs.steam_roots.iter().find_map(|root| {
            render_store_path(
                &inputs.registry.store_roots.steam.prefix,
                root,
                app_id,
                inputs.user_name,
            )
            .filter(|path| path.is_dir())
        })
    });
    let Some(prefix) = prefix else {
        return Vec::new();
    };
    let Some(suffix) = windows_folder_suffix(root_id) else {
        return Vec::new();
    };
    let drive_c = drive_c(&prefix);
    let mut users: Vec<String> = read_dir_names(&drive_c.join("users"))
        .into_iter()
        .filter(|name| {
            !matches!(
                name.to_ascii_lowercase().as_str(),
                "public" | "default" | "default user" | "all users"
            )
        })
        .collect();
    users.sort_by_key(|name| {
        let lower = name.to_ascii_lowercase();
        if lower == "steamuser" {
            0
        } else if lower == inputs.user_name.to_ascii_lowercase() {
            1
        } else {
            2
        }
    });
    if users.is_empty() && drive_c.join("users").is_dir() {
        users.push("steamuser".to_string());
    }
    users
        .into_iter()
        .map(|user| {
            let mut path = drive_c.join("users").join(user);
            for part in suffix.split('/') {
                path = path.join(part);
            }
            path
        })
        .collect()
}

fn windows_folder_suffix(root_id: &str) -> Option<&'static str> {
    match root_id {
        "saved_games" => Some("Saved Games"),
        "documents" => Some("Documents"),
        "local_app_data" => Some("AppData/Local"),
        "local_app_data_low" => Some("AppData/LocalLow"),
        "roaming_app_data" => Some("AppData/Roaming"),
        "program_data" => Some("ProgramData"),
        "public" => Some("Public"),
        _ => None,
    }
}

fn read_dir_names(parent: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir() && !is_link_like(&entry.path()))
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}

fn cloud_directory(entry: &str) -> Option<Vec<String>> {
    let normalized = entry.replace('\\', "/");
    let parts: Vec<String> = normalized
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_string)
        .collect();
    if parts.iter().any(|part| part == ".." || part.contains('*')) {
        return None;
    }
    if parts.len() < 2 {
        return None;
    }
    Some(parts)
}

fn common_components(directories: &[Vec<String>]) -> Vec<String> {
    let Some(first) = directories.first() else {
        return Vec::new();
    };
    let identical = directories.iter().all(|entry| entry == first);
    let mut common: Vec<String> = Vec::new();
    for (index, part) in first.iter().enumerate() {
        if directories
            .iter()
            .all(|entry| entry.get(index).map(|value| value == part).unwrap_or(false))
        {
            common.push(part.clone());
        } else {
            break;
        }
    }
    if identical {
        common.pop();
    }
    common
}

enum VdfToken {
    Value(String),
    Open,
    Close,
}

fn tokenize_vdf(text: &str) -> Vec<VdfToken> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < chars.len() && tokens.len() < MAX_VDF_TOKENS {
        match chars[index] {
            ' ' | '\t' | '\r' | '\n' => index += 1,
            '{' => {
                tokens.push(VdfToken::Open);
                index += 1;
            }
            '}' => {
                tokens.push(VdfToken::Close);
                index += 1;
            }
            '/' if chars.get(index + 1) == Some(&'/') => {
                while index < chars.len() && chars[index] != '\n' {
                    index += 1;
                }
            }
            '"' => {
                index += 1;
                let mut value = String::new();
                while index < chars.len() {
                    match chars[index] {
                        '"' => {
                            index += 1;
                            break;
                        }
                        '\\' if chars.get(index + 1) == Some(&'"') => {
                            value.push('"');
                            index += 2;
                        }
                        '\\' if chars.get(index + 1) == Some(&'\\') => {
                            value.push('\\');
                            index += 2;
                        }
                        character => {
                            value.push(character);
                            index += 1;
                        }
                    }
                }
                tokens.push(VdfToken::Value(value));
            }
            _ => index += 1,
        }
    }
    tokens
}

type VdfBlock = Vec<(String, VdfNode)>;

enum VdfNode {
    Value,
    Block(VdfBlock),
}

fn parse_vdf_block(
    tokens: &[VdfToken],
    index: &mut usize,
    depth: usize,
    nodes: &mut usize,
) -> VdfBlock {
    let mut block = VdfBlock::new();
    if depth > 32 {
        return block;
    }
    while *index < tokens.len() && *nodes < MAX_VDF_NODES {
        match &tokens[*index] {
            VdfToken::Close => {
                *index += 1;
                break;
            }
            VdfToken::Open => {
                *index += 1;
            }
            VdfToken::Value(key) => {
                let key = key.clone();
                *index += 1;
                match tokens.get(*index) {
                    Some(VdfToken::Value(_)) => {
                        *nodes += 1;
                        block.push((key, VdfNode::Value));
                        *index += 1;
                    }
                    Some(VdfToken::Open) => {
                        *index += 1;
                        *nodes += 1;
                        let nested = parse_vdf_block(tokens, index, depth + 1, nodes);
                        block.push((key, VdfNode::Block(nested)));
                    }
                    _ => break,
                }
            }
        }
    }
    block
}

/// Collect the file entries of one Steam Cloud root anywhere in the VDF tree.
fn collect_cloud_entries(block: &VdfBlock, root: &str, files: &mut Vec<String>) {
    for (key, node) in block {
        match node {
            VdfNode::Block(nested) if key == root => {
                collect_leaves(nested, "", files);
            }
            VdfNode::Block(nested) => collect_cloud_entries(nested, root, files),
            VdfNode::Value => {}
        }
    }
}

fn collect_leaves(block: &VdfBlock, prefix: &str, files: &mut Vec<String>) {
    if files.len() >= MAX_VDF_NODES {
        return;
    }
    for (key, node) in block {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}/{key}")
        };
        match node {
            VdfNode::Value => files.push(path),
            VdfNode::Block(nested) => collect_leaves(nested, &path, files),
        }
    }
}

/* ------------------------------------------------------------------ helpers */

/// Placeholder name when `segment` is exactly `{name}`.
fn placeholder(segment: &str) -> Option<&str> {
    let rest = segment.strip_prefix('{')?;
    let inner = rest.strip_suffix('}')?;
    if inner.is_empty() || inner.contains('{') || inner.contains('}') {
        None
    } else {
        Some(inner)
    }
}

fn tokens(value: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut rest = value;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            break;
        };
        tokens.push(&after[..close]);
        rest = &after[close + 1..];
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    static EMPTY_OVERRIDES: SaveOverrides = SaveOverrides {
        version: 1,
        overrides: Vec::new(),
    };

    struct Fixture {
        root: PathBuf,
        home: PathBuf,
        prefix: PathBuf,
        folders: ResolvedFolders,
        registry: SaveRegistry,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn temp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-registry-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fixture(label: &str) -> Fixture {
        let root = temp_root(label);
        let home = root.join("home");
        let prefix = root.join("prefix/pfx");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(prefix.join("drive_c/users/steamuser/AppData/Local")).unwrap();
        fs::create_dir_all(prefix.join("drive_c/users/steamuser/AppData/LocalLow")).unwrap();
        fs::create_dir_all(prefix.join("drive_c/users/steamuser/AppData/Roaming")).unwrap();
        fs::create_dir_all(prefix.join("drive_c/users/steamuser/Documents")).unwrap();
        fs::create_dir_all(prefix.join("drive_c/users/steamuser/Saved Games")).unwrap();
        fs::create_dir_all(prefix.join("drive_c/ProgramData")).unwrap();

        for path in [
            "Documents",
            "Saved Games",
            "AppData/Roaming",
            "AppData/Local",
            "AppData/LocalLow",
            "Library/Application Support",
            "Library/Containers",
            ".var/app",
            ".local/share",
            ".local/state",
            ".config",
        ] {
            fs::create_dir_all(home.join(path)).unwrap();
        }

        let mut folders = ResolvedFolders::default();
        folders.insert("home", home.clone());
        folders.insert("xdg_data", home.join(".local/share"));
        folders.insert("xdg_config", home.join(".config"));
        folders.insert("xdg_state", home.join(".local/state"));
        folders.insert("documents", home.join("Documents"));
        folders.insert("saved_games", home.join("Saved Games"));
        folders.insert("roaming_app_data", home.join("AppData/Roaming"));
        folders.insert("local_app_data", home.join("AppData/Local"));
        folders.insert("local_app_data_low", home.join("AppData/LocalLow"));
        folders.insert(
            "application_support",
            home.join("Library/Application Support"),
        );
        folders.insert("containers", home.join("Library/Containers"));
        folders.insert("flatpak_apps", home.join(".var/app"));

        Fixture {
            root,
            home,
            prefix,
            folders,
            registry: SaveRegistry::parse(REGISTRY_YAML).unwrap(),
        }
    }

    fn discover_in(fixture: &Fixture, platform: Platform, title: &str) -> Discovery {
        let inputs = DiscoveryInputs {
            platform,
            user_name: "player",
            folders: &fixture.folders,
            prefix: Some(&fixture.prefix),
            title,
            steam_app_id: None,
            steam_roots: &[],
            steam_user_ids: &[],
            roots: &[fixture.home.clone(), fixture.prefix.clone()],
            now: "2026-11-11T00:00:00Z",
            registry: &fixture.registry,
            overrides: &EMPTY_OVERRIDES,
        };
        discover(&inputs)
    }

    fn paths(discovery: &Discovery) -> Vec<String> {
        discovery
            .candidates
            .iter()
            .map(|candidate| candidate.path.clone())
            .collect()
    }

    #[test]
    fn embedded_registry_is_valid_and_versioned() {
        let registry = registry();
        assert!(registry.version >= 1);
        assert_eq!(registry.limits.max_depth, 4);
        assert!(registry.limits.whitelist_min_depth >= 2);
        assert_eq!(
            registry.known_folders.windows["saved_games"]
                .folder_id
                .as_deref(),
            Some("FOLDERID_SavedGames")
        );
        assert!(registry.known_folders.windows["local_app_data_low"]
            .env
            .is_empty());
        let ids: BTreeSet<&str> = registry
            .engines
            .iter()
            .map(|engine| engine.id.as_str())
            .collect();
        for expected in ["unity", "unreal", "godot", "renpy", "minecraft"] {
            assert!(ids.contains(expected), "missing engine {expected}");
        }
        assert_eq!(overrides().overrides.len(), 0);
    }

    #[test]
    fn invalid_registries_are_rejected() {
        assert!(SaveRegistry::parse(&REGISTRY_YAML.replace("version: 1", "version: 0")).is_err());
        assert!(
            SaveRegistry::parse(&REGISTRY_YAML.replace("max_depth: 4", "max_depth: 9")).is_err()
        );
        assert!(SaveRegistry::parse(&REGISTRY_YAML.replace(
            "      env: []\n      api: true\n      fallback: \"{profile}/Saved Games\"",
            "      env: [SAVEDGAMES]\n      api: true\n      fallback: \"{profile}/Saved Games\"",
        ))
        .is_err());
        assert!(SaveRegistry::parse(&REGISTRY_YAML.replace(
            "      env: []\n      api: true\n      fallback: \"{local_app_data}/../LocalLow\"",
            "      env: [LOCALAPPDATA]\n      api: true\n      fallback: \"{local_app_data}/../LocalLow\"",
        ))
        .is_err());
        assert!(SaveRegistry::parse(
            &REGISTRY_YAML.replace("{saved_games}/{product}", "{unknown_root}/{product}")
        )
        .is_err());
        assert!(SaveRegistry::parse(&REGISTRY_YAML.replace(
            "        - \"{local_app_data_low}/{company}/{product}\"",
            "        - \"{local_app_data_low}/{company}/{product}/{extra}/{deeper}\""
        ))
        .is_err());
    }

    #[test]
    fn known_folder_resolution_honours_redirects_and_local_low() {
        let catalog = &registry().known_folders;
        let mut env = BTreeMap::new();
        env.insert("USERPROFILE".to_string(), PathBuf::from("/users/player"));
        env.insert(
            "OneDrive".to_string(),
            PathBuf::from("/users/player/OneDrive"),
        );
        env.insert(
            "LOCALAPPDATA".to_string(),
            PathBuf::from("/users/player/AppData/Local"),
        );
        let inputs = KnownFolderInputs {
            env,
            api: BTreeMap::new(),
        };
        let resolved = resolve_known_folders(catalog, Platform::Windows, &inputs);
        assert_eq!(
            resolved.get("local_app_data_low"),
            Some(PathBuf::from("/users/player/AppData/LocalLow"))
        );
        // No API answer and no OneDrive/Documents directory: Documents stays
        // unresolved instead of being guessed as %USERPROFILE%\Documents.
        assert_eq!(resolved.get("documents"), None);

        let root = temp_root("known-folders");
        let onedrive = root.join("OneDrive");
        fs::create_dir_all(onedrive.join("Documents")).unwrap();
        let mut env = BTreeMap::new();
        env.insert("USERPROFILE".to_string(), root.join("profile"));
        env.insert("OneDrive".to_string(), onedrive.clone());
        let inputs = KnownFolderInputs {
            env,
            api: BTreeMap::new(),
        };
        let resolved = resolve_known_folders(catalog, Platform::Windows, &inputs);
        assert_eq!(resolved.get("documents"), Some(onedrive.join("Documents")));

        // The API answer wins over environment redirection.
        let mut api = BTreeMap::new();
        api.insert("documents".to_string(), root.join("api-docs"));
        api.insert("local_app_data".to_string(), root.join("api-local"));
        let inputs = KnownFolderInputs {
            env: BTreeMap::new(),
            api,
        };
        let resolved = resolve_known_folders(catalog, Platform::Windows, &inputs);
        assert_eq!(resolved.get("documents"), Some(root.join("api-docs")));
        assert_eq!(
            resolved.get("local_app_data_low"),
            Some(root.join("LocalLow"))
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn saved_games_and_documents_title_folders_are_likely_candidates() {
        let fixture = fixture("saved-games");
        fs::create_dir_all(fixture.home.join("Saved Games/Hades")).unwrap();
        fs::create_dir_all(fixture.home.join("Documents/My Games/Hades")).unwrap();
        let discovery = discover_in(&fixture, Platform::Windows, "Hades");
        let found = paths(&discovery);
        let saved = discovery
            .candidates
            .iter()
            .find(|candidate| candidate.path.ends_with("Saved Games/Hades"))
            .unwrap_or_else(|| panic!("missing Saved Games candidate: {found:?}"));
        assert_eq!(saved.root_id, "saved_games");
        assert_eq!(saved.source, SOURCE_KNOWN_FOLDER);
        assert_eq!(saved.confidence, CONFIDENCE_LIKELY);
        assert!(saved.evidence.contains("Game title folders"));
        assert!(saved.first_seen.is_some());
        assert!(!saved.confirmed);
        assert!(!saved.is_guess());
        assert!(discovery
            .candidates
            .iter()
            .any(|candidate| candidate.path.ends_with("Documents/My Games/Hades")));
    }

    #[test]
    fn unity_unreal_godot_renpy_and_minecraft_layouts_are_guesses() {
        let fixture = fixture("engines");
        fs::create_dir_all(fixture.home.join(".config/unity3d/Acme/Hades")).unwrap();
        fs::create_dir_all(fixture.home.join(".local/share/Hades/Saved/SaveGames")).unwrap();
        fs::create_dir_all(fixture.home.join(".local/share/godot/app_userdata/Hades")).unwrap();
        fs::create_dir_all(fixture.home.join(".renpy/Hades")).unwrap();
        fs::create_dir_all(fixture.home.join(".minecraft/saves")).unwrap();

        let discovery = discover_in(&fixture, Platform::Linux, "Hades");
        for expected in ["Unity", "Unreal Engine", "Godot", "Ren'Py", "Minecraft"] {
            assert!(
                discovery
                    .candidates
                    .iter()
                    .any(|candidate| candidate.evidence.contains(expected)),
                "missing {expected} in {:?}",
                paths(&discovery)
            );
        }
        let unity = discovery
            .candidates
            .iter()
            .find(|candidate| candidate.evidence.contains("Unity"))
            .unwrap();
        assert_eq!(unity.confidence, CONFIDENCE_GUESS);
        assert!(unity.is_guess());
        let minecraft = discovery
            .candidates
            .iter()
            .find(|candidate| candidate.evidence.contains("Minecraft"))
            .unwrap();
        assert_eq!(minecraft.confidence, CONFIDENCE_LIKELY);
    }

    #[test]
    fn proton_prefix_is_scanned_for_windows_layouts() {
        let fixture = fixture("proton");
        fs::create_dir_all(
            fixture
                .prefix
                .join("drive_c/users/steamuser/Saved Games/Hades"),
        )
        .unwrap();
        let discovery = discover_in(&fixture, Platform::Linux, "Hades");
        let candidate = discovery
            .candidates
            .iter()
            .find(|candidate| {
                candidate
                    .path
                    .contains("drive_c/users/steamuser/Saved Games/Hades")
            })
            .unwrap_or_else(|| panic!("{:?}", paths(&discovery)));
        assert_eq!(candidate.root_id, ROOT_PROTON_PREFIX);
        assert!(candidate.subpath.starts_with("users/"));
        assert_eq!(candidate.source, SOURCE_KNOWN_FOLDER);
    }

    #[test]
    fn xdg_flatpak_and_macos_containers_are_roots() {
        let fixture = fixture("xdg");
        fs::create_dir_all(fixture.home.join(".local/share/Hades")).unwrap();
        fs::create_dir_all(fixture.home.join(".config/Hades")).unwrap();
        fs::create_dir_all(fixture.home.join(".local/state/Hades")).unwrap();
        fs::create_dir_all(
            fixture
                .home
                .join(".var/app/com.mojang.Minecraft/.minecraft/saves"),
        )
        .unwrap();
        let discovery = discover_in(&fixture, Platform::Linux, "Hades");
        let roots: BTreeSet<String> = discovery
            .candidates
            .iter()
            .map(|candidate| candidate.root_id.clone())
            .collect();
        for root in ["xdg_data", "xdg_config", "xdg_state", "flatpak_apps"] {
            assert!(roots.contains(root), "missing root {root}: {roots:?}");
        }

        fs::create_dir_all(fixture.home.join("Library/Application Support/Hades")).unwrap();
        fs::create_dir_all(
            fixture
                .home
                .join("Library/Containers/com.example.hades/Data/Documents/Hades"),
        )
        .unwrap();
        let discovery = discover_in(&fixture, Platform::Macos, "Hades");
        assert!(discovery
            .candidates
            .iter()
            .any(|candidate| candidate.root_id == "application_support"));
        let container = discovery
            .candidates
            .iter()
            .find(|candidate| candidate.root_id == "containers")
            .expect("macOS container candidate");
        assert_eq!(container.confidence, CONFIDENCE_GUESS);
        assert!(container.subpath.contains("Data/Documents/Hades"));
    }

    #[test]
    fn title_matching_is_conservative() {
        assert!(title_matches("Hades", "Hades"));
        assert!(title_matches("Hades™", "Hades"));
        assert!(title_matches("Hades Game of the Year Edition", "Hades"));
        assert!(title_matches("MyGame", "My Game"));
        assert!(!title_matches("Hades II", "Hades"));
        assert!(!title_matches("Hade", "Hades"));
        assert!(!title_matches("My Game Launcher", "My Game"));
    }

    #[test]
    fn segment_templates_capture_company_and_product() {
        let captures = match_segment("unity.{company}.{product}", "unity.acme.hades").unwrap();
        assert_eq!(
            captures,
            vec![
                ("company", "acme".to_string()),
                ("product", "hades".to_string())
            ]
        );
        assert_eq!(
            match_segment("unity.{company}.{product}.save", "unity.acme.hades.deluxe"),
            None
        );
        assert_eq!(match_segment("unity.{product}", "unity"), None);
    }

    #[test]
    fn depth_and_entry_caps_are_enforced() {
        let mut fixture = fixture("caps");
        let deep = fixture.home.join("Saved Games/a/b/c/d/Hades");
        fs::create_dir_all(&deep).unwrap();
        // Depth 5 below saved_games: the registry cap is 4.
        let discovery = discover_in(&fixture, Platform::Windows, "Hades");
        assert!(!paths(&discovery)
            .iter()
            .any(|path| path.ends_with("d/Hades")));

        // Entry cap: one directory read returns at most one entry.
        fixture.registry.limits.max_entries = 1;
        for index in 0..5 {
            fs::create_dir_all(fixture.home.join(format!("Saved Games/{index}"))).unwrap();
        }
        let discovery = discover_in(&fixture, Platform::Windows, "Hades");
        assert!(discovery.candidates.len() <= 1, "{:?}", paths(&discovery));

        // Candidate cap: two real title folders, one returned.
        fixture.registry.limits.max_entries = 2000;
        fixture.registry.limits.max_candidates = 1;
        fs::create_dir_all(fixture.home.join("Saved Games/Hades")).unwrap();
        fs::create_dir_all(fixture.home.join("Documents/Hades")).unwrap();
        let discovery = discover_in(&fixture, Platform::Windows, "Hades");
        assert_eq!(discovery.candidates.len(), 1, "{:?}", paths(&discovery));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_excluded_folders_are_skipped() {
        let fixture = fixture("links");
        let outside = fixture.root.join("outside/Hades");
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, fixture.home.join("Saved Games/Hades")).unwrap();
        fs::create_dir_all(fixture.home.join("Saved Games/Cache/Hades")).unwrap();

        let discovery = discover_in(&fixture, Platform::Windows, "Hades");
        assert!(paths(&discovery).is_empty(), "{:?}", paths(&discovery));
    }

    #[test]
    fn candidates_outside_the_allowlist_are_dropped() {
        let fixture = fixture("allowlist");
        let outside = fixture.root.join("outside/Hades");
        fs::create_dir_all(&outside).unwrap();
        let inputs = DiscoveryInputs {
            platform: Platform::Windows,
            user_name: "player",
            folders: &fixture.folders,
            prefix: None,
            title: "Hades",
            steam_app_id: None,
            steam_roots: &[],
            steam_user_ids: &[],
            roots: std::slice::from_ref(&fixture.home),
            now: "2026-11-11T00:00:00Z",
            registry: &fixture.registry,
            overrides: &EMPTY_OVERRIDES,
        };
        let discovery = discover(&inputs);
        assert!(discovery
            .candidates
            .iter()
            .all(|candidate| Path::new(&candidate.path).starts_with(&fixture.home)));
    }

    #[test]
    fn overrides_are_certain_and_win_over_engine_guesses() {
        let fixture = fixture("override");
        fs::create_dir_all(fixture.home.join("Saved Games/Hades")).unwrap();
        let overrides = SaveOverrides {
            version: 1,
            overrides: vec![SaveOverride {
                title: "Hades".to_string(),
                steam_app_id: Some(1145360),
                note: Some("observed".to_string()),
                roots: vec![OverrideRoot {
                    root: "saved_games".to_string(),
                    path: "Hades".to_string(),
                }],
            }],
        };
        let inputs = DiscoveryInputs {
            platform: Platform::Windows,
            user_name: "player",
            folders: &fixture.folders,
            prefix: None,
            title: "Hades",
            steam_app_id: Some(1145360),
            steam_roots: &[],
            steam_user_ids: &[],
            roots: std::slice::from_ref(&fixture.home),
            now: "2026-11-11T00:00:00Z",
            registry: &fixture.registry,
            overrides: &overrides,
        };
        let discovery = discover(&inputs);
        let first = &discovery.candidates[0];
        assert_eq!(first.source, SOURCE_OVERRIDE);
        assert_eq!(first.confidence, CONFIDENCE_CERTAIN);
        assert!(!first.confirmed);
        assert_eq!(first.root_id, "saved_games");
        assert_eq!(first.evidence, "observed");
    }

    #[test]
    fn steam_remotecache_is_store_ground_truth() {
        let fixture = fixture("steam");
        let steam_root = fixture.root.join("Steam");
        let cache = steam_root.join("userdata/123/1145360/remotecache.vdf");
        fs::create_dir_all(cache.parent().unwrap()).unwrap();
        fs::create_dir_all(
            fixture
                .prefix
                .join("drive_c/users/steamuser/Saved Games/Hades"),
        )
        .unwrap();
        fs::write(
            &cache,
            r#"
"AppState"
{
    "appid" "1145360"
    "Root"
    {
        "WinSavedGames"
        {
            "Hades\\save1.sav" "hash"
            "Hades\\profiles\\p1.sav" "hash"
        }
    }
}
"#,
        )
        .unwrap();
        let roots = vec![fixture.home.clone(), fixture.prefix.clone()];
        let inputs = DiscoveryInputs {
            platform: Platform::Linux,
            user_name: "player",
            folders: &fixture.folders,
            prefix: Some(&fixture.prefix),
            title: "Hades",
            steam_app_id: Some(1145360),
            steam_roots: &[steam_root],
            steam_user_ids: &["123".to_string()],
            roots: &roots,
            now: "2026-11-11T00:00:00Z",
            registry: &fixture.registry,
            overrides: &EMPTY_OVERRIDES,
        };
        let discovery = discover(&inputs);
        let candidate = discovery
            .candidates
            .iter()
            .find(|candidate| candidate.source == SOURCE_STORE)
            .expect("store candidate");
        // A Windows game under Proton resolves WinSavedGames inside the prefix.
        assert!(
            candidate
                .path
                .contains("drive_c/users/steamuser/Saved Games/Hades"),
            "{}",
            candidate.path
        );
        assert_eq!(candidate.confidence, CONFIDENCE_CERTAIN);
        assert!(candidate.evidence.contains("remotecache.vdf"));
    }

    #[test]
    fn vdf_parser_bounds_malformed_input() {
        let tokens = tokenize_vdf("{\"a\" { \"b\" \"c\" ");
        assert!(!tokens.is_empty());
        let mut index = 0;
        let mut nodes = 0;
        let tree = parse_vdf_block(&tokens, &mut index, 0, &mut nodes);
        assert!(tree.is_empty() || nodes <= MAX_VDF_NODES);
        assert!(tokenize_vdf(&"\"x\"".repeat(MAX_VDF_TOKENS + 10)).len() <= MAX_VDF_TOKENS);
    }

    #[test]
    fn overrides_file_rejects_unsafe_paths() {
        let text = r#"
version: 1
overrides:
  - title: "Hades"
    roots:
      - root: saved_games
        path: "../escape"
"#;
        assert!(parse_overrides(text).is_err());
        let text = r#"
version: 1
overrides:
  - title: "Hades"
    roots:
      - root: nope
        path: "Hades"
"#;
        assert!(parse_overrides(text).is_err());
        let text = r#"
version: 1
overrides:
  - title: "Hades"
    roots:
      - root: saved_games
        path: "Hades"
"#;
        assert!(parse_overrides(text).is_ok());
    }
}
