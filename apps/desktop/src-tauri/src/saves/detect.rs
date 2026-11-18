//! Save folder detection for cloud saves (#107).
//!
//! Detection is local only. Ludex reads the Ludusavi manifest Ludex downloaded
//! into its app data folder (#153), an explicit path the user set, or a
//! user-installed copy, and optionally asks a `ludusavi` binary for a `preview`
//! through its JSON API. The manifest is never bundled with Ludex. Resolved
//! templates are matched against this machine only: host paths stay in the
//! desktop SQLite index and never reach the server.
//!
//! Security rules:
//! - results are canonicalised and must stay under an allowlisted root (home,
//!   XDG folders, the install folder, a Wine/Proton `drive_c`);
//! - `..` components and symlinks escaping those roots are rejected;
//! - manifests are parsed as data and never executed;
//! - directory scans skip symlinks and cap the number of files and bytes.

use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;

use crate::db::{InstalledGame, SaveDetectionRecord, SavePathRecord};
use crate::saves::registry::{ResolvedFolders, SaveCandidate, SaveRegistry};
use crate::settings::LocalSettings;
use crate::steam;

/// A path found in the Ludusavi data or a CLI preview.
pub const SOURCE_MANIFEST: &str = "manifest";
/// A path the user added by hand.
pub const SOURCE_MANUAL: &str = "manual";
/// A user path pinned over a manifest entry with the same location.
pub const SOURCE_OVERRIDE: &str = "override";

/// Message shown when a game has no detected or manual save path.
pub const NO_SAVE_MESSAGE: &str = "No save folder detected for this game yet. Install the game, launch it once so it creates its saves, then run detection, or add a path manually.";

const MISSING_NOTE: &str = "The path is missing right now; launch the game once and re-detect.";

/// Directory scans stop after this many files.
const MAX_SAVE_FILES: u64 = 10_000;
/// Or after this many bytes.
const MAX_SAVE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_GLOB_MATCHES: usize = 256;
const MAX_TEMPLATE_RESULTS: usize = 64;
/// Top-level manifest entries accepted (#236). The official manifest holds
/// 53,557 games in 17.8 MB (2026-11-18), past the old 50,000 cap that rejected
/// every detection. 250,000 leaves room for years of growth while a corrupt
/// file still stops early.
const MAX_MANIFEST_GAMES: usize = 250_000;
/// Hard cap on the manifest file, shared with the downloader
/// (`saves/manifest.rs` re-exports it). The official file is 17.8 MB, so 64 MiB
/// is headroom and a real memory guard.
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
/// One game entry may not exceed this: chunked parsing is only bounded if a
/// single value cannot be arbitrarily large. Official entries are < 100 KiB.
const MAX_MANIFEST_ENTRY_BYTES: usize = 4 * 1024 * 1024;
/// Total YAML nodes (mappings, sequences, scalars) parsed across the file. The
/// official manifest has ~1.9 M; this bounds the traversal work of a file that
/// is under `MAX_MANIFEST_BYTES` but absurdly dense.
const MAX_MANIFEST_NODES: usize = 16_000_000;
/// YAML anchors/aliases are rejected: an alias bomb is a few KB that expands
/// until memory is exhausted. The official manifest uses none, so this costs
/// nothing real. `serde_yaml` has a repetition limit, but it scales with the
/// document, which is not enough for a 64 MiB file.
const MAX_MANIFEST_ALIASES: usize = 0;
/// Top-level entries parsed per `serde_yaml` document. The whole official file
/// as one document peaks around 670 MB; 4,000-entry chunks (~1.4 MB each) stay
/// near 25 MB while keeping the parse in one pass.
const MANIFEST_CHUNK_GAMES: usize = 4_000;
/// Or 4 MiB of text per chunk, whichever comes first: a file with few but fat
/// entries must not be parsed as one document either. With
/// `MAX_MANIFEST_ENTRY_BYTES` this bounds a chunk to 8 MiB and its YAML tree
/// near 130 MB in the worst case.
const MAX_MANIFEST_CHUNK_BYTES: usize = 4 * 1024 * 1024;
const MAX_STORE_USER_IDS: usize = 4;
const MAX_CLI_OUTPUT: u64 = 1024 * 1024;
const CLI_TIMEOUT: Duration = Duration::from_secs(20);
const CLI_VERSION_TIMEOUT: Duration = Duration::from_secs(5);

/// Host platform, injectable so tests run on Linux CI without a real Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    Macos,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::Macos
        } else {
            Platform::Linux
        }
    }
}

/// Native Windows folders, when the app runs on Windows itself.
#[derive(Debug, Clone, Default)]
pub struct WindowsDirs {
    pub app_data: Option<PathBuf>,
    pub local_app_data: Option<PathBuf>,
    pub local_app_data_low: Option<PathBuf>,
    pub documents: Option<PathBuf>,
    pub saved_games: Option<PathBuf>,
    pub public: Option<PathBuf>,
    pub program_data: Option<PathBuf>,
    pub win_dir: Option<PathBuf>,
}

impl WindowsDirs {
    fn from_folders(folders: &ResolvedFolders) -> Self {
        Self {
            app_data: folders.get("roaming_app_data"),
            local_app_data: folders.get("local_app_data"),
            local_app_data_low: folders.get("local_app_data_low"),
            documents: folders.get("documents"),
            saved_games: folders.get("saved_games"),
            public: folders.get("public"),
            program_data: folders.get("program_data"),
            win_dir: env_path("SYSTEMROOT").or_else(|| env_path("WINDIR")),
        }
    }

    fn roots(&self) -> Vec<PathBuf> {
        [
            &self.app_data,
            &self.local_app_data,
            &self.local_app_data_low,
            &self.documents,
            &self.saved_games,
            &self.public,
            &self.program_data,
            &self.win_dir,
        ]
        .into_iter()
        .flatten()
        .cloned()
        .collect()
    }
}

/// Everything detection needs to know about the machine.
#[derive(Debug, Clone)]
pub struct DetectEnv {
    pub home: PathBuf,
    pub os_user_name: String,
    pub platform: Platform,
    pub xdg_data: PathBuf,
    pub xdg_config: PathBuf,
    pub xdg_state: PathBuf,
    pub windows: WindowsDirs,
    /// Resolved known folders from `data/save-roots.yaml` (#146).
    pub known: ResolvedFolders,
    /// Versioned resolver registry, embedded at build time.
    pub registry: SaveRegistry,
    /// Steam library roots, used for store ground truth and prefixes.
    pub steam_roots: Vec<PathBuf>,
    /// Wine/Proton prefix containing `drive_c`, when the game is a Windows build
    /// running on Linux/macOS.
    pub prefix: Option<PathBuf>,
    /// Steam account ids found in `userdata`, for `<storeUserId>`.
    pub steam_user_ids: Vec<String>,
    /// Explicit, auto-managed or auto-discovered Ludusavi manifest file.
    pub manifest_path: Option<PathBuf>,
    /// Where `manifest_path` came from: `configured`, `managed`, `user` or
    /// `none` (#153). Only used for the human-readable message.
    pub manifest_source: String,
    /// Explicit or `PATH`-discovered `ludusavi` executable.
    pub ludusavi: Option<PathBuf>,
}

impl DetectEnv {
    /// Build the environment from settings, environment variables and the
    /// installed game. `prefix_override` is the per-game launch prefix and
    /// `app_id_override` the per-game Steam app id, both when configured.
    pub fn for_game(
        settings: &LocalSettings,
        game: &InstalledGame,
        app_data: &Path,
        prefix_override: Option<&str>,
        app_id_override: Option<u32>,
    ) -> Self {
        let platform = Platform::current();
        let known = crate::saves::registry::system_known_folders(platform);
        let home = known
            .get("home")
            .or_else(|| known.get("profile"))
            .unwrap_or_else(home_dir);
        let os_user_name = current_user_name(&home);
        let xdg_data = known
            .get("xdg_data")
            .unwrap_or_else(|| home.join(".local/share"));
        let xdg_config = known
            .get("xdg_config")
            .unwrap_or_else(|| home.join(".config"));
        let xdg_state = known
            .get("xdg_state")
            .unwrap_or_else(|| home.join(".local/state"));
        let windows = WindowsDirs::from_folders(&known);
        let mut context = GameContext::from_installed(game);
        if context.steam_app_id.is_none() {
            context.steam_app_id = app_id_override;
        }
        let steam_roots = steam::discover_roots(None);
        let prefix = resolve_prefix(
            settings,
            &home,
            &context.install_dir,
            context.steam_app_id,
            prefix_override,
        );
        let (manifest_path, manifest_source) =
            resolve_manifest_path(settings, app_data, &home, &xdg_data, &windows);

        Self {
            os_user_name,
            platform,
            xdg_data,
            xdg_config,
            xdg_state,
            windows,
            known,
            registry: crate::saves::registry::registry().clone(),
            steam_roots,
            prefix,
            steam_user_ids: discover_steam_user_ids(),
            manifest_path,
            manifest_source: manifest_source.to_string(),
            ludusavi: resolve_ludusavi(settings),
            home,
        }
    }
}

/// The game being inspected.
#[derive(Debug, Clone)]
pub struct GameContext {
    pub title: String,
    pub install_dir: PathBuf,
    pub steam_app_id: Option<u32>,
    /// `steam`, `gog`, `epic`… when known.
    pub store: Option<String>,
}

impl GameContext {
    pub fn from_installed(game: &InstalledGame) -> Self {
        let install_dir = PathBuf::from(&game.install_path);
        let steam_app_id = game.launch_uri.as_deref().and_then(steam::app_id_from_uri);
        let store = if game.source == crate::db::SOURCE_STEAM || steam_app_id.is_some() {
            Some("steam".to_string())
        } else {
            store_markers(&install_dir)
        };
        Self {
            title: game.title.clone(),
            install_dir,
            steam_app_id,
            store,
        }
    }
}

/// One detected save location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedSave {
    /// Canonical host path. Never sent to the server.
    pub path: String,
    /// `file` or `directory`.
    pub kind: String,
    /// `manifest`, `manual` or `override`.
    pub source: String,
    pub size_bytes: i64,
    pub last_modified: Option<String>,
    pub notes: Option<String>,
}

/// One known root inspected by a detection run, for the "why nothing was
/// found" report (#158). The path stays on this machine, like every other
/// detected save path.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootReport {
    pub id: String,
    pub label: String,
    pub path: String,
    /// The root exists right now.
    pub exists: bool,
    /// Candidates discovery found below this root.
    #[serde(default)]
    pub candidates: usize,
}

/// Result of one detection run, persisted as `save_detection.result_json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Detection {
    pub detected: bool,
    pub saves: Vec<DetectedSave>,
    pub total_bytes: i64,
    pub source: String,
    pub manifest_version: Option<String>,
    pub message: String,
    pub warnings: Vec<String>,
    /// Resolver candidates (#146). High-confidence ones are promoted
    /// automatically; a guess waits for a session that writes to it (#180).
    #[serde(default)]
    pub candidates: Vec<SaveCandidate>,
    /// Version of `data/save-roots.yaml` used for this run.
    #[serde(default)]
    pub registry_version: Option<u32>,
    /// Every known root that was checked, with its resolved path and whether it
    /// exists (#158). Used to explain "no save folder detected" per root.
    #[serde(default)]
    pub roots: Vec<RootReport>,
}

/// One path in the `setSavePaths` payload.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavePathInput {
    pub path: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub primary: Option<bool>,
    #[serde(default)]
    pub source: Option<String>,
}

/// Persisted mapping plus the last preview, returned by the Tauri commands.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavePaths {
    pub game_id: i64,
    pub detected: bool,
    pub saves: Vec<SavePath>,
    /// Path a future snapshot archives first.
    pub primary: Option<String>,
    pub total_bytes: i64,
    pub manifest_version: Option<String>,
    pub detected_at: Option<String>,
    pub source: String,
    /// Human-readable summary, including the explicit "no save folder detected"
    /// state.
    pub message: String,
    pub warnings: Vec<String>,
}

/// One persisted save path, enriched with the last preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavePath {
    pub path: String,
    pub kind: String,
    pub source: String,
    pub primary: bool,
    pub size_bytes: i64,
    pub last_modified: Option<String>,
    pub notes: Option<String>,
}

/// Run detection for an installed game and promote its high-confidence
/// candidates (#180). `ignored` holds the paths the user dismissed: a dismissed
/// high-confidence candidate is never promoted automatically.
#[allow(clippy::too_many_arguments)]
pub fn detect_for_game_with_ignored(
    settings: &LocalSettings,
    game: &InstalledGame,
    app_data: &Path,
    manual: &[SavePathRecord],
    prefix_override: Option<&str>,
    app_id_override: Option<u32>,
    ignored: &[String],
) -> Detection {
    let env = DetectEnv::for_game(settings, game, app_data, prefix_override, app_id_override);
    let mut context = GameContext::from_installed(game);
    if context.steam_app_id.is_none() {
        context.steam_app_id = app_id_override;
    }
    let mut detection = detect(&env, &context, manual);
    let guard = PromotionGuard::for_game(settings, game);
    let auto_promoted = auto_promote_detected(&mut detection, &guard, ignored);
    if auto_promoted > 0 {
        detection.message = format!(
            "{auto_promoted} save path(s) synced automatically ({} total).",
            detection.saves.len()
        );
    }
    detection
}

/// Run detection with an explicit environment. This is the testable core.
pub fn detect(env: &DetectEnv, game: &GameContext, manual: &[SavePathRecord]) -> Detection {
    detect_at(env, game, manual, &crate::util::now_rfc3339())
}

/// Run detection with an explicit clock, so candidates and first-seen
/// timestamps are stable in tests. Promotion is applied by
/// `detect_for_game_with_ignored`, not here.
pub fn detect_at(
    env: &DetectEnv,
    game: &GameContext,
    manual: &[SavePathRecord],
    now: &str,
) -> Detection {
    let roots = canonical_roots(env, game);
    let mut detection = Detection::default();

    let mut manual_saves = Vec::new();
    for record in manual {
        if let Some(save) = manual_save(record, &roots, &mut detection.warnings) {
            manual_saves.push(save);
        }
    }

    let mut provider_saves: Vec<DetectedSave> = Vec::new();
    let mut provider_source = "none";
    let mut manifest_version = None;
    let mut status = None;

    // 1. The manifest first: an explicit path, then the copy Ludex manages in
    //    its app data folder (#153), then a user-installed Ludusavi data
    //    folder. The CLI (below) stays an advanced optional override.
    if let Some(path) = env.manifest_path.as_deref() {
        provider_source = "manifest";
        match load_manifest(path) {
            Ok(loaded) => {
                manifest_version = Some(loaded.version);
                match loaded.manifest.find(game) {
                    ManifestMatch::Found(found) => {
                        let walk = walk_manifest(found, env, game, &roots, &mut detection.warnings);
                        provider_saves = walk.saves;
                        if !walk.missing_stores.is_empty() {
                            let names: Vec<String> = walk.missing_stores.into_iter().collect();
                            detection.warnings.push(format!(
                                "Some manifest entries need a store Ludex could not identify ({}); they were skipped.",
                                names.join(", ")
                            ));
                        }
                        if provider_saves.is_empty() {
                            status = Some(
                                if found.registry_keys > 0 {
                                    "The Ludusavi manifest only lists registry locations for this game. Registry saves are not archived yet; add a save path manually if the game also writes files."
                                } else if walk.skipped_filters > 0 {
                                    "Every manifest entry for this game is filtered out on this machine (OS or store). Add a save path manually."
                                } else {
                                    "No save files exist yet for this game. Launch it once so it creates them, then re-detect."
                                }
                                .to_string(),
                            );
                        }
                    }
                    ManifestMatch::NotFound => {
                        status = Some("This game is not in the Ludusavi manifest. Add a save path manually, or re-detect after the manifest updates.".to_string());
                    }
                    ManifestMatch::Ambiguous => {
                        detection.warnings.push(format!(
                            "Several Ludusavi manifest entries match \"{}\"; Ludex did not guess.",
                            game.title
                        ));
                        status = Some("Several manifest entries match this game, so no save folder was picked. Add a save path manually.".to_string());
                    }
                }
            }
            Err(ManifestLoadError::TooLarge(bytes)) => {
                detection.warnings.push(format!(
                    "{} is {} ({} bytes), larger than the {} Ludex parses; it was not parsed.",
                    path.display(),
                    crate::util::human_bytes(bytes),
                    bytes,
                    crate::util::human_bytes(MAX_MANIFEST_BYTES)
                ));
                status = Some(format!(
                    "The Ludusavi manifest is {} ({} bytes), larger than the {} Ludex parses safely. Update Ludex, or add a save path manually.",
                    crate::util::human_bytes(bytes),
                    bytes,
                    crate::util::human_bytes(MAX_MANIFEST_BYTES)
                ));
            }
            Err(ManifestLoadError::Parse(error)) => {
                detection.warnings.push(error.clone());
                status = Some(format!("The Ludusavi manifest could not be read: {error}"));
            }
            Err(ManifestLoadError::Read(error)) => {
                detection
                    .warnings
                    .push(format!("Could not read {}: {error}", path.display()));
                status = Some(format!(
                    "The Ludusavi manifest at {} could not be read.",
                    path.display()
                ));
            }
        }
    }

    // 2. A `ludusavi` binary knows its own manifest and reconciliation rules:
    //    it is used when the manifest file listed nothing (or is absent).
    if provider_saves.is_empty() {
        if let Some(binary) = env.ludusavi.as_deref() {
            match ludusavi_preview(binary, &game.title) {
                Ok(preview) => {
                    let cli_version = preview.version.clone();
                    let saves = api_saves(&preview.paths, &roots, &mut detection.warnings);
                    if !saves.is_empty() {
                        provider_saves = saves;
                        provider_source = "ludusavi";
                        manifest_version = cli_version;
                        status = None;
                    } else if env.manifest_path.is_some() {
                        detection.warnings.push(
                            "The Ludusavi CLI listed no save files; Ludex is using the manifest result."
                                .to_string(),
                        );
                    } else {
                        provider_source = "ludusavi";
                        manifest_version = cli_version;
                        status = Some("The Ludusavi CLI did not list any existing save files for this game. Launch it once so it creates them, then re-detect.".to_string());
                    }
                }
                Err(error) => {
                    if env.manifest_path.is_none() {
                        status = Some("The Ludusavi CLI could not preview this game and no manifest file is configured. Check the ludusaviBinary setting, or add a save path manually.".to_string());
                    }
                    detection.warnings.push(format!(
                        "The Ludusavi CLI preview failed ({error}); {}",
                        if env.manifest_path.is_some() {
                            "using the manifest file"
                        } else {
                            "no manifest file is configured"
                        }
                    ));
                }
            }
        } else if env.manifest_path.is_none() {
            status = Some("No Ludusavi manifest or CLI was found. Ludex can download the official manifest in Settings → Saves, or add a save path manually.".to_string());
        }
    }

    let combined: Vec<DetectedSave> = manual_saves.into_iter().chain(provider_saves).collect();
    detection.saves = aggregate(combined);

    // 3. Known roots and engine defaults (#146). These are candidates: they
    //    are shown with their root, provenance and confidence, and the
    //    high-confidence ones are promoted automatically (#180). Manifest
    //    results above stay the trusted per-game layer and are de-duplicated
    //    here.
    let discovery = crate::saves::registry::discover(&crate::saves::registry::DiscoveryInputs {
        platform: env.platform,
        user_name: &env.os_user_name,
        folders: &env.known,
        prefix: env.prefix.as_deref(),
        title: &game.title,
        steam_app_id: game.steam_app_id,
        steam_roots: &env.steam_roots,
        steam_user_ids: &env.steam_user_ids,
        roots: &roots,
        now,
        registry: &env.registry,
        overrides: crate::saves::registry::overrides(),
    });
    let saved_paths: BTreeSet<String> = detection
        .saves
        .iter()
        .map(|save| save.path.clone())
        .collect();
    detection.roots = root_reports(env, &discovery.candidates);
    detection.candidates = discovery
        .candidates
        .into_iter()
        .filter(|candidate| !saved_paths.contains(&candidate.path))
        .collect();
    detection.warnings.extend(discovery.warnings);
    detection.registry_version = Some(env.registry.version);

    let manual_count = detection
        .saves
        .iter()
        .filter(|save| save.source != SOURCE_MANIFEST)
        .count();
    let provider_count = detection.saves.len().saturating_sub(manual_count);
    detection.source = if provider_count > 0 {
        provider_source.to_string()
    } else if manual_count > 0 {
        "manual".to_string()
    } else {
        provider_source.to_string()
    };
    detection.detected = detection.saves.iter().any(|save| !is_missing(save));
    detection.total_bytes = detection
        .saves
        .iter()
        .map(|save| save.size_bytes.max(0))
        .sum();

    let provider_label = if provider_source == "ludusavi" {
        "the Ludusavi CLI"
    } else if env.manifest_source == "managed" {
        "the auto-managed Ludusavi manifest"
    } else {
        "the Ludusavi manifest"
    };
    detection.message = if provider_count > 0 && manual_count > 0 {
        format!(
            "{} save path(s): {provider_count} detected with {provider_label} and {manual_count} configured manually.",
            detection.saves.len()
        )
    } else if provider_count > 0 {
        format!("{provider_count} save path(s) detected with {provider_label}.")
    } else if manual_count > 0 {
        if detection.detected {
            format!("{manual_count} manually configured save path(s).")
        } else {
            "The configured save path is missing right now. Launch the game once, then re-detect."
                .to_string()
        }
    } else if detection
        .candidates
        .iter()
        .any(|candidate| candidate.auto_status != CANDIDATE_STATUS_SAVING)
    {
        let waiting: Vec<_> = detection
            .candidates
            .iter()
            .filter(|candidate| candidate.auto_status != CANDIDATE_STATUS_SAVING)
            .collect();
        let guesses = waiting
            .iter()
            .filter(|candidate| candidate.is_guess())
            .count();
        if guesses == waiting.len() {
            format!(
                "{guesses} guessed save location(s) were found from engine defaults; the folder that changes during a session is synced automatically."
            )
        } else {
            format!(
                "{} possible save location(s) found; the folder that changes during a session is synced automatically.",
                waiting.len()
            )
        }
    } else {
        status.unwrap_or_else(|| NO_SAVE_MESSAGE.to_string())
    };

    // #158: say *why* nothing was found, root by root, instead of an opaque
    // "no paths". Only when the run produced neither saves nor candidates.
    if detection.saves.is_empty() && detection.candidates.is_empty() {
        if let Some(explanation) = explain_roots(&detection.roots) {
            detection.message.push(' ');
            detection.message.push_str(&explanation);
        }
        if !detection.roots.is_empty() && detection.roots.iter().all(|root| !root.exists) {
            detection.warnings.push(
                "None of the known save folders exists on this machine. Ludex resolves them through the OS; if they are missing, the save files are likely on another drive or this is a fresh profile."
                    .to_string(),
            );
        }
    }

    detection.warnings.sort();
    detection.warnings.dedup();
    detection.manifest_version = manifest_version;
    detection
}

/// Validate one save path against the allowlisted roots. Used by manual edits
/// and by candidate confirmation (#146). The path must exist, be readable and
/// stay inside a known root; returns the canonical path and its kind.
pub fn validate_save_path(
    env: &DetectEnv,
    game: &GameContext,
    raw: &str,
) -> Result<(PathBuf, String), String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("A save path cannot be empty.".to_string());
    }
    let roots = canonical_roots(env, game);
    let path = manual_path(raw, env);
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!("Refusing a save path containing '..': {raw}"));
    }

    let canonical = path
        .canonicalize()
        .map_err(|error| format!("The save path {raw} is not accessible: {error}"))?;
    if !inside_roots(&canonical, &roots) {
        return Err(format!(
            "Refusing {} because it is outside the allowed save locations (home, XDG folders, install folder, Wine prefix).",
            canonical.display()
        ));
    }

    let metadata = fs::metadata(&canonical)
        .map_err(|error| format!("Could not read {}: {error}", canonical.display()))?;
    let kind = if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "file"
    } else {
        return Err(format!(
            "{} is not a file or a folder.",
            canonical.display()
        ));
    };

    // Test readability before persisting.
    if kind == "directory" {
        fs::read_dir(&canonical)
            .map_err(|error| format!("Could not list {}: {error}", canonical.display()))?;
    } else {
        fs::File::open(&canonical)
            .map_err(|error| format!("Could not read {}: {error}", canonical.display()))?;
    }

    Ok((canonical, kind.to_string()))
}

/// Describe an existing path as a detected save (size, modification time and
/// scan notes). Used when a confirmed candidate is folded into the snapshot.
pub fn measured_save(path: &Path, source: &str) -> DetectedSave {
    let measured = measure(path);
    DetectedSave {
        path: path.to_string_lossy().to_string(),
        kind: measured.kind,
        source: source.to_string(),
        size_bytes: measured.size,
        last_modified: measured.last_modified,
        notes: join_notes(measured.notes),
    }
}

/* -------------------------------------------------- automatic promotion (#180) */

/// Automatic status stored on a candidate: it is a save path and is archived.
pub const CANDIDATE_STATUS_SAVING: &str = "saving";
/// A guess that no session has proven yet.
pub const CANDIDATE_STATUS_WAITING: &str = "waiting";
/// A candidate rejected by the safety rules, with a readable reason.
pub const CANDIDATE_STATUS_SKIPPED: &str = "skipped";

/// Snapshot/measurement walk caps: a candidate is one save folder, not a disk.
const MAX_SNAPSHOT_FILES: u64 = 20_000;
const MAX_SNAPSHOT_DEPTH: usize = 32;
/// Per-path budget for the measurements taken around a launch (#180).
pub const SNAPSHOT_BUDGET: Duration = Duration::from_millis(250);

/// Bounded measurement of one candidate path. `truncated` means the walk hit a
/// cap/deadline before seeing everything, so the caller must not treat the
/// numbers as a complete save folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathMeasurement {
    pub exists: bool,
    pub is_dir: bool,
    pub files: u64,
    pub bytes: u64,
    pub newest: Option<String>,
    pub truncated: bool,
}

/// Walk one path without following symlinks and with a hard file/depth/deadline
/// cap. Used before and after a session to prove which folder the game wrote.
pub fn measure_path(path: &Path, budget: Duration) -> PathMeasurement {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return PathMeasurement::default(),
    };
    if metadata.file_type().is_symlink() {
        return PathMeasurement::default();
    }
    if metadata.is_file() {
        return PathMeasurement {
            exists: true,
            is_dir: false,
            files: 1,
            bytes: metadata.len(),
            newest: metadata.modified().ok().map(rfc3339),
            truncated: false,
        };
    }
    if !metadata.is_dir() {
        return PathMeasurement::default();
    }

    let deadline = std::time::Instant::now() + budget;
    let mut measurement = PathMeasurement {
        exists: true,
        is_dir: true,
        ..PathMeasurement::default()
    };
    let mut newest: Option<SystemTime> = None;
    let mut seen = 0u64;
    let mut stack: Vec<(PathBuf, usize)> = vec![(path.to_path_buf(), 0)];

    while let Some((current, depth)) = stack.pop() {
        seen += 1;
        if seen > MAX_SNAPSHOT_FILES.saturating_mul(2)
            || measurement.files >= MAX_SNAPSHOT_FILES
            || depth > MAX_SNAPSHOT_DEPTH
        {
            measurement.truncated = true;
            break;
        }
        if seen.is_multiple_of(256) && std::time::Instant::now() >= deadline {
            measurement.truncated = true;
            break;
        }
        let Ok(metadata) = fs::symlink_metadata(&current) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            if let Ok(entries) = fs::read_dir(&current) {
                for entry in entries.flatten() {
                    stack.push((entry.path(), depth + 1));
                    // A directory with an absurd number of children must not
                    // grow the stack without bound before the file cap hits.
                    if stack.len() > MAX_SNAPSHOT_FILES.saturating_mul(2) as usize {
                        measurement.truncated = true;
                        break;
                    }
                }
                if measurement.truncated {
                    break;
                }
            }
            continue;
        }
        if metadata.is_file() {
            measurement.files = measurement.files.saturating_add(1);
            measurement.bytes = measurement.bytes.saturating_add(metadata.len());
            if let Ok(modified) = metadata.modified() {
                newest = Some(newest.map_or(modified, |current| current.max(modified)));
            }
        }
    }

    measurement.newest = newest.map(rfc3339);
    measurement
}

/// Safety rules for automatic promotion (#180). Manual confirmation bypasses
/// this guard: the user is allowed to pick a broad folder on purpose.
#[derive(Debug, Clone)]
pub struct PromotionGuard {
    home: Option<PathBuf>,
    broad_roots: Vec<PathBuf>,
    install_dir: Option<PathBuf>,
    max_bytes: u64,
}

impl PromotionGuard {
    pub fn for_game(settings: &LocalSettings, game: &InstalledGame) -> Self {
        let known = crate::saves::registry::system_known_folders(Platform::current());
        let home = known
            .get("home")
            .or_else(|| known.get("profile"))
            .unwrap_or_else(home_dir);
        Self::from_parts(
            home,
            known.values().cloned().collect(),
            Some(PathBuf::from(&game.install_path)).filter(|path| path.is_dir()),
            settings
                .cloud_save_folder_cap_mb
                .saturating_mul(1024 * 1024),
        )
    }

    fn from_parts(
        home: PathBuf,
        broad_roots: Vec<PathBuf>,
        install_dir: Option<PathBuf>,
        max_bytes: u64,
    ) -> Self {
        let mut broad_roots = broad_roots;
        broad_roots.sort();
        broad_roots.dedup();
        Self {
            home: Some(home),
            broad_roots,
            install_dir,
            max_bytes,
        }
    }

    /// `Ok` when the candidate may be archived automatically; `Err` carries a
    /// sentence the panel can show as "Skipped: …".
    pub fn check(&self, path: &Path, kind: &str, measured: &PathMeasurement) -> Result<(), String> {
        if has_symlink_component(path) {
            return Err("The path goes through a symlink.".to_string());
        }
        if !measured.exists {
            return Err("The folder does not exist.".to_string());
        }
        if measured.truncated {
            return Err(format!(
                "The folder is too large or too slow to inspect safely (over {MAX_SNAPSHOT_FILES} files)."
            ));
        }
        let canonical = path
            .canonicalize()
            .map_err(|_| "The folder cannot be resolved.".to_string())?;
        if canonical.parent().is_none() {
            return Err("This is a drive root, not a save folder.".to_string());
        }
        if let Some(home) = &self.home {
            if same_path(&canonical, home) {
                return Err("This is your home folder, not a save folder.".to_string());
            }
        }
        for root in &self.broad_roots {
            if same_path(&canonical, root) {
                let name = root
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| root.to_string_lossy().to_string());
                return Err(format!(
                    "This is the known {name} folder root, not a game save folder."
                ));
            }
        }
        if let Some(install_dir) = &self.install_dir {
            if same_path(&canonical, install_dir) {
                return Err("This is the game install folder, not a save folder.".to_string());
            }
        }
        if self.max_bytes > 0 && measured.bytes > self.max_bytes {
            return Err(format!(
                "The folder is larger than the {} automatic limit.",
                crate::util::human_bytes(self.max_bytes)
            ));
        }
        if kind != "directory" && kind != "file" {
            return Err("The path is not a file or a folder.".to_string());
        }
        Ok(())
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

/// True when any component of `path` is a symlink, including its parents. A
/// guessed folder reached through a link can escape the allowlisted roots.
fn has_symlink_component(path: &Path) -> bool {
    let mut current = Some(path);
    while let Some(candidate) = current {
        if let Ok(metadata) = fs::symlink_metadata(candidate) {
            if metadata.file_type().is_symlink() {
                return true;
            }
        }
        current = candidate.parent();
    }
    false
}

fn detected_from_measurement(
    path: &Path,
    source: &str,
    measured: &PathMeasurement,
) -> DetectedSave {
    DetectedSave {
        path: path.to_string_lossy().to_string(),
        kind: if measured.is_dir { "directory" } else { "file" }.to_string(),
        source: source.to_string(),
        size_bytes: i64::try_from(measured.bytes).unwrap_or(i64::MAX),
        last_modified: measured.newest.clone(),
        notes: if measured.truncated {
            Some(format!("Scan capped at {MAX_SNAPSHOT_FILES} files."))
        } else {
            None
        },
    }
}

/// Promote the high-confidence candidates of one detection run (#180): Steam
/// ground truth and paths the user already confirmed become save paths without
/// a prompt. Guesses stay candidates and wait for a session.
///
/// Returns the number of promoted paths. Skipped candidates keep a readable
/// `auto_reason` so Settings can explain the decision. `ignored` paths are
/// never promoted: a dismissal wins over the automatic guess.
pub fn auto_promote_detected(
    detection: &mut Detection,
    guard: &PromotionGuard,
    ignored: &[String],
) -> usize {
    let mut promoted: Vec<DetectedSave> = Vec::new();
    for candidate in detection.candidates.iter_mut() {
        if !crate::saves::registry::is_high_confidence(candidate) {
            continue;
        }
        if ignored.iter().any(|path| path == &candidate.path) {
            candidate.auto_status = CANDIDATE_STATUS_SKIPPED.to_string();
            candidate.auto_reason = Some("You ignored this folder.".to_string());
            continue;
        }
        let path = PathBuf::from(&candidate.path);
        let measured = measure_path(&path, SNAPSHOT_BUDGET);
        match guard.check(&path, &candidate.kind, &measured) {
            Ok(()) => {
                candidate.auto_status = CANDIDATE_STATUS_SAVING.to_string();
                candidate.auto_reason = None;
                candidate.confirmed = true;
                promoted.push(detected_from_measurement(
                    &path,
                    &candidate.source,
                    &measured,
                ));
            }
            Err(reason) => {
                candidate.auto_status = CANDIDATE_STATUS_SKIPPED.to_string();
                candidate.auto_reason = Some(reason);
            }
        }
    }
    if promoted.is_empty() {
        return 0;
    }
    let count = promoted.len();
    detection.saves.extend(promoted);
    detection.saves = aggregate(std::mem::take(&mut detection.saves));
    detection.detected = detection.saves.iter().any(|save| !is_missing(save));
    detection.total_bytes = detection
        .saves
        .iter()
        .map(|save| save.size_bytes.max(0))
        .sum();
    count
}

/// True when a session changed a candidate: it existed before the session,
/// still exists, and its bytes or newest file timestamp moved.
pub fn session_changed(
    before: &crate::db::CandidateSnapshotRecord,
    after: &PathMeasurement,
) -> bool {
    if !before.existed || !after.exists || after.files == 0 {
        return false;
    }
    if after.bytes != before.bytes {
        return true;
    }
    match (
        before
            .newest
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok()),
        after
            .newest
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok()),
    ) {
        (Some(before), Some(after)) => after > before,
        (None, Some(_)) => true,
        _ => false,
    }
}

/// Bytes added by the session (never negative). Used to rank changed paths.
pub fn session_changed_bytes(
    before: &crate::db::CandidateSnapshotRecord,
    after: &PathMeasurement,
) -> u64 {
    after.bytes.saturating_sub(before.bytes)
}

/// One changed candidate considered for promotion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotionTarget {
    pub path: String,
    pub changed_bytes: u64,
    pub total_bytes: u64,
}

/// Deterministic primary: largest changed byte count, then largest total, then
/// the lexicographically smallest path.
pub fn choose_promotion_primary(targets: &[PromotionTarget]) -> Option<String> {
    targets
        .iter()
        .max_by(|left, right| {
            left.changed_bytes
                .cmp(&right.changed_bytes)
                .then(left.total_bytes.cmp(&right.total_bytes))
                .then_with(|| right.path.cmp(&left.path))
        })
        .map(|target| target.path.clone())
}

/// Validate and persist a manual override list. The whole list replaces the
/// previous mapping: every path must exist, be readable and stay inside an
/// allowlisted root.
pub fn manual_override(
    env: &DetectEnv,
    game: &GameContext,
    game_id: i64,
    inputs: &[SavePathInput],
    now: &str,
) -> Result<(Vec<SavePathRecord>, Detection), String> {
    let mut records: Vec<SavePathRecord> = Vec::new();
    let mut saves: Vec<DetectedSave> = Vec::new();
    let mut primary_taken = false;

    for input in inputs {
        let raw = input.path.trim();
        let (canonical, kind) = validate_save_path(env, game, raw)?;
        let kind = kind.as_str();

        if let Some(requested) = input
            .kind
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if !requested.eq_ignore_ascii_case(kind) {
                return Err(format!(
                    "{} is a {kind}, not a {requested}.",
                    canonical.display()
                ));
            }
        }

        let path_string = canonical.to_string_lossy().to_string();
        if records.iter().any(|record| record.path == path_string) {
            continue;
        }

        let primary = input.primary.unwrap_or(false);
        if primary && primary_taken {
            return Err("Only one save path can be primary.".to_string());
        }

        let source = match input
            .source
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some("override") => SOURCE_OVERRIDE,
            Some("manual") | None => SOURCE_MANUAL,
            Some(other) => {
                return Err(format!(
                    "Unknown save path source {other}; use manual or override."
                ))
            }
        };

        let measured = measure(&canonical);
        saves.push(DetectedSave {
            path: path_string.clone(),
            kind: kind.to_string(),
            source: source.to_string(),
            size_bytes: measured.size,
            last_modified: measured.last_modified,
            notes: join_notes(measured.notes),
        });
        records.push(SavePathRecord {
            game_id,
            path: path_string,
            kind: kind.to_string(),
            source: source.to_string(),
            primary,
            created_at: now.to_string(),
        });
        if primary {
            primary_taken = true;
        }
    }

    if !records.is_empty() && !primary_taken {
        records[0].primary = true;
    }

    let total_bytes = saves.iter().map(|save| save.size_bytes.max(0)).sum();
    let manual_count = saves.len();
    let detection = Detection {
        detected: !saves.is_empty(),
        saves,
        total_bytes,
        source: if records
            .iter()
            .any(|record| record.source == SOURCE_OVERRIDE)
        {
            SOURCE_OVERRIDE.to_string()
        } else {
            SOURCE_MANUAL.to_string()
        },
        manifest_version: None,
        message: if manual_count == 0 {
            "No manual save paths. Run detection to find this game's save folders.".to_string()
        } else {
            format!("{manual_count} manually configured save path(s).")
        },
        warnings: Vec::new(),
        ..Detection::default()
    };
    Ok((records, detection))
}

/// Build the persisted command result from the DB rows and the last preview.
pub fn persisted(
    game_id: i64,
    rows: Vec<SavePathRecord>,
    detection: Option<SaveDetectionRecord>,
) -> SavePaths {
    let snapshot: Option<Detection> = detection
        .as_ref()
        .and_then(|record| serde_json::from_str(&record.result_json).ok());
    let primary = rows
        .iter()
        .find(|row| row.primary)
        .map(|row| row.path.clone())
        .or_else(|| rows.first().map(|row| row.path.clone()));

    let saves: Vec<SavePath> = rows
        .iter()
        .map(|row| {
            let detail = snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.saves.iter().find(|save| save.path == row.path));
            SavePath {
                path: row.path.clone(),
                kind: row.kind.clone(),
                source: row.source.clone(),
                primary: primary.as_deref() == Some(row.path.as_str()),
                size_bytes: detail.map(|save| save.size_bytes).unwrap_or(0),
                last_modified: detail.and_then(|save| save.last_modified.clone()),
                notes: detail.and_then(|save| save.notes.clone()),
            }
        })
        .collect();

    let detected = snapshot
        .as_ref()
        .map(|snapshot| snapshot.detected)
        .unwrap_or(!rows.is_empty());
    let message = snapshot
        .as_ref()
        .map(|snapshot| snapshot.message.clone())
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| {
            if rows.is_empty() {
                NO_SAVE_MESSAGE.to_string()
            } else {
                format!("{} save path(s) configured.", rows.len())
            }
        });

    SavePaths {
        game_id,
        detected,
        total_bytes: snapshot
            .as_ref()
            .map(|snapshot| snapshot.total_bytes)
            .unwrap_or_else(|| saves.iter().map(|save| save.size_bytes.max(0)).sum()),
        source: snapshot
            .as_ref()
            .map(|snapshot| snapshot.source.clone())
            .unwrap_or_else(|| {
                rows.first()
                    .map(|row| row.source.clone())
                    .unwrap_or_else(|| "none".to_string())
            }),
        manifest_version: detection
            .as_ref()
            .and_then(|record| record.manifest_version.clone())
            .or_else(|| {
                snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.manifest_version.clone())
            }),
        detected_at: detection.as_ref().map(|record| record.detected_at.clone()),
        warnings: snapshot
            .as_ref()
            .map(|snapshot| snapshot.warnings.clone())
            .unwrap_or_default(),
        primary,
        saves,
        message,
    }
}

/// Choose the primary path: keep the previous one when it still exists, else
/// the first usable save.
pub fn choose_primary(saves: &[DetectedSave], preferred: Option<&str>) -> Option<String> {
    if let Some(preferred) = preferred {
        if saves.iter().any(|save| save.path == preferred) {
            return Some(preferred.to_string());
        }
    }
    saves
        .iter()
        .find(|save| !is_missing(save))
        .or_else(|| saves.first())
        .map(|save| save.path.clone())
}

/// Turn a detection result into the rows persisted in `save_paths`.
pub fn records_from_detection(
    game_id: i64,
    detection: &Detection,
    primary: Option<&str>,
    now: &str,
) -> Vec<SavePathRecord> {
    detection
        .saves
        .iter()
        .map(|save| SavePathRecord {
            game_id,
            path: save.path.clone(),
            kind: save.kind.clone(),
            source: save.source.clone(),
            primary: primary == Some(save.path.as_str()),
            created_at: now.to_string(),
        })
        .collect()
}

fn source_rank(source: &str) -> u8 {
    if source == SOURCE_MANIFEST {
        1
    } else {
        0
    }
}

/// De-duplicate by canonical path and drop children of an already kept
/// directory. User paths win over manifest paths for the same location.
fn aggregate(mut saves: Vec<DetectedSave>) -> Vec<DetectedSave> {
    saves.sort_by(|left, right| {
        left.path
            .len()
            .cmp(&right.path.len())
            .then(source_rank(&left.source).cmp(&source_rank(&right.source)))
            .then(left.path.cmp(&right.path))
    });

    let mut kept: Vec<DetectedSave> = Vec::new();
    for save in saves {
        if kept.iter().any(|existing| existing.path == save.path) {
            continue;
        }
        let path = Path::new(&save.path);
        if kept.iter().any(|existing| {
            existing.kind == "directory" && path.starts_with(Path::new(&existing.path))
        }) {
            continue;
        }
        kept.push(save);
    }

    kept.sort_by(|left, right| {
        source_rank(&left.source)
            .cmp(&source_rank(&right.source))
            .then(left.path.cmp(&right.path))
    });
    kept
}

fn is_missing(save: &DetectedSave) -> bool {
    save.notes
        .as_deref()
        .map(|notes| notes.contains(MISSING_NOTE))
        .unwrap_or(false)
}

fn join_notes(notes: Vec<String>) -> Option<String> {
    if notes.is_empty() {
        None
    } else {
        Some(notes.join("; "))
    }
}

// ---------------------------------------------------------------------------
// Environment discovery
// ---------------------------------------------------------------------------

fn home_dir() -> PathBuf {
    env_path("HOME")
        .or_else(|| env_path("USERPROFILE"))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn current_user_name(home: &Path) -> String {
    env_path("USER")
        .or_else(|| env_path("USERNAME"))
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .or_else(|| {
            home.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "player".to_string())
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

fn resolve_ludusavi(settings: &LocalSettings) -> Option<PathBuf> {
    let configured = settings.ludusavi_binary.trim();
    if !configured.is_empty() {
        return Some(PathBuf::from(configured));
    }
    crate::paths::find_on_path("ludusavi")
}

/// Resolve the manifest to read, in precedence order (#153):
///
/// 1. `ludusaviManifestPath`, when the user set one;
/// 2. the copy Ludex downloaded into `{appDataDir}/manifest/`;
/// 3. a manifest from a user-installed Ludusavi data folder.
///
/// A configured path that no longer exists still wins over nothing so the
/// command can report *why* it could not be read, but an existing managed or
/// user copy is preferred over a broken override.
fn resolve_manifest_path(
    settings: &LocalSettings,
    app_data: &Path,
    home: &Path,
    xdg_data: &Path,
    windows: &WindowsDirs,
) -> (Option<PathBuf>, &'static str) {
    let configured = settings.ludusavi_manifest_path.trim();
    let configured_path = (!configured.is_empty()).then(|| PathBuf::from(configured));

    if let Some(path) = configured_path.as_ref().filter(|path| path.is_file()) {
        return (Some(path.clone()), "configured");
    }

    let managed = crate::saves::manifest::ManifestPaths::for_app_data(app_data).manifest;
    if managed.is_file() {
        return (Some(managed), "managed");
    }

    if let Some(path) = default_manifest_paths(home, xdg_data, windows)
        .into_iter()
        .find(|path| path.is_file())
    {
        return (Some(path), "user");
    }

    if configured_path.is_some() {
        (configured_path, "configured")
    } else {
        (None, "none")
    }
}

fn default_manifest_paths(home: &Path, xdg_data: &Path, windows: &WindowsDirs) -> Vec<PathBuf> {
    let mut paths = vec![
        xdg_data.join("ludusavi/manifest.yaml"),
        home.join(".local/share/ludusavi/manifest.yaml"),
        home.join(".var/app/com.github.mtkennerly.ludusavi/data/ludusavi/manifest.yaml"),
        home.join(".config/ludusavi/manifest.yaml"),
        home.join("Library/Application Support/ludusavi/manifest.yaml"),
    ];
    if let Some(app_data) = &windows.app_data {
        paths.push(app_data.join("ludusavi/manifest.yaml"));
    }
    if let Some(local_app_data) = &windows.local_app_data {
        paths.push(local_app_data.join("ludusavi/manifest.yaml"));
    }
    paths
}

fn resolve_prefix(
    settings: &LocalSettings,
    home: &Path,
    install_dir: &Path,
    app_id: Option<u32>,
    prefix_override: Option<&str>,
) -> Option<PathBuf> {
    if let Some(prefix) = prefix_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(PathBuf::from(prefix));
    }
    let configured = settings.wine_prefix.trim();
    if !configured.is_empty() {
        return Some(PathBuf::from(configured));
    }
    if let Some(prefix) = env_path("WINEPREFIX") {
        return Some(prefix);
    }
    if let Some(app_id) = app_id {
        for root in steam::discover_roots(None) {
            let prefix = root
                .join("steamapps")
                .join("compatdata")
                .join(app_id.to_string())
                .join("pfx");
            if prefix.is_dir() {
                return Some(prefix);
            }
        }
    }
    // Ludex's own Linux launcher defaults to `{installDir}/.prefix`.
    let game_prefix = install_dir.join(".prefix");
    if game_prefix.is_dir() {
        return Some(game_prefix);
    }
    let wine = home.join(".wine");
    if wine.is_dir() {
        Some(wine)
    } else {
        None
    }
}

fn discover_steam_user_ids() -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for root in steam::discover_roots(None) {
        let Ok(entries) = fs::read_dir(root.join("userdata")) else {
            continue;
        };
        let mut found: Vec<String> = entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()))
            .collect();
        found.sort();
        for id in found {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        if ids.len() >= MAX_STORE_USER_IDS {
            break;
        }
    }
    ids.truncate(MAX_STORE_USER_IDS);
    ids
}

/// Markers used to guess a store for a Ludex-managed install.
fn store_markers(install_dir: &Path) -> Option<String> {
    let entries = fs::read_dir(install_dir).ok()?;
    let mut markers: Vec<&str> = Vec::new();
    for entry in entries.flatten().take(500) {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name.starts_with("goggame-") && name.ends_with(".info") {
            return Some("gog".to_string());
        }
        if name == "steam_appid.txt" || (name.starts_with("steam_api") && name.ends_with(".dll")) {
            markers.push("steam");
        }
        if name.ends_with(".egstore") {
            markers.push("epic");
        }
        if name == "galaxy.dll" {
            markers.push("gog");
        }
    }
    markers.first().map(|store| (*store).to_string())
}

// ---------------------------------------------------------------------------
// Path roots and security
// ---------------------------------------------------------------------------

fn prefix_drive_c(prefix: &Path) -> PathBuf {
    let drive_c = prefix.join("drive_c");
    if drive_c.is_dir() || !prefix.ends_with("drive_c") {
        drive_c
    } else {
        prefix.to_path_buf()
    }
}

/// Canonical allowlisted roots used by detection and candidate discovery.
/// Exposed so Settings → Saves can run the registry without the full Ludusavi
/// manifest pass (#146).
pub fn save_roots(env: &DetectEnv, game: &GameContext) -> Vec<PathBuf> {
    canonical_roots(env, game)
}

fn canonical_roots(env: &DetectEnv, game: &GameContext) -> Vec<PathBuf> {
    let mut roots = vec![
        env.home.clone(),
        env.xdg_data.clone(),
        env.xdg_config.clone(),
        env.xdg_state.clone(),
        game.install_dir.clone(),
    ];
    if let Some(prefix) = &env.prefix {
        roots.push(prefix_drive_c(prefix));
    }
    roots.extend(env.steam_roots.iter().cloned());
    roots.extend(env.known.values().cloned());
    roots.extend(env.windows.roots());

    let mut canonical: Vec<PathBuf> = Vec::new();
    for root in roots {
        let root = root.canonicalize().unwrap_or(root);
        if !canonical.contains(&root) {
            canonical.push(root);
        }
    }
    canonical
}

fn inside_roots(candidate: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| candidate.starts_with(root))
}

/// Known roots worth reporting per game (#158). Home and the XDG config/state
/// folders are omitted: they are not save locations a user expects to see, and
/// listing them would bury the useful roots.
fn root_reports(env: &DetectEnv, candidates: &[SaveCandidate]) -> Vec<RootReport> {
    let mut roots: Vec<RootReport> = Vec::new();
    for (id, path) in env.known.iter() {
        let Some(label) = root_label(id) else {
            continue;
        };
        roots.push(RootReport {
            id: id.clone(),
            label: label.to_string(),
            path: path.to_string_lossy().to_string(),
            exists: path.is_dir(),
            candidates: candidates
                .iter()
                .filter(|candidate| candidate.root_id == *id)
                .count(),
        });
    }
    if let Some(prefix) = env.prefix.as_deref() {
        let drive_c = prefix_drive_c(prefix);
        roots.push(RootReport {
            id: crate::saves::registry::ROOT_PROTON_PREFIX.to_string(),
            label: "Proton prefix".to_string(),
            path: drive_c.to_string_lossy().to_string(),
            exists: drive_c.is_dir(),
            candidates: candidates
                .iter()
                .filter(|candidate| candidate.root_id == crate::saves::registry::ROOT_PROTON_PREFIX)
                .count(),
        });
    }
    // Most useful first, so the "why nothing was found" sentence starts with
    // Saved Games/Documents instead of an arbitrary BTreeMap order.
    roots.sort_by_key(|root| (root_priority(&root.id), root.id.clone()));
    roots
}

fn root_priority(id: &str) -> u8 {
    match id {
        "saved_games" => 0,
        "documents" => 1,
        "local_app_data_low" => 2,
        "local_app_data" => 3,
        "roaming_app_data" => 4,
        "program_data" => 5,
        "public" => 6,
        "proton_prefix" => 7,
        "xdg_data" => 8,
        "application_support" => 9,
        "containers" => 10,
        "flatpak_apps" => 11,
        "preferences" => 12,
        _ => 13,
    }
}

fn root_label(id: &str) -> Option<&'static str> {
    match id {
        "saved_games" => Some("Saved Games"),
        "documents" => Some("Documents"),
        "local_app_data_low" => Some("AppData/LocalLow"),
        "local_app_data" => Some("AppData/Local"),
        "roaming_app_data" => Some("AppData/Roaming"),
        "program_data" => Some("ProgramData"),
        "public" => Some("Public"),
        "xdg_data" => Some("application data"),
        "application_support" => Some("Application Support"),
        "flatpak_apps" => Some("Flatpak app data"),
        "containers" => Some("sandbox containers"),
        "preferences" => Some("Preferences"),
        _ => None,
    }
}

/// Human-readable "checked roots" sentence for a run that found nothing. It is
/// deliberately short (at most six roots) so the game page stays readable.
fn explain_roots(roots: &[RootReport]) -> Option<String> {
    if roots.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = roots
        .iter()
        .take(6)
        .map(|root| {
            if root.exists {
                format!("{} (no matching folder)", root.label)
            } else {
                format!("{} (missing)", root.label)
            }
        })
        .collect();
    if roots.len() > 6 {
        parts.push(format!("and {} more", roots.len() - 6));
    }
    Some(format!(
        "Checked {} known folder(s): {}.",
        roots.len(),
        parts.join(", ")
    ))
}

/// True when reading `path` cannot leave the allowlisted roots, even if the
/// final component does not exist yet. Symlinks are resolved when they exist.
fn may_read(path: &Path, roots: &[PathBuf]) -> bool {
    if let Ok(canonical) = path.canonicalize() {
        return inside_roots(&canonical, roots);
    }
    path.parent()
        .and_then(|parent| parent.canonicalize().ok())
        .map(|parent| inside_roots(&parent, roots))
        .unwrap_or(false)
}

fn prefix_users(prefix: &Path, os_user: &str) -> Vec<String> {
    let drive_c = prefix_drive_c(prefix);
    let mut names: Vec<String> = Vec::new();
    if let Ok(entries) = fs::read_dir(drive_c.join("users")) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let lower = name.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "public" | "default" | "default user" | "all users"
            ) {
                continue;
            }
            names.push(name);
        }
    }
    names.sort();
    let os_user = os_user.to_ascii_lowercase();
    names.sort_by_key(|name| {
        let lower = name.to_ascii_lowercase();
        if lower == "steamuser" {
            0
        } else if lower == os_user {
            1
        } else {
            2
        }
    });
    if names.is_empty() && drive_c.is_dir() {
        // The prefix exists but the Windows user folder has not been created
        // yet: keep the conventional name so a later launch can fill it in.
        names.push("steamuser".to_string());
    }
    names
}

// ---------------------------------------------------------------------------
// Manifest parsing
// ---------------------------------------------------------------------------

/// Parsed Ludusavi manifest (the parts save detection needs).
#[derive(Debug, Clone, Default)]
pub struct Manifest {
    pub games: Vec<ManifestGame>,
}

#[derive(Debug, Clone, Default)]
pub struct ManifestGame {
    pub name: String,
    pub steam_id: Option<u32>,
    pub files: Vec<ManifestFile>,
    /// Number of registry keys listed; they cannot be archived yet.
    pub registry_keys: usize,
}

#[derive(Debug, Clone)]
pub struct ManifestFile {
    pub template: String,
    pub when: Vec<WhenCondition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhenCondition {
    /// `os: windows|linux|macos`.
    Os(Vec<String>),
    /// `store: steam|gog|epic|…`.
    Store(Vec<String>),
    /// Unknown filters are treated as "does not apply", never guessed.
    Unknown,
}

enum ManifestMatch<'a> {
    Found(&'a ManifestGame),
    NotFound,
    Ambiguous,
}

impl Manifest {
    /// Match by Steam app id first, then by exact normalised title, then by the
    /// edition-stripped title. Ambiguity is reported, never guessed.
    fn find(&self, game: &GameContext) -> ManifestMatch<'_> {
        if let Some(app_id) = game.steam_app_id {
            let mut matching = self
                .games
                .iter()
                .filter(|candidate| candidate.steam_id == Some(app_id));
            if let Some(first) = matching.next() {
                return if matching.next().is_some() {
                    ManifestMatch::Ambiguous
                } else {
                    ManifestMatch::Found(first)
                };
            }
        }

        let normalized = steam::normalize_title(&game.title);
        if let Some(found) = find_by_title(&self.games, &normalized) {
            return found;
        }
        let stripped = steam::strip_edition(&game.title);
        if !stripped.is_empty() {
            if let Some(found) = find_by_stripped_title(&self.games, &stripped) {
                return found;
            }
        }
        ManifestMatch::NotFound
    }
}

fn find_by_title<'a>(games: &'a [ManifestGame], key: &str) -> Option<ManifestMatch<'a>> {
    let matching: Vec<&ManifestGame> = games
        .iter()
        .filter(|candidate| steam::normalize_title(&candidate.name) == key)
        .collect();
    match matching.len() {
        0 => None,
        1 => Some(ManifestMatch::Found(matching[0])),
        _ => Some(ManifestMatch::Ambiguous),
    }
}

fn find_by_stripped_title<'a>(games: &'a [ManifestGame], key: &str) -> Option<ManifestMatch<'a>> {
    let matching: Vec<&ManifestGame> = games
        .iter()
        .filter(|candidate| steam::strip_edition(&candidate.name) == key)
        .collect();
    match matching.len() {
        0 => None,
        1 => Some(ManifestMatch::Found(matching[0])),
        _ => Some(ManifestMatch::Ambiguous),
    }
}

/// Limits applied to one manifest parse. `Default` is the shipped policy; tests
/// shrink single fields so a cap can be exercised without a huge fixture.
#[derive(Debug, Clone, Copy)]
struct ManifestLimits {
    max_games: usize,
    max_entry_bytes: usize,
    max_nodes: usize,
    max_aliases: usize,
    chunk_games: usize,
}

impl Default for ManifestLimits {
    fn default() -> Self {
        Self {
            max_games: MAX_MANIFEST_GAMES,
            max_entry_bytes: MAX_MANIFEST_ENTRY_BYTES,
            max_nodes: MAX_MANIFEST_NODES,
            max_aliases: MAX_MANIFEST_ALIASES,
            chunk_games: MANIFEST_CHUNK_GAMES,
        }
    }
}

/// Parse a Ludusavi manifest. Both the normal `Title: {files: …}` shape and a
/// wrapping `games:` mapping are accepted. Nothing is executed.
///
/// The file is parsed in chunks of top-level entries so a 250k-game manifest
/// stays bounded in memory: the official 2026-11-16 file (53,549 games,
/// 17.8 MB) is about 14 chunks (#236).
pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    parse_manifest_with(text, ManifestLimits::default())
}

fn parse_manifest_with(text: &str, limits: ManifestLimits) -> Result<Manifest, String> {
    let bytes = text.len() as u64;
    if bytes > MAX_MANIFEST_BYTES {
        return Err(format!(
            "The Ludusavi manifest is {} ({} bytes), larger than the {} Ludex parses safely. It was not parsed.",
            crate::util::human_bytes(bytes),
            bytes,
            crate::util::human_bytes(MAX_MANIFEST_BYTES)
        ));
    }
    let scan = scan_manifest(text);
    if scan.aliases > limits.max_aliases {
        return Err(format!(
            "The Ludusavi manifest uses {} YAML anchor(s) or alias(es); Ludex accepts manifests without them because an alias bomb can exhaust memory. It was not parsed.",
            scan.aliases
        ));
    }

    let ranges = manifest_chunks(text, &scan.entries, &limits)?;
    let mut games = Vec::new();
    let mut nodes = 0usize;
    for (start, end) in ranges {
        let root: YamlValue = serde_yaml::from_str(&text[start..end])
            .map_err(|error| format!("Invalid Ludusavi manifest: {error}"))?;
        nodes += count_yaml_nodes(&root);
        if nodes > limits.max_nodes {
            return Err(format!(
                "The Ludusavi manifest has more than {} YAML nodes (Ludex accepts {}), so it is too dense to parse safely. It was not parsed.",
                nodes, limits.max_nodes
            ));
        }
        let mapping = root
            .as_mapping()
            .ok_or_else(|| "Invalid Ludusavi manifest: the root is not a mapping.".to_string())?;
        collect_manifest_games(mapping, &mut games, limits.max_games)?;
    }
    Ok(Manifest { games })
}

/// Byte ranges of top-level entries, grouped so one `serde_yaml` document holds
/// at most `chunk_games` entries and `MAX_MANIFEST_CHUNK_BYTES` of text.
fn manifest_chunks(
    text: &str,
    entries: &[usize],
    limits: &ManifestLimits,
) -> Result<Vec<(usize, usize)>, String> {
    if entries.is_empty() {
        // Empty or indentation-only text: let the parser report it.
        return Ok(vec![(0, text.len())]);
    }
    let mut starts = entries.to_vec();
    if starts[0] > 0 {
        starts[0] = 0; // keep a leading comment or `%YAML` directive
    }

    // A wrapping `games:` mapping hides every game one level down: its children
    // are indented, so the whole file becomes one chunk. That compatibility
    // shape stays bounded by `MAX_MANIFEST_BYTES` and `MAX_MANIFEST_NODES`.
    let first_line_end = text[starts[0]..]
        .find('\n')
        .map(|index| starts[0] + index + 1)
        .unwrap_or(text.len());
    let wrapped = text[starts[0]..first_line_end]
        .trim_end()
        .trim_end_matches(':')
        .trim_end()
        == "games";
    if !wrapped && starts.len() > limits.max_games {
        return Err(format!(
            "The Ludusavi manifest lists too many games ({} top-level entries, limit {}). It was not parsed.",
            starts.len(),
            limits.max_games
        ));
    }

    for (index, start) in starts.iter().enumerate() {
        let entry_end = starts.get(index + 1).copied().unwrap_or(text.len());
        let entry_bytes = entry_end.saturating_sub(*start);
        if entry_bytes > limits.max_entry_bytes {
            return Err(format!(
                "The Ludusavi manifest has a single entry of {} bytes, larger than the {} Ludex accepts per game. It was not parsed.",
                entry_bytes, limits.max_entry_bytes
            ));
        }
    }

    let mut ranges = Vec::new();
    let mut chunk_start = starts[0];
    let mut chunk_entries = 0usize;
    for start in &starts {
        if chunk_entries == limits.chunk_games
            || start.saturating_sub(chunk_start) >= MAX_MANIFEST_CHUNK_BYTES
        {
            ranges.push((chunk_start, *start));
            chunk_start = *start;
            chunk_entries = 0;
        }
        chunk_entries += 1;
    }
    ranges.push((chunk_start, text.len()));
    Ok(ranges)
}

fn is_top_level_line(line: &str) -> bool {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() || trimmed.starts_with([' ', '\t', '#']) {
        return false;
    }
    !matches!(trimmed, "---" | "...")
}

/// Append the games of one chunk. A `games:` wrapper is still accepted here for
/// the compatibility shape.
fn collect_manifest_games(
    mapping: &serde_yaml::Mapping,
    games: &mut Vec<ManifestGame>,
    max_games: usize,
) -> Result<(), String> {
    let mut push_game = |name: &str, value: &YamlValue| -> Result<(), String> {
        if games.len() >= max_games {
            return Err(format!(
                "The Ludusavi manifest lists too many games (limit {}). It was not parsed.",
                max_games
            ));
        }
        if let Some(game) = parse_manifest_game(name, value) {
            games.push(game);
        }
        Ok(())
    };

    if let Some(wrapped) = mapping_get(mapping, "games").and_then(YamlValue::as_mapping) {
        for (key, value) in wrapped {
            if let Some(name) = key.as_str() {
                push_game(name, value)?;
            }
        }
    } else {
        for (key, value) in mapping {
            let Some(name) = key.as_str() else {
                continue;
            };
            // Metadata keys (`$schema: "…"`, `version: 3`) are skipped, but a
            // game may legitimately start with `$`: the official manifest ships
            // `"$1 Ride"`. Only a reserved *name with a non-mapping value* is
            // metadata; a mapping under that name is a game entry.
            let reserved = name.starts_with('$') || name.eq_ignore_ascii_case("version");
            if reserved && value.as_mapping().is_none() {
                continue;
            }
            push_game(name, value)?;
        }
    }
    Ok(())
}

/// Count YAML nodes iteratively: pathological input must not recurse in our own
/// code either.
fn count_yaml_nodes(root: &YamlValue) -> usize {
    let mut nodes = 0usize;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        nodes += 1;
        match node {
            YamlValue::Sequence(items) => stack.extend(items.iter()),
            YamlValue::Mapping(mapping) => {
                for (key, value) in mapping {
                    stack.push(key);
                    stack.push(value);
                }
            }
            _ => {}
        }
    }
    nodes
}

/// One structural pass over the manifest text.
struct ManifestScan {
    /// `&anchor` / `*alias` tokens outside quoted scalars, comments and block
    /// scalars, where those characters are literal text.
    aliases: usize,
    /// Byte offsets of lines that start a top-level entry. A line inside a
    /// multi-line flow collection is not a boundary, so a flow-style manifest
    /// is never split mid-value.
    entries: Vec<usize>,
}

/// Scan the text once for aliases and entry boundaries.
///
/// `&`/`*` count only where YAML expects a node, so `Tom & Jerry` and
/// `<home>/saves/*/slot` are literal. A top-level entry is a non-blank,
/// non-comment line in the first column at flow depth zero: YAML indents a
/// value, so a chunk boundary there never splits a game.
fn scan_manifest(text: &str) -> ManifestScan {
    let mut scan = ManifestScan {
        aliases: 0,
        entries: Vec::new(),
    };
    let mut quote: Option<u8> = None;
    let mut block_indent: Option<usize> = None;
    let mut flow_depth = 0usize;
    let mut offset = 0usize;

    for line in text.split_inclusive('\n') {
        let bytes = line.as_bytes();
        let indent = bytes.iter().take_while(|byte| **byte == b' ').count();
        if let Some(base) = block_indent {
            if bytes.iter().all(u8::is_ascii_whitespace) || indent > base {
                offset += line.len();
                continue; // blank line or literal block-scalar content
            }
            block_indent = None;
        }
        if quote.is_none() && flow_depth == 0 && is_top_level_line(line) {
            scan.entries.push(offset);
        }
        if quote.is_none() && line.trim_start().starts_with('#') {
            offset += line.len();
            continue;
        }

        // `at_node_start` is true where YAML expects a node: line start, after
        // `: `, `- `, `? `, `[`, `{` or `,`.
        let mut at_node_start = true;
        let mut previous: Option<u8> = None;
        let mut index = 0usize;
        while index < bytes.len() {
            let byte = bytes[index];
            if let Some(open) = quote {
                if byte == open {
                    if open == b'\'' && bytes.get(index + 1) == Some(&b'\'') {
                        index += 1; // `''` is an escaped quote
                    } else if open != b'"' || !is_escaped(bytes, index) {
                        quote = None;
                    }
                }
                previous = Some(byte);
                index += 1;
                continue;
            }
            match byte {
                b'"' | b'\'' if at_node_start => {
                    quote = Some(byte);
                    at_node_start = false;
                }
                b'#' if previous.is_none_or(|character| character.is_ascii_whitespace()) => break,
                b'&' | b'*' if at_node_start => {
                    scan.aliases += 1;
                    index += 1;
                    // Skip the name so its characters are not read as structure.
                    while index < bytes.len()
                        && !bytes[index].is_ascii_whitespace()
                        && !matches!(bytes[index], b'[' | b']' | b'{' | b'}' | b',')
                    {
                        index += 1;
                    }
                    previous = Some(b'x');
                    at_node_start = false;
                    continue;
                }
                // Only a flow indicator where a node may start opens a
                // collection: a brace in a plain scalar (`<base>/{slot}`) is
                // literal text, not structure.
                b'[' | b'{' if at_node_start => {
                    flow_depth += 1;
                    at_node_start = true;
                }
                b']' | b'}' if flow_depth > 0 => {
                    flow_depth -= 1;
                    at_node_start = false;
                }
                b',' if flow_depth > 0 => at_node_start = true,
                b':' if bytes
                    .get(index + 1)
                    .is_some_and(|next| next.is_ascii_whitespace()) =>
                {
                    at_node_start = true;
                }
                b'-' | b'?'
                    if previous.is_none_or(|character| character.is_ascii_whitespace())
                        && bytes
                            .get(index + 1)
                            .is_some_and(|next| next.is_ascii_whitespace()) =>
                {
                    at_node_start = true;
                }
                other if other.is_ascii_whitespace() => {}
                _ => at_node_start = false,
            }
            previous = Some(byte);
            index += 1;
        }

        if quote.is_none() && is_block_scalar_line(line) {
            block_indent = Some(indent);
        }
        offset += line.len();
    }
    scan
}

/// A double-quoted scalar closes on a quote that is not escaped, i.e. preceded
/// by an odd number of backslashes. `\\"` ends the scalar, `\\\"` does not.
fn is_escaped(bytes: &[u8], quote: usize) -> bool {
    let mut backslashes = 0usize;
    let mut cursor = quote;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        backslashes += 1;
        cursor -= 1;
    }
    backslashes % 2 == 1
}

/// True for a line whose value is a literal or folded block scalar (`|`, `>-`,
/// `|2+`, …). Its content is text, not YAML structure.
fn is_block_scalar_line(line: &str) -> bool {
    let without_comment = line.split(" #").next().unwrap_or(line).trim_end();
    let token = without_comment
        .rsplit(|character: char| character.is_whitespace())
        .next()
        .unwrap_or_default();
    let mut chars = token.chars();
    match chars.next() {
        Some('|') | Some('>') => {}
        _ => return false,
    }
    match chars.collect::<Vec<_>>().as_slice() {
        [] => true,
        [indicator] => matches!(indicator, '+' | '-' | '1'..='9'),
        [digit, sign] => matches!(digit, '1'..='9') && matches!(sign, '+' | '-'),
        _ => false,
    }
}

fn parse_manifest_game(name: &str, value: &YamlValue) -> Option<ManifestGame> {
    let mapping = value.as_mapping()?;
    let mut game = ManifestGame {
        name: name.to_string(),
        ..ManifestGame::default()
    };

    if let Some(files) = mapping_get(mapping, "files") {
        parse_manifest_files(files, &mut game.files);
    }
    if let Some(steam) = mapping_get(mapping, "steam").and_then(YamlValue::as_mapping) {
        game.steam_id = mapping_get(steam, "id").and_then(yaml_u32);
    }
    if let Some(registry) = mapping_get(mapping, "registry") {
        game.registry_keys = registry
            .as_mapping()
            .map(|map| map.len())
            .or_else(|| registry.as_sequence().map(|items| items.len()))
            .unwrap_or(0);
    }
    Some(game)
}

/// Files come in two shapes (#236):
///
/// ```yaml
/// files:                      # official Ludusavi manifest
///   "<winAppData>/Game/Saves":
///     tags: [save]
///     when:
///       - os: windows
/// ```
///
/// and the older `files: [{path: …, when: …}]` / `files: [<template>]` list,
/// which stays supported. `tags` are ignored: a listed path is a path.
fn parse_manifest_files(value: &YamlValue, files: &mut Vec<ManifestFile>) {
    match value {
        YamlValue::Sequence(items) => {
            for item in items {
                if let Some(file) = parse_manifest_file(item) {
                    files.push(file);
                }
            }
        }
        YamlValue::Mapping(entries) => {
            for (key, spec) in entries {
                let Some(template) = key.as_str().map(str::trim).filter(|key| !key.is_empty())
                else {
                    continue;
                };
                let when = spec
                    .as_mapping()
                    .and_then(|mapping| mapping_get(mapping, "when"))
                    .map(parse_when)
                    .unwrap_or_default();
                files.push(ManifestFile {
                    template: template.to_string(),
                    when,
                });
            }
        }
        _ => {}
    }
}

fn parse_manifest_file(value: &YamlValue) -> Option<ManifestFile> {
    if let Some(path) = yaml_str(value) {
        return Some(ManifestFile {
            template: path.to_string(),
            when: Vec::new(),
        });
    }
    let mapping = value.as_mapping()?;
    let template = mapping_get(mapping, "path").and_then(yaml_str)?;
    Some(ManifestFile {
        template: template.to_string(),
        when: mapping_get(mapping, "when")
            .map(parse_when)
            .unwrap_or_default(),
    })
}

fn parse_when(value: &YamlValue) -> Vec<WhenCondition> {
    let mut conditions = Vec::new();
    match value {
        YamlValue::Sequence(items) => {
            for item in items {
                parse_when_item(item, &mut conditions);
            }
        }
        other => parse_when_item(other, &mut conditions),
    }
    conditions
}

fn parse_when_item(value: &YamlValue, conditions: &mut Vec<WhenCondition>) {
    match value {
        YamlValue::String(store) => conditions.push(WhenCondition::Store(vec![store.clone()])),
        YamlValue::Mapping(mapping) => {
            for (key, value) in mapping {
                let Some(key) = key.as_str() else {
                    conditions.push(WhenCondition::Unknown);
                    continue;
                };
                let values = yaml_string_list(value);
                if values.is_empty() {
                    conditions.push(WhenCondition::Unknown);
                } else {
                    match key.to_ascii_lowercase().as_str() {
                        "os" => conditions.push(WhenCondition::Os(values)),
                        "store" => conditions.push(WhenCondition::Store(values)),
                        _ => conditions.push(WhenCondition::Unknown),
                    }
                }
            }
        }
        _ => conditions.push(WhenCondition::Unknown),
    }
}

fn mapping_get<'a>(mapping: &'a serde_yaml::Mapping, key: &str) -> Option<&'a YamlValue> {
    mapping
        .iter()
        .find(|(candidate, _)| candidate.as_str() == Some(key))
        .map(|(_, value)| value)
}

fn yaml_str(value: &YamlValue) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn yaml_u32(value: &YamlValue) -> Option<u32> {
    match value {
        YamlValue::Number(number) => number.as_u64().and_then(|value| u32::try_from(value).ok()),
        YamlValue::String(value) => value.trim().parse().ok(),
        _ => None,
    }
}

fn yaml_string_list(value: &YamlValue) -> Vec<String> {
    match value {
        YamlValue::String(value) => vec![value.clone()],
        YamlValue::Sequence(items) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// SHA-256 of the manifest bytes, lowercase hex. Doubles as the cache key and
/// as the `manifest_version` reported by a detection.
fn manifest_digest(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn manifest_version_from_digest(digest: &str) -> String {
    format!("sha256:{}", &digest[..12.min(digest.len())])
}

/// A manifest ready for detection.
#[derive(Debug)]
struct LoadedManifest {
    manifest: Arc<Manifest>,
    /// `sha256:<12 hex>` of the parsed bytes.
    version: String,
}

/// Why a manifest could not be loaded. The caller turns this into the panel
/// message and the warning list.
#[derive(Debug)]
enum ManifestLoadError {
    Read(std::io::Error),
    /// Bytes on disk and the cap they exceed.
    TooLarge(u64),
    Parse(String),
}

/// Parsed manifests, kept between detection runs. Detection runs once per game
/// and again after each launch; the official manifest is 18 MB and takes
/// seconds to parse, so re-parsing it per game is not an option.
///
/// Entries are keyed by path, so the explicit path, the managed copy and a
/// user-installed copy never evict each other, and one changed file only
/// replaces its own entry. At most [`MAX_CACHED_MANIFESTS`] paths are kept: a
/// process that reads more distinct manifest files than that is pathological,
/// and the cap stops it from holding parsed YAML forever.
static MANIFEST_CACHE: OnceLock<Mutex<ManifestCache>> = OnceLock::new();

/// Distinct manifest paths kept in memory. Real use is one to three.
const MAX_CACHED_MANIFESTS: usize = 8;

struct CachedManifest {
    digest: String,
    manifest: Arc<Manifest>,
}

#[derive(Default)]
struct ManifestCache {
    entries: std::collections::HashMap<PathBuf, CachedManifest>,
}

impl ManifestCache {
    fn get(&self, path: &Path, digest: &str) -> Option<Arc<Manifest>> {
        self.entries
            .get(path)
            .filter(|cached| cached.digest == digest)
            .map(|cached| cached.manifest.clone())
    }

    fn put(&mut self, path: &Path, digest: &str, manifest: Arc<Manifest>) {
        if self.entries.len() >= MAX_CACHED_MANIFESTS && !self.entries.contains_key(path) {
            // Bounded memory: drop one arbitrary other path. A later detection
            // that still wants it pays one parse, never a wrong result.
            if let Some(evicted) = self.entries.keys().next().cloned() {
                self.entries.remove(&evicted);
            }
        }
        self.entries.insert(
            path.to_path_buf(),
            CachedManifest {
                digest: digest.to_string(),
                manifest,
            },
        );
    }
}

fn manifest_cache() -> &'static Mutex<ManifestCache> {
    MANIFEST_CACHE.get_or_init(|| Mutex::new(ManifestCache::default()))
}

fn lock_manifest_cache() -> std::sync::MutexGuard<'static, ManifestCache> {
    match manifest_cache().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Remember an already-parsed manifest. The update path validates a download
/// before writing it, so the next detection can skip the parse entirely.
pub(crate) fn remember_manifest(path: &Path, digest: &str, manifest: Manifest) {
    lock_manifest_cache().put(path, digest, Arc::new(manifest));
}

/// Read and parse the manifest at `path`, reusing the previous parse when the
/// bytes are unchanged. The file is stat'ed before it is read so a 1 GB file is
/// rejected without being loaded.
fn load_manifest(path: &Path) -> Result<LoadedManifest, ManifestLoadError> {
    load_manifest_with(path, manifest_cache())
}

/// [`load_manifest`] over an explicit cache, so a test can exercise the
/// content key without the process-wide cache of the other tests.
fn load_manifest_with(
    path: &Path,
    cache: &Mutex<ManifestCache>,
) -> Result<LoadedManifest, ManifestLoadError> {
    let metadata = fs::metadata(path).map_err(ManifestLoadError::Read)?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestLoadError::TooLarge(metadata.len()));
    }
    let text = fs::read_to_string(path).map_err(ManifestLoadError::Read)?;
    if text.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(ManifestLoadError::TooLarge(text.len() as u64));
    }

    let digest = manifest_digest(&text);
    let version = manifest_version_from_digest(&digest);
    let cached = match cache.lock() {
        Ok(guard) => guard.get(path, &digest),
        Err(poisoned) => poisoned.into_inner().get(path, &digest),
    };
    if let Some(manifest) = cached {
        return Ok(LoadedManifest { manifest, version });
    }
    let manifest = parse_manifest(&text).map_err(ManifestLoadError::Parse)?;
    tracing::info!(
        games = manifest.games.len(),
        path = %path.display(),
        "parsed the Ludusavi save-path manifest"
    );
    let manifest = Arc::new(manifest);
    match cache.lock() {
        Ok(mut guard) => guard.put(path, &digest, manifest.clone()),
        Err(poisoned) => poisoned.into_inner().put(path, &digest, manifest.clone()),
    }
    Ok(LoadedManifest { manifest, version })
}

// ---------------------------------------------------------------------------
// Template expansion
// ---------------------------------------------------------------------------

const TEMPLATES: [&str; 14] = [
    "<winLocalAppDataLow>",
    "<winLocalAppData>",
    "<winSavedGames>",
    "<winAppData>",
    "<winDocuments>",
    "<winPublic>",
    "<winProgramData>",
    "<winDir>",
    "<storeUserId>",
    "<osUserName>",
    "<xdgConfig>",
    "<xdgData>",
    "<base>",
    "<home>",
];

fn find_placeholder(value: &str) -> Option<(usize, usize, &'static str)> {
    TEMPLATES
        .iter()
        .filter_map(|name| {
            value
                .find(name)
                .map(|start| (start, start + name.len(), *name))
        })
        .min_by_key(|(start, _, _)| *start)
}

fn render_templates(template: &str, env: &DetectEnv, game: &GameContext) -> Vec<String> {
    let mut results = vec![template.to_string()];

    for _ in 0..12 {
        let mut changed = false;
        let mut next = Vec::new();
        for value in &results {
            if let Some((start, end, name)) = find_placeholder(value) {
                changed = true;
                for variant in template_variants(name, env, game) {
                    let mut rendered = value.clone();
                    rendered.replace_range(start..end, &variant);
                    next.push(rendered);
                }
            } else {
                next.push(value.clone());
            }
        }
        results = next;
        if !changed || results.len() > MAX_TEMPLATE_RESULTS {
            break;
        }
    }

    results.retain(|value| !value.contains('<'));
    results.truncate(MAX_TEMPLATE_RESULTS);
    results
}

fn template_variants(name: &str, env: &DetectEnv, game: &GameContext) -> Vec<String> {
    match name {
        "<base>" => vec![game.install_dir.to_string_lossy().to_string()],
        "<home>" => vec![env.home.to_string_lossy().to_string()],
        "<osUserName>" => vec![env.os_user_name.clone()],
        "<storeUserId>" => env.steam_user_ids.clone(),
        "<xdgData>" => vec![env.xdg_data.to_string_lossy().to_string()],
        "<xdgConfig>" => vec![env.xdg_config.to_string_lossy().to_string()],
        "<winDir>" => {
            if let Some(prefix) = &env.prefix {
                vec![prefix_drive_c(prefix).to_string_lossy().to_string()]
            } else {
                env.windows
                    .win_dir
                    .as_ref()
                    .map(|path| vec![path.to_string_lossy().to_string()])
                    .unwrap_or_default()
            }
        }
        "<winAppData>" => {
            user_windows_variants(env, "AppData/Roaming", env.windows.app_data.as_ref())
        }
        "<winLocalAppData>" => {
            user_windows_variants(env, "AppData/Local", env.windows.local_app_data.as_ref())
        }
        "<winDocuments>" => user_windows_variants(env, "Documents", env.windows.documents.as_ref()),
        // Ludusavi manifests address Saved Games and LocalLow as well; both are
        // known folders and must never be built from `%USERPROFILE%` (#158).
        "<winSavedGames>" => {
            user_windows_variants(env, "Saved Games", env.windows.saved_games.as_ref())
        }
        "<winLocalAppDataLow>" => user_windows_variants(
            env,
            "AppData/LocalLow",
            env.windows.local_app_data_low.as_ref(),
        ),
        "<winPublic>" => {
            if let Some(prefix) = &env.prefix {
                vec![prefix_drive_c(prefix)
                    .join("users/Public")
                    .to_string_lossy()
                    .to_string()]
            } else {
                env.windows
                    .public
                    .as_ref()
                    .map(|path| vec![path.to_string_lossy().to_string()])
                    .unwrap_or_default()
            }
        }
        "<winProgramData>" => {
            if let Some(prefix) = &env.prefix {
                vec![prefix_drive_c(prefix)
                    .join("ProgramData")
                    .to_string_lossy()
                    .to_string()]
            } else {
                env.windows
                    .program_data
                    .as_ref()
                    .map(|path| vec![path.to_string_lossy().to_string()])
                    .unwrap_or_default()
            }
        }
        _ => Vec::new(),
    }
}

fn user_windows_variants(env: &DetectEnv, suffix: &str, native: Option<&PathBuf>) -> Vec<String> {
    if let Some(prefix) = &env.prefix {
        let drive_c = prefix_drive_c(prefix);
        let variants: Vec<String> = prefix_users(prefix, &env.os_user_name)
            .into_iter()
            .map(|user| {
                drive_c
                    .join("users")
                    .join(user)
                    .join(suffix)
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        if !variants.is_empty() {
            return variants;
        }
    }
    native
        .map(|path| vec![path.to_string_lossy().to_string()])
        .unwrap_or_default()
}

/// Turn one rendered template into an absolute host path.
fn rendered_path(raw: &str, env: &DetectEnv) -> Result<PathBuf, String> {
    let text = raw.replace('\\', "/");
    let text = text.trim();
    if text.is_empty() {
        return Err("the path is empty".to_string());
    }
    if text == "~" {
        return Ok(env.home.clone());
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return Ok(env.home.join(rest));
    }

    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        let drive = (bytes[0] as char).to_ascii_uppercase();
        let rest = text[2..].trim_start_matches('/');
        if drive == 'C' {
            if let Some(prefix) = &env.prefix {
                return Ok(prefix_drive_c(prefix).join(rest));
            }
            if env.platform == Platform::Windows {
                return Ok(PathBuf::from(raw));
            }
            return Err("a Windows path needs a Wine/Proton prefix".to_string());
        }
        return Err(format!("drive {drive}: is not supported"));
    }

    let path = PathBuf::from(text);
    if env.platform != Platform::Windows && !path.is_absolute() {
        return Err("the path is not absolute".to_string());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("the path escapes its root with '..'".to_string());
    }
    Ok(path)
}

/// Expand a `*` inside path segments. `*` never crosses a separator. Directories
/// are only listed when they stay under an allowlisted root.
fn expand_glob(path: &Path, roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut current = vec![PathBuf::new()];

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                for base in &mut current {
                    *base = PathBuf::from(prefix.as_os_str());
                }
            }
            Component::RootDir => {
                for base in &mut current {
                    base.push(std::path::MAIN_SEPARATOR.to_string());
                }
            }
            Component::CurDir => {}
            Component::ParentDir => return Vec::new(),
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                if name.contains('*') {
                    let mut next = Vec::new();
                    for base in &current {
                        if !may_read(base, roots) {
                            continue;
                        }
                        let Ok(entries) = fs::read_dir(base) else {
                            continue;
                        };
                        let mut matches: Vec<PathBuf> = entries
                            .flatten()
                            .filter(|entry| {
                                let file_name = entry.file_name();
                                segment_matches(&name, &file_name.to_string_lossy())
                            })
                            .map(|entry| entry.path())
                            .take(MAX_GLOB_MATCHES)
                            .collect();
                        matches.sort();
                        next.extend(matches);
                    }
                    current = next;
                } else {
                    for base in &mut current {
                        base.push(name.as_ref());
                    }
                }
            }
        }
        if current.len() > MAX_GLOB_MATCHES {
            current.truncate(MAX_GLOB_MATCHES);
        }
        if current.is_empty() {
            break;
        }
    }
    current
}

fn segment_matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut pattern_index, mut name_index) = (0, 0);
    let mut star: Option<usize> = None;
    let mut resume = 0;

    while name_index < name.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == '?' || pattern[pattern_index] == name[name_index])
        {
            pattern_index += 1;
            name_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == '*' {
            star = Some(pattern_index);
            pattern_index += 1;
            resume = name_index;
        } else if let Some(star_index) = star {
            pattern_index = star_index + 1;
            resume += 1;
            name_index = resume;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == '*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

// ---------------------------------------------------------------------------
// Manifest walk
// ---------------------------------------------------------------------------

struct ManifestWalk {
    saves: Vec<DetectedSave>,
    skipped_filters: usize,
    missing_stores: BTreeSet<String>,
}

fn walk_manifest(
    found: &ManifestGame,
    env: &DetectEnv,
    game: &GameContext,
    roots: &[PathBuf],
    warnings: &mut Vec<String>,
) -> ManifestWalk {
    let mut walk = ManifestWalk {
        saves: Vec::new(),
        skipped_filters: 0,
        missing_stores: BTreeSet::new(),
    };

    for file in &found.files {
        if !conditions_match(&file.when, env, game.store.as_deref()) {
            walk.skipped_filters += 1;
            if game.store.is_none() {
                for condition in &file.when {
                    if let WhenCondition::Store(stores) = condition {
                        walk.missing_stores.extend(stores.iter().cloned());
                    }
                }
            }
            continue;
        }

        let rendered = render_templates(&file.template, env, game);
        if rendered.is_empty() {
            warnings.push(format!(
                "Could not resolve the save path template {} on this machine.",
                file.template
            ));
            continue;
        }

        for raw in rendered {
            let path = match rendered_path(&raw, env) {
                Ok(path) => path,
                Err(error) => {
                    warnings.push(format!("Skipped save path {raw}: {error}"));
                    continue;
                }
            };
            if !path.to_string_lossy().contains('*') && !may_read(&path, roots) {
                warnings.push(format!(
                    "Refused save path outside the allowlisted roots: {}",
                    path.display()
                ));
                continue;
            }

            for candidate in expand_glob(&path, roots) {
                if fs::symlink_metadata(&candidate).is_err() {
                    // Not created yet: the user can re-detect after launching.
                    continue;
                }
                let Ok(canonical) = candidate.canonicalize() else {
                    continue;
                };
                if !inside_roots(&canonical, roots) {
                    warnings.push(format!(
                        "Refused save path outside the allowlisted roots: {}",
                        canonical.display()
                    ));
                    continue;
                }
                walk.saves.push(save_from_canonical(canonical));
            }
        }
    }
    walk
}

fn conditions_match(conditions: &[WhenCondition], env: &DetectEnv, store: Option<&str>) -> bool {
    conditions.iter().all(|condition| match condition {
        WhenCondition::Os(values) => values.iter().any(|value| os_matches(value, env)),
        WhenCondition::Store(values) => store
            .map(|store| values.iter().any(|value| value.eq_ignore_ascii_case(store)))
            .unwrap_or(false),
        WhenCondition::Unknown => false,
    })
}

fn os_matches(value: &str, env: &DetectEnv) -> bool {
    match value.to_ascii_lowercase().as_str() {
        // A Windows game running under Wine/Proton also matches `windows`.
        "windows" => env.platform == Platform::Windows || env.prefix.is_some(),
        "linux" => env.platform == Platform::Linux,
        "macos" | "mac" | "osx" => env.platform == Platform::Macos,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Measurement
// ---------------------------------------------------------------------------

struct Measured {
    kind: String,
    size: i64,
    last_modified: Option<String>,
    notes: Vec<String>,
}

fn save_from_canonical(path: PathBuf) -> DetectedSave {
    let measured = measure(&path);
    DetectedSave {
        path: path.to_string_lossy().to_string(),
        kind: measured.kind,
        source: SOURCE_MANIFEST.to_string(),
        size_bytes: measured.size,
        last_modified: measured.last_modified,
        notes: join_notes(measured.notes),
    }
}

fn manual_save(
    record: &SavePathRecord,
    roots: &[PathBuf],
    warnings: &mut Vec<String>,
) -> Option<DetectedSave> {
    let path = PathBuf::from(&record.path);
    if !may_read(&path, roots) {
        warnings.push(format!(
            "Refused manual save path outside the allowlisted roots: {}",
            path.display()
        ));
        return None;
    }
    match path.canonicalize() {
        Ok(canonical) => {
            if !inside_roots(&canonical, roots) {
                warnings.push(format!(
                    "Refused manual save path outside the allowlisted roots: {}",
                    canonical.display()
                ));
                return None;
            }
            let measured = measure(&canonical);
            Some(DetectedSave {
                path: canonical.to_string_lossy().to_string(),
                kind: measured.kind,
                source: record.source.clone(),
                size_bytes: measured.size,
                last_modified: measured.last_modified,
                notes: join_notes(measured.notes),
            })
        }
        Err(_) => Some(DetectedSave {
            path: record.path.clone(),
            kind: record.kind.clone(),
            source: record.source.clone(),
            size_bytes: 0,
            last_modified: None,
            notes: Some(MISSING_NOTE.to_string()),
        }),
    }
}

fn measure(path: &Path) -> Measured {
    let Ok(metadata) = fs::metadata(path) else {
        return Measured {
            kind: "file".to_string(),
            size: 0,
            last_modified: None,
            notes: vec![MISSING_NOTE.to_string()],
        };
    };

    if metadata.is_file() {
        return Measured {
            kind: "file".to_string(),
            size: i64::try_from(metadata.len()).unwrap_or(i64::MAX),
            last_modified: metadata.modified().ok().map(rfc3339),
            notes: Vec::new(),
        };
    }

    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut newest: Option<SystemTime> = None;
    let mut notes = Vec::new();
    if let Ok(modified) = metadata.modified() {
        newest = Some(modified);
    }

    let mut stack: Vec<PathBuf> = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(metadata) = fs::symlink_metadata(&current) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            // Never follow links inside a save folder: they can escape the root.
            continue;
        }
        if metadata.is_dir() {
            if let Ok(modified) = metadata.modified() {
                newest = Some(newest.map_or(modified, |newest| newest.max(modified)));
            }
            if let Ok(entries) = fs::read_dir(&current) {
                for entry in entries.flatten() {
                    stack.push(entry.path());
                }
            }
        } else if metadata.is_file() {
            files += 1;
            bytes = bytes.saturating_add(metadata.len());
            if let Ok(modified) = metadata.modified() {
                newest = Some(newest.map_or(modified, |newest| newest.max(modified)));
            }
            if files >= MAX_SAVE_FILES || bytes >= MAX_SAVE_BYTES {
                notes.push(format!(
                    "Scan capped at {MAX_SAVE_FILES} files / {}",
                    crate::util::human_bytes(MAX_SAVE_BYTES)
                ));
                break;
            }
        }
    }

    Measured {
        kind: "directory".to_string(),
        size: i64::try_from(bytes).unwrap_or(i64::MAX),
        last_modified: newest.map(rfc3339),
        notes,
    }
}

fn rfc3339(time: SystemTime) -> String {
    let datetime: chrono::DateTime<chrono::Utc> = time.into();
    datetime.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn manual_path(raw: &str, env: &DetectEnv) -> PathBuf {
    let text = raw.replace('\\', "/");
    if text == "~" {
        return env.home.clone();
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return env.home.join(rest);
    }
    PathBuf::from(text)
}

// ---------------------------------------------------------------------------
// Ludusavi CLI (`--api` preview)
// ---------------------------------------------------------------------------

struct LudusaviPreview {
    paths: Vec<String>,
    version: Option<String>,
}

struct Captured {
    success: bool,
    status: Option<i32>,
    text: String,
}

fn ludusavi_preview(binary: &Path, title: &str) -> Result<LudusaviPreview, String> {
    if !binary.is_file() {
        return Err(format!("{} is not a file", binary.display()));
    }
    let version = ludusavi_version(binary);

    let mut command = crate::process::command(binary);
    command.arg("--api");
    let request = serde_json::json!({
        "apiVersion": "v1",
        "command": "preview",
        "games": [title],
    })
    .to_string();
    let captured = run_capture(command, Some(&request), MAX_CLI_OUTPUT, CLI_TIMEOUT)?;
    if !captured.success {
        return Err(match captured.status {
            Some(code) => format!("ludusavi exited with code {code}"),
            None => "ludusavi was terminated".to_string(),
        });
    }
    let paths = parse_api_preview(&captured.text)?;
    Ok(LudusaviPreview { paths, version })
}

fn ludusavi_version(binary: &Path) -> Option<String> {
    let mut command = crate::process::command(binary);
    command.arg("--version");
    let captured = run_capture(command, None, 64 * 1024, CLI_VERSION_TIMEOUT).ok()?;
    if !captured.success {
        return None;
    }
    captured
        .text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

fn run_capture(
    mut command: Command,
    payload: Option<&str>,
    limit: u64,
    timeout: Duration,
) -> Result<Captured, String> {
    let label = command.get_program().to_string_lossy().to_string();
    command
        .stdin(if payload.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not run {label}: {error}"))?;

    if let Some(payload) = payload {
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(payload.as_bytes())
                .and_then(|_| stdin.write_all(b"\n"))
                .map_err(|error| format!("Could not send the request to {label}: {error}"))?;
            // Dropping stdin closes the pipe so `ludusavi --api` can finish.
        }
    }

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{label} did not expose stdout"))?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let mut limited = stdout.by_ref().take(limit);
        let _ = limited.read_to_end(&mut buffer);
        let _ = sender.send(buffer);
    });

    match receiver.recv_timeout(timeout) {
        Ok(bytes) => {
            // `take` stops reading at the cap; a child still writing would
            // block forever on a full pipe, so terminate it before waiting.
            if bytes.len() as u64 >= limit {
                let _ = child.kill();
            }
            let status = child
                .wait()
                .map_err(|error| format!("Could not wait for {label}: {error}"))?;
            Ok(Captured {
                success: status.success(),
                status: status.code(),
                text: String::from_utf8_lossy(&bytes).to_string(),
            })
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(format!(
                "{label} did not answer within {} seconds",
                timeout.as_secs()
            ))
        }
    }
}

/// Read the last JSON line Ludusavi printed that contains a preview.
fn parse_api_preview(text: &str) -> Result<Vec<String>, String> {
    for line in text.lines().rev() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<JsonValue>(line) {
            if let Some(paths) = api_paths(&value) {
                return Ok(paths);
            }
        }
    }
    Err("Ludusavi returned no preview that Ludex could understand.".to_string())
}

fn api_paths(value: &JsonValue) -> Option<Vec<String>> {
    let games = value.get("games")?;
    let mut paths = Vec::new();
    match games {
        JsonValue::Object(map) => {
            for entry in map.values() {
                collect_api_entry(entry, &mut paths);
            }
        }
        JsonValue::Array(items) => {
            for entry in items {
                collect_api_entry(entry, &mut paths);
            }
        }
        _ => return None,
    }
    Some(paths)
}

fn collect_api_entry(entry: &JsonValue, paths: &mut Vec<String>) {
    let Some(files) = entry.get("files") else {
        return;
    };
    match files {
        JsonValue::Object(map) => {
            for (path, metadata) in map {
                let ignored = metadata
                    .get("ignored")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false);
                if !ignored && !path.trim().is_empty() {
                    paths.push(path.clone());
                }
            }
        }
        JsonValue::Array(items) => {
            for item in items {
                let ignored = item
                    .get("ignored")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false);
                if ignored {
                    continue;
                }
                if let Some(path) = item.get("path").and_then(JsonValue::as_str) {
                    paths.push(path.to_string());
                }
            }
        }
        _ => {}
    }
}

fn api_saves(paths: &[String], roots: &[PathBuf], warnings: &mut Vec<String>) -> Vec<DetectedSave> {
    let mut saves = Vec::new();
    for raw in paths {
        let path = PathBuf::from(raw);
        if path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            warnings.push(format!("Refused save path containing '..': {raw}"));
            continue;
        }
        if !may_read(&path, roots) {
            warnings.push(format!(
                "Refused save path outside the allowlisted roots: {raw}"
            ));
            continue;
        }
        let Ok(canonical) = path.canonicalize() else {
            continue;
        };
        if !inside_roots(&canonical, roots) {
            warnings.push(format!(
                "Refused save path outside the allowlisted roots: {}",
                canonical.display()
            ));
            continue;
        }
        saves.push(save_from_canonical(canonical));
    }
    saves
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    const HADES_MANIFEST: &str = r#"
"Hades™":
  files:
    - <winDocuments>/Hades
  steam:
    id: 1145360
"#;

    /// Top-level game count of the published manifest on 2026-11-16. The parser
    /// must always accept at least this many entries.
    const OFFICIAL_MANIFEST_GAMES: usize = 53_549;

    struct Fixture {
        root: PathBuf,
        home: PathBuf,
        install: PathBuf,
        prefix: PathBuf,
        env: DetectEnv,
        game: GameContext,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn temp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fixture(label: &str) -> Fixture {
        let root = temp_root(label);
        let home = root.join("home");
        let install = root.join("games/Hades");
        let prefix = root.join("prefix/pfx");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&install).unwrap();
        fs::create_dir_all(prefix.join("drive_c/users/steamuser/AppData/Roaming")).unwrap();
        fs::create_dir_all(prefix.join("drive_c/users/steamuser/Documents")).unwrap();
        fs::create_dir_all(prefix.join("drive_c/users/Public")).unwrap();
        fs::create_dir_all(prefix.join("drive_c/ProgramData")).unwrap();

        let mut known = ResolvedFolders::default();
        known.insert("home", home.clone());
        known.insert("xdg_data", home.join(".local/share"));
        known.insert("xdg_config", home.join(".config"));
        known.insert("xdg_state", home.join(".local/state"));
        fs::create_dir_all(home.join(".local/state")).unwrap();

        let env = DetectEnv {
            home: home.clone(),
            os_user_name: "player".to_string(),
            platform: Platform::Linux,
            xdg_data: home.join(".local/share"),
            xdg_config: home.join(".config"),
            xdg_state: home.join(".local/state"),
            windows: WindowsDirs::default(),
            known,
            registry: crate::saves::registry::registry().clone(),
            steam_roots: Vec::new(),
            prefix: Some(prefix.clone()),
            steam_user_ids: vec!["76561198000000001".to_string()],
            manifest_path: None,
            manifest_source: "none".to_string(),
            ludusavi: None,
        };
        let game = GameContext {
            title: "Hades".to_string(),
            install_dir: install.clone(),
            steam_app_id: Some(1145360),
            store: Some("steam".to_string()),
        };
        Fixture {
            root,
            home,
            install,
            prefix,
            env,
            game,
        }
    }

    fn detect_with_manifest(fixture: &Fixture, manifest: &str) -> Detection {
        detect_with_context(fixture, manifest, &fixture.game)
    }

    fn detect_with_context(fixture: &Fixture, manifest: &str, game: &GameContext) -> Detection {
        let path = fixture.root.join("manifest.yaml");
        fs::write(&path, manifest).unwrap();
        let mut env = fixture.env.clone();
        env.manifest_path = Some(path);
        detect(&env, game, &[])
    }

    #[test]
    fn manifest_matches_steam_app_id_and_resolves_windows_templates() {
        let fixture = fixture("saves-manifest");
        let save_dir = fixture
            .prefix
            .join("drive_c/users/steamuser/Documents/Hades");
        fs::create_dir_all(&save_dir).unwrap();
        fs::write(save_dir.join("slot1.sav"), b"12345").unwrap();

        let detection = detect_with_manifest(&fixture, HADES_MANIFEST);
        assert!(detection.detected, "{detection:?}");
        assert_eq!(detection.saves.len(), 1, "{:?}", detection.saves);
        let detected = &detection.saves[0];
        assert_eq!(
            detected.path,
            save_dir.canonicalize().unwrap().to_string_lossy()
        );
        assert_eq!(detected.kind, "directory");
        assert_eq!(detected.source, SOURCE_MANIFEST);
        assert_eq!(detected.size_bytes, 5);
        assert!(detected.last_modified.is_some());
        assert_eq!(detection.source, "manifest");
        assert!(detection
            .manifest_version
            .as_deref()
            .unwrap_or_default()
            .starts_with("sha256:"));
    }

    /// A compact manifest with `games` entries plus one real one at the end, so
    /// a truncated or mis-chunked parse is caught by the final assertion (#236).
    fn synthetic_manifest(games: usize) -> String {
        let mut text = String::with_capacity(games * 32 + 128);
        for index in 0..games {
            text.push_str(&format!("\"Game {index}\": {{}}\n"));
        }
        text.push_str("\"Hades™\":\n  files:\n    \"<home>/.local/share/Hades\":\n      tags:\n        - save\n  steam:\n    id: 1145360\n");
        text
    }

    #[test]
    fn a_manifest_above_the_old_fifty_thousand_cap_parses_and_matches() {
        // The official manifest had 53,549 games on 2026-11-16, past the old
        // 50,000 cap that made every detection fail. 60,000 games in ~2 MB is
        // the regression guard.
        let manifest = parse_manifest(&synthetic_manifest(60_000)).unwrap();
        assert_eq!(manifest.games.len(), 60_001);
        assert_eq!(manifest.games.last().unwrap().name, "Hades™");

        let by_id = GameContext {
            title: "Something Else".to_string(),
            install_dir: PathBuf::new(),
            steam_app_id: Some(1145360),
            store: None,
        };
        match manifest.find(&by_id) {
            ManifestMatch::Found(found) => {
                assert_eq!(found.name, "Hades™");
                assert_eq!(found.files.len(), 1);
                assert_eq!(found.files[0].template, "<home>/.local/share/Hades");
            }
            _ => panic!("the Steam id lookup failed"),
        }

        let by_title = GameContext {
            title: "Hades".to_string(),
            install_dir: PathBuf::new(),
            steam_app_id: None,
            store: None,
        };
        match manifest.find(&by_title) {
            ManifestMatch::Found(found) => assert_eq!(found.steam_id, Some(1145360)),
            _ => panic!("the title lookup failed"),
        }
    }

    #[test]
    fn the_official_files_mapping_shape_is_parsed() {
        // The official manifest lists `files` as a mapping of template to
        // `{tags, when}` (#236). The older sequence shape is covered by
        // `HADES_MANIFEST` and `title_matching_falls_back_…`.
        let text = r#"
"Stardew Valley":
  cloud:
    steam: true
  files:
    "<winAppData>/StardewValley/Saves":
      tags:
        - save
      when:
        - os: windows
    "<xdgConfig>/StardewValley/Saves":
      tags:
        - save
      when:
        - os: linux
  steam:
    id: 413150
"#;
        let manifest = parse_manifest(text).unwrap();
        let game = &manifest.games[0];
        assert_eq!(game.name, "Stardew Valley");
        assert_eq!(game.steam_id, Some(413150));
        assert_eq!(game.files.len(), 2, "{:?}", game.files);
        assert_eq!(game.files[0].template, "<winAppData>/StardewValley/Saves");
        assert_eq!(
            game.files[0].when,
            vec![WhenCondition::Os(vec!["windows".to_string()])]
        );
        assert_eq!(
            game.files[1].when,
            vec![WhenCondition::Os(vec!["linux".to_string()])]
        );
    }

    #[test]
    fn the_game_cap_reports_the_actual_count() {
        let error = parse_manifest_with(
            &synthetic_manifest(3),
            ManifestLimits {
                max_games: 2,
                ..ManifestLimits::default()
            },
        )
        .unwrap_err();
        assert!(error.contains("too many games"), "{error}");
        assert!(error.contains('4'), "the actual count is reported: {error}");
        assert!(error.contains("limit 2"), "the cap is reported: {error}");
    }

    #[test]
    fn one_oversized_entry_is_rejected() {
        let text = "\"Hades\":\n  files:\n    - <home>/.local/share/Hades\n";
        let error = parse_manifest_with(
            text,
            ManifestLimits {
                max_entry_bytes: 8,
                ..ManifestLimits::default()
            },
        )
        .unwrap_err();
        assert!(error.contains("single entry"), "{error}");
        assert!(error.contains("bytes"), "{error}");
    }

    #[test]
    fn yaml_aliases_are_rejected_but_literal_ampersands_are_not() {
        let alias = "\"Hades\":\n  files:\n    - *reused\n";
        let error = parse_manifest(alias).unwrap_err();
        assert!(error.contains("1 YAML anchor(s) or alias(es)"), "{error}");

        // `&`/`*` inside a quoted scalar, a comment, a block scalar or as part
        // of a path are literal text: the official manifest is full of them.
        let literal = r#"
# Tom & Jerry: a comment with &anchor
"Tom & Jerry":
  files:
    - "<home>/.local/share/Tom & Jerry/*/save"
  note: |
    &not-an-anchor and *not-an-alias
"#;
        assert_eq!(scan_manifest(literal).aliases, 0);
        assert_eq!(
            parse_manifest(literal).unwrap().games[0].files[0].template,
            "<home>/.local/share/Tom & Jerry/*/save"
        );
    }

    #[test]
    fn escaped_backslashes_and_plain_braces_do_not_desync_the_scan() {
        // The official manifest has keys like `"Indie-Dev\\BoxesWithGuns\\":`:
        // the closing quote follows an *escaped* backslash, so it ends the key
        // and the next line is a new entry. A braced plain scalar is literal
        // text, not a flow collection (#236).
        let text = r#""Indie-Dev\\BoxesWithGuns\\":
  files:
    "<home>/Game": {}
"Hades":
  files:
    <base>/{slot}/save: {}
"#;
        let scan = scan_manifest(text);
        assert_eq!(scan.entries.len(), 2);
        assert_eq!(scan.aliases, 0);
        let manifest = parse_manifest_with(
            text,
            ManifestLimits {
                chunk_games: 1,
                ..ManifestLimits::default()
            },
        )
        .unwrap();
        assert_eq!(manifest.games.len(), 2);
        assert_eq!(manifest.games[1].files[0].template, "<base>/{slot}/save");
    }

    #[test]
    fn deeply_nested_yaml_fails_without_crashing() {
        let text = format!(
            "\"Hades\":\n  files: {}x{}\n",
            "[".repeat(300),
            "]".repeat(300)
        );
        let error = parse_manifest(&text).unwrap_err();
        assert!(error.starts_with("Invalid Ludusavi manifest"), "{error}");
    }

    #[test]
    fn the_node_budget_is_enforced() {
        let error = parse_manifest_with(
            &synthetic_manifest(4),
            ManifestLimits {
                max_nodes: 5,
                ..ManifestLimits::default()
            },
        )
        .unwrap_err();
        assert!(error.contains("YAML nodes"), "{error}");
    }

    #[test]
    fn a_dollar_named_game_is_not_metadata() {
        // The official manifest ships a game called "$1 Ride", so a blanket
        // "skip keys starting with $" silently dropped it. Only a reserved name
        // with a non-mapping value is metadata.
        let text = "\"$schema\": \"https://example/manifest\"\n\
                    version: 3\n\
                    \"$1 Ride\":\n  files:\n    - <home>/.config/1Ride\n\
                    Hades:\n  files:\n    - <home>/.config/Hades\n";
        let manifest = parse_manifest(text).unwrap();
        assert_eq!(
            manifest
                .games
                .iter()
                .map(|game| game.name.as_str())
                .collect::<Vec<_>>(),
            vec!["$1 Ride", "Hades"]
        );
        assert_eq!(manifest.games[0].files.len(), 1);
    }

    #[test]
    fn a_wrapped_games_mapping_still_parses() {
        let text = "games:\n  \"Hades\":\n    files:\n      - <home>/.config/Hades\n";
        let manifest = parse_manifest(text).unwrap();
        assert_eq!(manifest.games.len(), 1);
        assert_eq!(manifest.games[0].name, "Hades");
    }

    #[test]
    fn a_flow_style_manifest_is_not_split_at_a_closing_brace() {
        // The closing brace of a flow mapping sits in the first column, which a
        // naive line split would read as a new entry. Chunking must keep each
        // flow-style game whole.
        let mut text = String::new();
        for index in 0..5 {
            text.push_str(&format!(
                "\"Game {index}\": {{\n  files: {{\"<home>/Game {index}\": {{}}}}\n}}\n"
            ));
        }
        text.push_str("\"Hades\": {files: {\"<home>/Hades\": {}}}\n");
        let manifest = parse_manifest_with(
            &text,
            ManifestLimits {
                chunk_games: 2,
                ..ManifestLimits::default()
            },
        )
        .unwrap();
        assert_eq!(manifest.games.len(), 6);
    }

    #[test]
    fn the_parsed_manifest_is_cached_by_content() {
        // A private cache: the process-wide one is shared with every other
        // test in this binary, and a cache assertion must not race them.
        let cache = Mutex::new(ManifestCache::default());
        let root = temp_root("manifest-cache");
        let path = root.join("manifest.yaml");
        fs::write(&path, HADES_MANIFEST).unwrap();
        let first = load_manifest_with(&path, &cache).unwrap();
        let second = load_manifest_with(&path, &cache).unwrap();
        assert!(Arc::ptr_eq(&first.manifest, &second.manifest));
        assert_eq!(first.version, second.version);

        // Same length, different bytes: the digest must invalidate the cache.
        let replaced = HADES_MANIFEST.replace("Hades", "Hadet");
        assert_eq!(replaced.len(), HADES_MANIFEST.len());
        fs::write(&path, &replaced).unwrap();
        let third = load_manifest_with(&path, &cache).unwrap();
        assert!(!Arc::ptr_eq(&first.manifest, &third.manifest));
        assert_eq!(third.manifest.games[0].name, "Hadet™");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_manifest_cache_keeps_one_entry_per_path() {
        let mut cache = ManifestCache::default();
        let alpha = Path::new("/manifests/extras/alpha.yaml");
        let beta = Path::new("/manifests/extras/beta.yaml");
        let parsed = Arc::new(Manifest {
            games: vec![ManifestGame {
                name: "Hades".to_string(),
                ..ManifestGame::default()
            }],
        });
        cache.put(alpha, "aaa", parsed.clone());
        cache.put(beta, "bbb", parsed.clone());

        assert!(Arc::ptr_eq(&cache.get(alpha, "aaa").unwrap(), &parsed));
        assert!(Arc::ptr_eq(&cache.get(beta, "bbb").unwrap(), &parsed));
        assert!(cache.get(alpha, "changed").is_none());
        assert!(cache.get(Path::new("/elsewhere.yaml"), "aaa").is_none());

        // A second write of the same path replaces its entry only.
        let other = Arc::new(Manifest { games: Vec::new() });
        cache.put(alpha, "ccc", other.clone());
        assert!(Arc::ptr_eq(&cache.get(alpha, "ccc").unwrap(), &other));
        assert!(Arc::ptr_eq(&cache.get(beta, "bbb").unwrap(), &parsed));
    }

    #[test]
    fn an_oversized_manifest_file_is_rejected_with_its_size() {
        let root = temp_root("manifest-oversize");
        let path = root.join("manifest.yaml");
        fs::write(&path, vec![b'#'; MAX_MANIFEST_BYTES as usize + 1]).unwrap();
        let error = match load_manifest(&path) {
            Err(ManifestLoadError::TooLarge(bytes)) => bytes,
            _ => panic!("the size cap did not apply"),
        };
        assert_eq!(error, MAX_MANIFEST_BYTES + 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn title_matching_falls_back_to_edition_stripped_titles() {
        let fixture = fixture("saves-title");
        let save_dir = fixture.home.join(".config/Hades");
        fs::create_dir_all(&save_dir).unwrap();
        fs::write(save_dir.join("save.dat"), b"x").unwrap();
        let manifest = r#"
"Hades Game of the Year Edition":
  files:
    - <home>/.config/Hades
"#;

        let mut game = fixture.game.clone();
        game.steam_app_id = None;
        game.title = "Hades".to_string();
        let detection = detect_with_context(&fixture, manifest, &game);
        assert!(detection.detected, "{detection:?}");
        assert_eq!(
            detection.saves[0].path,
            save_dir.canonicalize().unwrap().to_string_lossy()
        );

        game.title = "Unrelated Game".to_string();
        let detection = detect_with_context(&fixture, manifest, &game);
        assert!(!detection.detected);
        assert!(detection.message.contains("not in the Ludusavi manifest"));
    }

    #[test]
    fn when_filters_honour_os_and_store() {
        let fixture = fixture("saves-when");
        let manifest = r#"
"Filtered Game":
  files:
    - path: <home>/linux-save
      when:
        - os: linux
    - path: <home>/gog-save
      when:
        - os: linux
        - store: gog
    - path: <home>/windows-save
      when:
        - os: windows
"#;
        for dir in ["linux-save", "gog-save", "windows-save"] {
            fs::create_dir_all(fixture.home.join(dir)).unwrap();
        }
        let mut game = fixture.game.clone();
        game.steam_app_id = None;
        game.title = "Filtered Game".to_string();

        let detection = detect_with_context(&fixture, manifest, &game);
        let names: Vec<String> = detection
            .saves
            .iter()
            .map(|save| {
                Path::new(&save.path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        assert_eq!(names, vec!["linux-save", "windows-save"], "{names:?}");

        // Without a known store, the GOG-only entry is skipped and reported.
        game.store = None;
        let detection = detect_with_context(&fixture, manifest, &game);
        assert!(detection
            .warnings
            .iter()
            .any(|warning| warning.contains("store") && warning.contains("gog")));
    }

    #[test]
    fn missing_saves_are_retried_after_the_first_launch() {
        let fixture = fixture("saves-retry");
        let manifest = r#"
"Late Game":
  files:
    - <home>/.config/LateGame
"#;
        let mut game = fixture.game.clone();
        game.steam_app_id = None;
        game.title = "Late Game".to_string();

        let detection = detect_with_context(&fixture, manifest, &game);
        assert!(!detection.detected, "{detection:?}");
        assert!(detection.saves.is_empty());
        assert!(detection.message.contains("No save files exist yet"));

        let save_dir = fixture.home.join(".config/LateGame");
        fs::create_dir_all(&save_dir).unwrap();
        fs::write(save_dir.join("save.dat"), b"abc").unwrap();
        let detection = detect_with_context(&fixture, manifest, &game);
        assert!(detection.detected, "{detection:?}");
        assert_eq!(detection.saves.len(), 1);
        assert_eq!(detection.saves[0].size_bytes, 3);
    }

    #[test]
    fn registry_only_games_report_an_explicit_state() {
        let fixture = fixture("saves-registry");
        let manifest = r#"
"Registry Game":
  registry:
    "HKEY_CURRENT_USER\\Software\\RegistryGame":
      - "Save"
"#;
        let mut game = fixture.game.clone();
        game.steam_app_id = None;
        game.title = "Registry Game".to_string();

        let detection = detect_with_context(&fixture, manifest, &game);
        assert!(!detection.detected);
        assert!(
            detection.message.contains("registry"),
            "{}",
            detection.message
        );
    }

    #[test]
    fn glob_expands_only_within_a_segment_and_profiles_aggregate() {
        let fixture = fixture("saves-glob");
        let manifest = r#"
"Profile Game":
  files:
    - <home>/.config/ProfileGame/profile-*/save.dat
"#;
        for profile in ["profile-a", "profile-b"] {
            let dir = fixture.home.join(".config/ProfileGame").join(profile);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("save.dat"), b"save").unwrap();
        }
        let mut game = fixture.game.clone();
        game.steam_app_id = None;
        game.title = "Profile Game".to_string();

        let detection = detect_with_context(&fixture, manifest, &game);
        assert_eq!(detection.saves.len(), 2, "{:?}", detection.saves);
        assert!(detection.saves.iter().all(|save| save.kind == "file"));
        assert_eq!(detection.total_bytes, 8);
    }

    #[test]
    fn a_directory_wins_over_its_own_files() {
        let fixture = fixture("saves-aggregate");
        let manifest = r#"
"Aggregate Game":
  files:
    - <home>/.config/AggregateGame
    - <home>/.config/AggregateGame/save.dat
"#;
        let dir = fixture.home.join(".config/AggregateGame");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("save.dat"), b"x").unwrap();
        let mut game = fixture.game.clone();
        game.steam_app_id = None;
        game.title = "Aggregate Game".to_string();

        let detection = detect_with_context(&fixture, manifest, &game);
        assert_eq!(detection.saves.len(), 1);
        assert_eq!(detection.saves[0].kind, "directory");
    }

    #[test]
    fn multi_user_prefixes_are_aggregated() {
        let fixture = fixture("saves-profiles");
        let manifest = r#"
"Profile Game":
  files:
    - <winDocuments>/ProfileGame
"#;
        for user in ["steamuser", "alice"] {
            let dir = fixture
                .prefix
                .join("drive_c/users")
                .join(user)
                .join("Documents/ProfileGame");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("slot.sav"), b"x").unwrap();
        }
        let mut game = fixture.game.clone();
        game.steam_app_id = None;
        game.title = "Profile Game".to_string();

        let detection = detect_with_context(&fixture, manifest, &game);
        assert_eq!(detection.saves.len(), 2, "{:?}", detection.saves);
        for save in &detection.saves {
            assert!(save.path.contains("ProfileGame"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn traversal_and_symlink_escapes_are_rejected() {
        let fixture = fixture("saves-escape");
        let outside = fixture.root.join("outside/secret");
        fs::create_dir_all(&outside).unwrap();

        let traversal = r#"
"Escape Game":
  files:
    - <home>/../outside/secret
"#;
        let mut game = fixture.game.clone();
        game.steam_app_id = None;
        game.title = "Escape Game".to_string();
        let detection = detect_with_context(&fixture, traversal, &game);
        assert!(detection.saves.is_empty(), "{:?}", detection.saves);
        assert!(detection
            .warnings
            .iter()
            .any(|warning| warning.contains("escapes its root") || warning.contains("'..'")));

        std::os::unix::fs::symlink(&outside, fixture.home.join("escape")).unwrap();
        let symlink_manifest = r#"
"Escape Game":
  files:
    - <home>/escape
"#;
        let detection = detect_with_context(&fixture, symlink_manifest, &game);
        assert!(detection.saves.is_empty(), "{:?}", detection.saves);
        assert!(detection
            .warnings
            .iter()
            .any(|warning| warning.contains("Refused save path")));
    }

    #[test]
    fn manual_override_validates_paths_and_primary() {
        let fixture = fixture("saves-manual");
        let save_dir = fixture.home.join(".config/ManualGame");
        fs::create_dir_all(&save_dir).unwrap();
        fs::write(save_dir.join("save.dat"), b"abcd").unwrap();

        let inputs = vec![SavePathInput {
            path: save_dir.to_string_lossy().to_string(),
            kind: None,
            primary: Some(true),
            source: None,
        }];
        let (records, detection) = manual_override(
            &fixture.env,
            &fixture.game,
            42,
            &inputs,
            "2026-11-10T00:00:00Z",
        )
        .unwrap();
        assert_eq!(records.len(), 1);
        assert!(records[0].primary);
        assert_eq!(records[0].source, SOURCE_MANUAL);
        assert_eq!(detection.saves[0].size_bytes, 4);

        // A second primary is refused.
        let two_primaries = vec![
            SavePathInput {
                path: save_dir.to_string_lossy().to_string(),
                primary: Some(true),
                ..SavePathInput::default()
            },
            SavePathInput {
                path: fixture.install.to_string_lossy().to_string(),
                primary: Some(true),
                ..SavePathInput::default()
            },
        ];
        assert!(manual_override(
            &fixture.env,
            &fixture.game,
            42,
            &two_primaries,
            "2026-11-10T00:00:00Z"
        )
        .is_err());

        // A path outside the allowlisted roots is refused.
        let outside = fixture.root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let inputs = vec![SavePathInput {
            path: outside.to_string_lossy().to_string(),
            ..SavePathInput::default()
        }];
        assert!(manual_override(
            &fixture.env,
            &fixture.game,
            42,
            &inputs,
            "2026-11-10T00:00:00Z"
        )
        .is_err());

        // A missing path is refused at set time; re-detection keeps a path that
        // vanished with a clear note instead.
        let missing = vec![SavePathInput {
            path: fixture.home.join("nope").to_string_lossy().to_string(),
            ..SavePathInput::default()
        }];
        assert!(manual_override(
            &fixture.env,
            &fixture.game,
            42,
            &missing,
            "2026-11-10T00:00:00Z"
        )
        .is_err());
    }

    #[test]
    fn manual_paths_win_over_manifest_paths() {
        let fixture = fixture("saves-precedence");
        let save_dir = fixture.home.join(".config/Hades");
        fs::create_dir_all(&save_dir).unwrap();
        fs::write(save_dir.join("save.dat"), b"x").unwrap();
        let path = save_dir
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .to_string();

        let manual = vec![SavePathRecord {
            game_id: 1,
            path: path.clone(),
            kind: "directory".to_string(),
            source: SOURCE_OVERRIDE.to_string(),
            primary: true,
            created_at: "2026-11-10T00:00:00Z".to_string(),
        }];
        let manifest_path = fixture.root.join("manifest.yaml");
        fs::write(
            &manifest_path,
            r#""Hades™":
  files:
    - <home>/.config/Hades
"#,
        )
        .unwrap();
        let mut env = fixture.env.clone();
        env.manifest_path = Some(manifest_path);

        let detection = detect(&env, &fixture.game, &manual);
        assert_eq!(detection.saves.len(), 1);
        assert_eq!(detection.saves[0].source, SOURCE_OVERRIDE);
        assert_eq!(detection.source, SOURCE_MANUAL);
        assert!(detection.message.contains("manually configured"));
    }

    #[test]
    fn persisted_result_distinguishes_no_detection() {
        let paths = persisted(1, Vec::new(), None);
        assert!(!paths.detected);
        assert!(paths.saves.is_empty());
        assert_eq!(paths.primary, None);
        assert!(paths.message.contains("No save folder detected"));

        let detection = Detection {
            detected: true,
            saves: vec![DetectedSave {
                path: "/home/player/.config/Game".to_string(),
                kind: "directory".to_string(),
                source: SOURCE_MANIFEST.to_string(),
                size_bytes: 12,
                last_modified: Some("2026-11-10T00:00:00Z".to_string()),
                notes: None,
            }],
            total_bytes: 12,
            source: SOURCE_MANIFEST.to_string(),
            manifest_version: Some("sha256:abc".to_string()),
            message: "1 save path(s) detected.".to_string(),
            warnings: Vec::new(),
            ..Detection::default()
        };
        let rows = records_from_detection(1, &detection, Some("/home/player/.config/Game"), "t");
        let record = SaveDetectionRecord {
            game_id: 1,
            version_id: 1,
            manifest_version: Some("sha256:abc".to_string()),
            detected_at: "t".to_string(),
            result_json: serde_json::to_string(&detection).unwrap(),
        };
        let paths = persisted(1, rows, Some(record));
        assert!(paths.detected);
        assert_eq!(paths.primary.as_deref(), Some("/home/player/.config/Game"));
        assert_eq!(paths.saves[0].size_bytes, 12);
        assert_eq!(paths.manifest_version.as_deref(), Some("sha256:abc"));
        assert_eq!(paths.detected_at.as_deref(), Some("t"));
    }

    #[test]
    fn api_preview_reads_the_last_json_line() {
        let response = r#"{"overall":{"totalGames":1},"games":{"Hades":{"decision":"Processed","files":{"/home/player/.config/Hades/save.sav":{"bytes":12,"ignored":false}}}}}"#;
        let paths = parse_api_preview(&format!("updating manifest…\n{response}\n")).unwrap();
        assert_eq!(
            paths,
            vec!["/home/player/.config/Hades/save.sav".to_string()]
        );

        let array_response =
            r#"{"games":[{"name":"Hades","files":[{"path":"/tmp/save.sav","bytes":1}]}]}"#;
        assert_eq!(
            parse_api_preview(array_response).unwrap(),
            vec!["/tmp/save.sav".to_string()]
        );
        assert!(parse_api_preview("not json").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn configured_ludusavi_cli_preview_is_used() {
        let fixture = fixture("saves-cli");
        let save_file = fixture.home.join("cli-save.sav");
        fs::write(&save_file, b"abc").unwrap();

        let script = fixture.root.join("ludusavi");
        let response = format!(
            r#"{{"games":{{"Hades":{{"files":{{"{}":{{"bytes":3}}}}}}}}}}"#,
            save_file.display()
        );
        let body = format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'ludusavi 0.24.0'; exit 0; fi\ncat >/dev/null\nprintf '%s\\n' '{}'\n",
            response
        );
        fs::write(&script, body).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let mut env = fixture.env.clone();
        env.ludusavi = Some(script);
        let detection = detect(&env, &fixture.game, &[]);
        assert!(detection.detected, "{detection:?}");
        assert_eq!(detection.source, "ludusavi");
        assert_eq!(detection.saves.len(), 1);
        assert_eq!(
            detection.saves[0].path,
            save_file.canonicalize().unwrap().to_string_lossy()
        );
        assert_eq!(
            detection.manifest_version.as_deref(),
            Some("ludusavi 0.24.0")
        );
    }

    #[test]
    fn manifest_precedence_is_explicit_then_managed_then_user_copy() {
        let fixture = fixture("saves-manifest-order");
        let app_data = fixture.root.join("app-data");
        let managed = app_data.join("manifest/ludusavi-manifest.yaml");
        fs::create_dir_all(managed.parent().unwrap()).unwrap();
        fs::write(&managed, HADES_MANIFEST).unwrap();
        let user = fixture.home.join(".local/share/ludusavi/manifest.yaml");
        fs::create_dir_all(user.parent().unwrap()).unwrap();
        fs::write(&user, HADES_MANIFEST).unwrap();

        let mut settings = LocalSettings::for_app_data(&app_data);
        let resolve = |settings: &LocalSettings| {
            resolve_manifest_path(
                settings,
                &app_data,
                &fixture.home,
                &fixture.env.xdg_data,
                &fixture.env.windows,
            )
        };

        // The auto-managed copy wins over a user-installed one.
        assert_eq!(resolve(&settings), (Some(managed.clone()), "managed"));

        // An explicit path wins over the managed copy.
        let explicit = fixture.root.join("explicit.yaml");
        fs::write(&explicit, HADES_MANIFEST).unwrap();
        settings.ludusavi_manifest_path = explicit.to_string_lossy().to_string();
        assert_eq!(resolve(&settings), (Some(explicit.clone()), "configured"));

        // A configured path that disappeared falls back to the managed copy.
        settings.ludusavi_manifest_path =
            fixture.root.join("gone.yaml").to_string_lossy().to_string();
        assert_eq!(resolve(&settings), (Some(managed.clone()), "managed"));

        // Without a managed copy the user-installed one is used.
        fs::remove_file(&managed).unwrap();
        assert_eq!(resolve(&settings), (Some(user.clone()), "user"));

        // Nothing left: the broken override is reported so the read error is
        // actionable, but no file is opened silently.
        fs::remove_file(&user).unwrap();
        let (path, source) = resolve(&settings);
        assert_eq!(source, "configured");
        assert!(path.unwrap().ends_with("gone.yaml"));

        settings.ludusavi_manifest_path.clear();
        assert_eq!(resolve(&settings), (None, "none"));
    }

    #[test]
    fn a_managed_manifest_is_used_by_detection() {
        let fixture = fixture("saves-managed");
        let save_dir = fixture.home.join(".config/Hades");
        fs::create_dir_all(&save_dir).unwrap();
        fs::write(save_dir.join("save.dat"), b"x").unwrap();

        let managed = fixture
            .root
            .join("app-data/manifest/ludusavi-manifest.yaml");
        fs::create_dir_all(managed.parent().unwrap()).unwrap();
        fs::write(
            &managed,
            "\"Hades\":\n  files:\n    - <home>/.config/Hades\n",
        )
        .unwrap();

        let mut env = fixture.env.clone();
        env.manifest_path = Some(managed);
        env.manifest_source = "managed".to_string();
        let detection = detect(&env, &fixture.game, &[]);
        assert!(detection.detected, "{detection:?}");
        assert_eq!(detection.source, "manifest");
        assert!(
            detection.message.contains("auto-managed"),
            "{}",
            detection.message
        );
    }

    #[test]
    fn discovery_candidates_wait_for_a_session_instead_of_a_confirmation() {
        let fixture = fixture("saves-candidates");
        fs::create_dir_all(fixture.home.join(".local/share/Hades")).unwrap();

        let detection = detect(&fixture.env, &fixture.game, &[]);
        assert!(detection.saves.is_empty(), "{:?}", detection.saves);
        assert!(!detection.detected);
        let candidate = detection
            .candidates
            .iter()
            .find(|candidate| candidate.path.ends_with(".local/share/Hades"))
            .expect("candidate");
        assert_eq!(
            candidate.source,
            crate::saves::registry::SOURCE_KNOWN_FOLDER
        );
        assert!(candidate.confidence == crate::saves::registry::CONFIDENCE_LIKELY);
        assert!(!candidate.confirmed);
        assert!(
            detection.message.contains("synced automatically"),
            "{}",
            detection.message
        );
        assert_eq!(
            detection.registry_version,
            Some(fixture.env.registry.version)
        );
        // A guess waits for a session that writes to it (#180).
        assert!(records_from_detection(1, &detection, None, "t").is_empty());
    }

    #[test]
    fn a_minecraft_folder_is_not_offered_to_another_game() {
        let fixture = fixture("saves-minecraft-guard");
        let minecraft = fixture.home.join(".minecraft/saves/world");
        fs::create_dir_all(&minecraft).unwrap();
        fs::write(minecraft.join("level.dat"), b"level").unwrap();

        // Hades: the shared launcher folder is neither a save path nor a
        // candidate, and it produces no warning (#215).
        let detection = detect(&fixture.env, &fixture.game, &[]);
        assert!(
            detection
                .candidates
                .iter()
                .all(|candidate| !candidate.path.contains(".minecraft")),
            "{:?}",
            detection
                .candidates
                .iter()
                .map(|candidate| candidate.path.clone())
                .collect::<Vec<_>>()
        );
        assert!(detection
            .warnings
            .iter()
            .all(|warning| !warning.contains("Minecraft")));
        assert!(detection.saves.is_empty());

        // Minecraft itself still gets it, as a labelled guess that waits.
        let mut game = fixture.game.clone();
        game.title = "Minecraft: Java Edition".to_string();
        let detection = detect(&fixture.env, &game, &[]);
        let candidate = detection
            .candidates
            .iter()
            .find(|candidate| candidate.path.contains(".minecraft"))
            .expect("Minecraft candidate");
        assert_eq!(candidate.source, crate::saves::registry::SOURCE_ENGINE);
        assert_eq!(
            candidate.confidence,
            crate::saves::registry::CONFIDENCE_GUESS
        );
        assert!(detection.saves.is_empty(), "{:?}", detection.saves);
    }

    #[test]
    fn an_ignored_candidate_is_never_promoted() {
        let root = temp_root("promote-ignored");
        let guard = guard_for(&root, 0);
        let saves = root.join("home/.minecraft/saves");
        fs::create_dir_all(saves.join("world")).unwrap();
        fs::write(saves.join("world/level.dat"), b"level").unwrap();

        // A path the user confirmed is high confidence and would be promoted.
        let candidate = || SaveCandidate {
            path: saves.to_string_lossy().to_string(),
            kind: "directory".to_string(),
            root_id: "home".to_string(),
            subpath: ".minecraft/saves".to_string(),
            source: crate::saves::registry::SOURCE_OVERRIDE.to_string(),
            confidence: crate::saves::registry::CONFIDENCE_CERTAIN.to_string(),
            evidence: "Observed override for Minecraft".to_string(),
            confirmed: true,
            ..SaveCandidate::default()
        };

        let mut detection = Detection {
            candidates: vec![candidate()],
            ..Detection::default()
        };
        assert_eq!(auto_promote_detected(&mut detection, &guard, &[]), 1);

        // Once dismissed, the candidate is skipped and never becomes a save
        // path, even though its evidence did not change (#215).
        let mut dismissed = Detection {
            candidates: vec![candidate()],
            ..Detection::default()
        };
        let ignored = vec![saves.to_string_lossy().to_string()];
        assert_eq!(auto_promote_detected(&mut dismissed, &guard, &ignored), 0);
        assert!(dismissed.saves.is_empty());
        assert_eq!(
            dismissed.candidates[0].auto_status,
            CANDIDATE_STATUS_SKIPPED
        );
        assert_eq!(
            dismissed.candidates[0].auto_reason.as_deref(),
            Some("You ignored this folder.")
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_manifest_save_is_not_repeated_as_a_candidate() {
        let fixture = fixture("saves-candidate-dedupe");
        let save_dir = fixture.home.join(".config/Hades");
        fs::create_dir_all(&save_dir).unwrap();
        fs::write(save_dir.join("save.dat"), b"x").unwrap();
        let manifest = r#"
"Hades":
  files:
    - <home>/.config/Hades
"#;
        let mut game = fixture.game.clone();
        game.steam_app_id = None;
        let detection = detect_with_context(&fixture, manifest, &game);
        assert_eq!(detection.saves.len(), 1, "{:?}", detection.saves);
        assert!(
            detection
                .candidates
                .iter()
                .all(|candidate| !candidate.path.ends_with(".config/Hades")),
            "{:?}",
            detection.candidates
        );
    }

    #[test]
    fn no_manifest_reports_an_actionable_state() {
        let fixture = fixture("saves-no-manifest");
        let detection = detect(&fixture.env, &fixture.game, &[]);
        assert!(!detection.detected);
        assert!(detection.message.contains("No Ludusavi manifest or CLI"));
        assert_eq!(detection.source, "none");
    }

    #[test]
    fn per_game_prefix_override_wins_over_settings() {
        let fixture = fixture("saves-prefix");
        let mut settings = LocalSettings::for_app_data(&fixture.root);
        settings.wine_prefix = fixture.prefix.to_string_lossy().to_string();

        assert_eq!(
            resolve_prefix(
                &settings,
                &fixture.home,
                &fixture.install,
                None,
                Some("/custom/pfx"),
            ),
            Some(PathBuf::from("/custom/pfx"))
        );
        assert_eq!(
            resolve_prefix(&settings, &fixture.home, &fixture.install, None, None),
            Some(fixture.prefix.clone())
        );
    }

    #[test]
    fn segment_wildcards_do_not_cross_separators() {
        assert!(segment_matches("profile-*", "profile-a"));
        assert!(segment_matches("*.sav", "slot1.sav"));
        assert!(!segment_matches("*.sav", "slot1.sav.bak"));
        assert!(!segment_matches("profile-*", "other/profile-a"));
        assert!(segment_matches("*", "anything"));
    }

    /// A native Windows machine with OneDrive-redirected Documents and every
    /// known folder resolved by the injectable resolver (#158).
    fn windows_fixture(label: &str) -> Fixture {
        let root = temp_root(label);
        let profile = root.join("Users/player");
        let documents = profile.join("OneDrive/Documents");
        let saved_games = profile.join("Saved Games");
        let local_low = profile.join("AppData/LocalLow");
        let roaming = profile.join("AppData/Roaming");
        let local = profile.join("AppData/Local");
        let install = root.join("Games/Hades");
        for dir in [
            &documents,
            &saved_games,
            &local_low,
            &roaming,
            &local,
            &install,
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::create_dir_all(root.join("ProgramData")).unwrap();
        fs::create_dir_all(root.join("Public")).unwrap();

        let mut known = ResolvedFolders::default();
        known.insert("profile", profile.clone());
        known.insert("documents", documents.clone());
        known.insert("saved_games", saved_games.clone());
        known.insert("local_app_data_low", local_low.clone());
        known.insert("roaming_app_data", roaming.clone());
        known.insert("local_app_data", local.clone());
        known.insert("program_data", root.join("ProgramData"));
        known.insert("public", root.join("Public"));

        let windows = WindowsDirs::from_folders(&known);
        let env = DetectEnv {
            home: profile.clone(),
            os_user_name: "player".to_string(),
            platform: Platform::Windows,
            xdg_data: profile.join(".local/share"),
            xdg_config: profile.join(".config"),
            xdg_state: profile.join(".local/state"),
            windows,
            known,
            registry: crate::saves::registry::registry().clone(),
            steam_roots: Vec::new(),
            prefix: None,
            steam_user_ids: Vec::new(),
            manifest_path: None,
            manifest_source: "none".to_string(),
            ludusavi: None,
        };
        let game = GameContext {
            title: "Hades".to_string(),
            install_dir: install.clone(),
            steam_app_id: Some(1145360),
            store: Some("steam".to_string()),
        };
        Fixture {
            root,
            home: profile,
            install,
            prefix: PathBuf::new(),
            env,
            game,
        }
    }

    #[test]
    fn windows_saved_games_root_is_a_candidate_without_a_manifest() {
        let fixture = windows_fixture("win-saved-games");
        let saved_games = fixture.env.known.get("saved_games").unwrap();
        fs::create_dir_all(saved_games.join("Hades")).unwrap();

        let detection = detect(&fixture.env, &fixture.game, &[]);
        assert!(
            detection
                .candidates
                .iter()
                .any(|candidate| candidate.root_id == "saved_games"),
            "{:?}",
            detection.candidates
        );
        let report = detection
            .roots
            .iter()
            .find(|root| root.id == "saved_games")
            .expect("Saved Games is reported");
        assert!(report.exists, "{report:?}");
        assert_eq!(report.candidates, 1, "{report:?}");
        assert!(detection.message.contains("synced automatically"));
    }

    #[test]
    fn windows_local_low_and_onedrive_documents_resolve() {
        let fixture = windows_fixture("win-local-low");
        let documents = fixture.env.known.get("documents").unwrap();
        assert!(documents.ends_with("OneDrive/Documents"), "{documents:?}");
        fs::create_dir_all(documents.join("My Games/Hades")).unwrap();
        fs::create_dir_all(
            fixture
                .env
                .known
                .get("local_app_data_low")
                .unwrap()
                .join("Studio/Hades"),
        )
        .unwrap();

        let detection = detect(&fixture.env, &fixture.game, &[]);
        let roots: Vec<&str> = detection
            .candidates
            .iter()
            .map(|candidate| candidate.root_id.as_str())
            .collect();
        assert!(roots.contains(&"documents"), "{:?}", detection.candidates);
        assert!(
            roots.contains(&"local_app_data_low"),
            "{:?}",
            detection.candidates
        );
        assert!(detection
            .candidates
            .iter()
            .all(|candidate| candidate.path.contains("OneDrive")
                || candidate.path.contains("LocalLow")));
    }

    #[test]
    fn windows_saved_games_manifest_placeholder_is_resolved() {
        let fixture = windows_fixture("win-manifest-saved-games");
        let save_dir = fixture.env.known.get("saved_games").unwrap().join("Hades");
        fs::create_dir_all(&save_dir).unwrap();
        fs::write(save_dir.join("slot1.sav"), b"save").unwrap();

        let manifest = r#"
"Hades":
  files:
    - <winSavedGames>/Hades
"#;
        let detection = detect_with_manifest(&fixture, manifest);
        assert!(detection.detected, "{detection:?}");
        assert_eq!(detection.saves.len(), 1, "{:?}", detection.saves);
        assert_eq!(detection.saves[0].path, save_dir.to_string_lossy());
    }

    #[test]
    fn a_managed_manifest_wins_while_the_registry_still_runs() {
        let fixture = windows_fixture("win-managed");
        let documents = fixture.env.known.get("documents").unwrap();
        let manifest_save = documents.join("Hades");
        fs::create_dir_all(&manifest_save).unwrap();
        fs::write(manifest_save.join("slot1.sav"), b"save").unwrap();
        // A second, different root: the registry must still report it as a
        // candidate even though the manifest answered for this game.
        let saved_games = fixture.env.known.get("saved_games").unwrap();
        fs::create_dir_all(saved_games.join("Hades")).unwrap();

        let path = fixture.root.join("appdata/manifest.yaml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, HADES_MANIFEST).unwrap();
        let mut env = fixture.env.clone();
        env.manifest_path = Some(path);
        env.manifest_source = "managed".to_string();

        let detection = detect(&env, &fixture.game, &[]);
        assert!(detection.detected, "{detection:?}");
        assert_eq!(detection.source, "manifest");
        assert!(
            detection.message.contains("auto-managed"),
            "{}",
            detection.message
        );
        assert!(
            detection
                .candidates
                .iter()
                .any(|candidate| candidate.root_id == "saved_games"),
            "{:?}",
            detection.candidates
        );
    }

    #[test]
    fn an_empty_windows_machine_explains_every_root() {
        let fixture = windows_fixture("win-empty");
        let detection = detect(&fixture.env, &fixture.game, &[]);
        assert!(!detection.detected);
        assert!(
            detection.candidates.is_empty(),
            "{:?}",
            detection.candidates
        );
        assert!(
            detection.message.contains("Checked"),
            "{}",
            detection.message
        );
        assert!(
            detection.message.contains("Saved Games"),
            "{}",
            detection.message
        );
        assert!(
            detection.message.contains("Documents"),
            "{}",
            detection.message
        );
        let saved_games = detection
            .roots
            .iter()
            .find(|root| root.id == "saved_games")
            .expect("Saved Games is reported");
        assert!(saved_games.exists);
        assert!(detection
            .roots
            .iter()
            .any(|root| root.id == "local_app_data_low"));
    }

    /* ------------------------------------------ automatic promotion (#180) */

    fn guard_for(root: &Path, cap_bytes: u64) -> PromotionGuard {
        let home = root.join("home");
        let documents = root.join("documents");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&documents).unwrap();
        PromotionGuard::from_parts(
            home,
            vec![documents, root.join("games")],
            Some(root.join("games")),
            cap_bytes,
        )
    }

    #[test]
    fn promotion_rejects_broad_roots_symlinks_and_oversized_folders() {
        let root = temp_root("promote-guard");
        let guard = guard_for(&root, 16);
        fs::create_dir_all(root.join("home")).unwrap();
        fs::create_dir_all(root.join("documents")).unwrap();

        let empty = PathMeasurement {
            exists: true,
            is_dir: true,
            ..PathMeasurement::default()
        };
        assert!(guard
            .check(&root.join("home"), "directory", &empty)
            .unwrap_err()
            .contains("home folder"));
        let documents_error = guard
            .check(&root.join("documents"), "directory", &empty)
            .unwrap_err()
            .to_lowercase();
        assert!(
            documents_error.contains("documents") || documents_error.contains("known"),
            "{documents_error}"
        );

        let save = root.join("home/.local/share/Hades");
        fs::create_dir_all(&save).unwrap();
        let measured = measure_path(&save, SNAPSHOT_BUDGET);
        assert!(guard.check(&save, "directory", &measured).is_ok());

        // A dotfolder that is a known folder root is rejected by name, so a
        // broad `.config`/`.local/share` can never be auto-promoted (#180,
        // #215).
        fs::create_dir_all(root.join("home/.config")).unwrap();
        fs::create_dir_all(root.join("home/.local/share")).unwrap();
        let dot_root = PromotionGuard::from_parts(
            root.join("home"),
            vec![root.join("home/.config"), root.join("home/.local/share")],
            None,
            0,
        );
        let dot_error = dot_root
            .check(&root.join("home/.config"), "directory", &empty)
            .unwrap_err();
        assert!(dot_error.contains(".config"), "{dot_error}");
        let xdg_error = dot_root
            .check(&root.join("home/.local/share"), "directory", &empty)
            .unwrap_err();
        assert!(xdg_error.contains("known"), "{xdg_error}");
        assert!(xdg_error.contains("share"), "{xdg_error}");

        // A missing folder is never promoted.
        assert!(guard
            .check(
                &save.join("missing"),
                "directory",
                &PathMeasurement::default()
            )
            .unwrap_err()
            .contains("does not exist"));

        // The size cap rejects a save folder that is too wide.
        let large = PathMeasurement {
            exists: true,
            is_dir: true,
            bytes: 4096,
            ..PathMeasurement::default()
        };
        assert!(guard
            .check(&save, "directory", &large)
            .unwrap_err()
            .contains("larger than"));

        #[cfg(unix)]
        {
            let link = root.join("save-link");
            std::os::unix::fs::symlink(&save, &link).unwrap();
            let link_measure = measure_path(&link, SNAPSHOT_BUDGET);
            assert!(guard
                .check(&link, "directory", &link_measure)
                .unwrap_err()
                .contains("symlink"));
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_session_change_needs_an_existing_folder_and_moving_bytes_or_time() {
        use crate::db::CandidateSnapshotRecord;
        let before = CandidateSnapshotRecord {
            game_id: 1,
            path: "/saves/Hades".to_string(),
            existed: true,
            files: 1,
            bytes: 10,
            newest: Some("2026-11-10T10:00:00Z".to_string()),
            captured_at: "2026-11-10T10:00:00Z".to_string(),
        };
        let unchanged = PathMeasurement {
            exists: true,
            is_dir: true,
            files: 1,
            bytes: 10,
            newest: Some("2026-11-10T10:00:00Z".to_string()),
            truncated: false,
        };
        assert!(!session_changed(&before, &unchanged));
        assert_eq!(session_changed_bytes(&before, &unchanged), 0);

        let grown = PathMeasurement {
            bytes: 42,
            ..unchanged.clone()
        };
        assert!(session_changed(&before, &grown));
        assert_eq!(session_changed_bytes(&before, &grown), 32);

        let touched = PathMeasurement {
            newest: Some("2026-11-10T10:01:00Z".to_string()),
            ..unchanged.clone()
        };
        assert!(session_changed(&before, &touched));

        let absent = CandidateSnapshotRecord {
            existed: false,
            ..before.clone()
        };
        assert!(!session_changed(&absent, &grown));

        let empty = PathMeasurement {
            files: 0,
            bytes: 0,
            ..unchanged
        };
        assert!(!session_changed(&before, &empty));
    }

    #[test]
    fn promotion_picks_the_largest_changed_folder_as_primary() {
        let targets = vec![
            PromotionTarget {
                path: "/saves/alpha".to_string(),
                changed_bytes: 10,
                total_bytes: 100,
            },
            PromotionTarget {
                path: "/saves/beta".to_string(),
                changed_bytes: 40,
                total_bytes: 80,
            },
            PromotionTarget {
                path: "/saves/gamma".to_string(),
                changed_bytes: 40,
                total_bytes: 80,
            },
        ];
        assert_eq!(
            choose_promotion_primary(&targets).as_deref(),
            Some("/saves/beta")
        );
        assert_eq!(choose_promotion_primary(&[]), None);
    }

    #[test]
    fn high_confidence_candidates_are_promoted_and_guesses_wait() {
        let root = temp_root("promote-detect");
        let guard = guard_for(&root, 0);
        let store = root.join("home/.local/share/Steam/userdata/1/Hades");
        fs::create_dir_all(&store).unwrap();
        fs::write(store.join("slot.sav"), b"1234").unwrap();
        let guess = root.join("home/.config/unity3d/Hades");
        fs::create_dir_all(&guess).unwrap();
        fs::write(guess.join("save"), b"12").unwrap();

        let mut detection = Detection {
            candidates: vec![
                SaveCandidate {
                    path: store.to_string_lossy().to_string(),
                    kind: "directory".to_string(),
                    root_id: "steam_userdata".to_string(),
                    subpath: "Hades".to_string(),
                    source: crate::saves::registry::SOURCE_STORE.to_string(),
                    confidence: crate::saves::registry::CONFIDENCE_CERTAIN.to_string(),
                    evidence: "Steam Cloud".to_string(),
                    ..SaveCandidate::default()
                },
                SaveCandidate {
                    path: guess.to_string_lossy().to_string(),
                    kind: "directory".to_string(),
                    root_id: "xdg_config".to_string(),
                    subpath: "unity3d/Hades".to_string(),
                    source: crate::saves::registry::SOURCE_ENGINE.to_string(),
                    confidence: crate::saves::registry::CONFIDENCE_GUESS.to_string(),
                    evidence: "Unity default".to_string(),
                    ..SaveCandidate::default()
                },
            ],
            ..Detection::default()
        };

        let promoted = auto_promote_detected(&mut detection, &guard, &[]);
        assert_eq!(promoted, 1);
        assert_eq!(detection.saves.len(), 1);
        assert!(detection.saves[0].path.contains("Steam"));
        assert_eq!(detection.candidates.len(), 2);
        let store_candidate = detection
            .candidates
            .iter()
            .find(|candidate| candidate.source == crate::saves::registry::SOURCE_STORE)
            .unwrap();
        assert_eq!(store_candidate.auto_status, CANDIDATE_STATUS_SAVING);
        let guess_candidate = detection
            .candidates
            .iter()
            .find(|candidate| candidate.source == crate::saves::registry::SOURCE_ENGINE)
            .unwrap();
        assert_eq!(guess_candidate.auto_status, "");

        // A dismissed high-confidence path is never revived automatically.
        let mut dismissed = Detection {
            candidates: detection.candidates.clone(),
            ..Detection::default()
        };
        let ignored = vec![store.to_string_lossy().to_string()];
        assert_eq!(auto_promote_detected(&mut dismissed, &guard, &ignored), 0);
        assert!(dismissed.saves.is_empty());
        let dismissed_store = dismissed
            .candidates
            .iter()
            .find(|candidate| candidate.source == crate::saves::registry::SOURCE_STORE)
            .unwrap();
        assert_eq!(dismissed_store.auto_status, CANDIDATE_STATUS_SKIPPED);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn official_manifest_size_parses() {
        // The published manifest has 53,549 top-level games as of 2026-11-16,
        // so a cap below that rejects the real file for every user.
        let count = OFFICIAL_MANIFEST_GAMES;
        let mut manifest = String::new();
        for index in 0..count {
            manifest.push_str(&format!(
                "\"Game {index}\":\n  files:\n    - <winDocuments>/Game{index}\n"
            ));
        }
        let parsed = parse_manifest(&manifest).expect("the official manifest size must parse");
        assert_eq!(parsed.games.len(), count);
    }

    #[test]
    fn an_oversized_manifest_is_still_rejected() {
        let count = MAX_MANIFEST_GAMES + 1;
        let mut manifest = String::new();
        for index in 0..count {
            manifest.push_str(&format!(
                "\"Game {index}\":\n  files:\n    - <winDocuments>/Game{index}\n"
            ));
        }
        let error = parse_manifest(&manifest).expect_err("an oversized manifest must be refused");
        assert!(error.contains("too many games"), "{error}");
    }

    #[test]
    fn bounded_measurement_stops_on_a_huge_tree() {
        let root = temp_root("promote-measure");
        let saves = root.join("saves");
        fs::create_dir_all(&saves).unwrap();
        for index in 0..600 {
            fs::write(saves.join(format!("slot-{index:04}.sav")), b"x").unwrap();
        }
        let started = std::time::Instant::now();
        let measured = measure_path(&saves, Duration::from_millis(0));
        assert!(measured.exists);
        assert!(measured.truncated);
        assert!(measured.files < 600, "{measured:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
        let _ = fs::remove_dir_all(&root);
    }
}
