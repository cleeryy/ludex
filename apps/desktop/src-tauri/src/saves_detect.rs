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
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;

use crate::db::{InstalledGame, SaveDetectionRecord, SavePathRecord};
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
const MAX_MANIFEST_GAMES: usize = 50_000;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
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
    pub documents: Option<PathBuf>,
    pub public: Option<PathBuf>,
    pub program_data: Option<PathBuf>,
    pub win_dir: Option<PathBuf>,
}

impl WindowsDirs {
    fn from_env() -> Self {
        Self {
            app_data: env_path("APPDATA"),
            local_app_data: env_path("LOCALAPPDATA"),
            documents: env_path("USERPROFILE").map(|home| home.join("Documents")),
            public: env_path("PUBLIC"),
            program_data: env_path("PROGRAMDATA"),
            win_dir: env_path("SYSTEMROOT").or_else(|| env_path("WINDIR")),
        }
    }

    fn roots(&self) -> Vec<PathBuf> {
        [
            &self.app_data,
            &self.local_app_data,
            &self.documents,
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
    pub windows: WindowsDirs,
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
        let home = home_dir();
        let os_user_name = current_user_name(&home);
        let xdg_data = env_path("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share"));
        let xdg_config = env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
        let windows = WindowsDirs::from_env();
        let mut context = GameContext::from_installed(game);
        if context.steam_app_id.is_none() {
            context.steam_app_id = app_id_override;
        }
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
            windows,
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

/// Run detection for an installed game, merging persisted manual paths.
pub fn detect_for_game(
    settings: &LocalSettings,
    game: &InstalledGame,
    app_data: &Path,
    manual: &[SavePathRecord],
    prefix_override: Option<&str>,
    app_id_override: Option<u32>,
) -> Detection {
    let env = DetectEnv::for_game(settings, game, app_data, prefix_override, app_id_override);
    let mut context = GameContext::from_installed(game);
    if context.steam_app_id.is_none() {
        context.steam_app_id = app_id_override;
    }
    detect(&env, &context, manual)
}

/// Run detection with an explicit environment. This is the testable core.
pub fn detect(env: &DetectEnv, game: &GameContext, manual: &[SavePathRecord]) -> Detection {
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
        match fs::read_to_string(path) {
            Ok(text) if text.len() as u64 > MAX_MANIFEST_BYTES => {
                detection.warnings.push(format!(
                    "{} is larger than {}; it was not parsed.",
                    path.display(),
                    crate::util::human_bytes(MAX_MANIFEST_BYTES)
                ));
                status = Some(
                    "The Ludusavi manifest is too large to parse safely. Add a save path manually."
                        .to_string(),
                );
            }
            Ok(text) => match parse_manifest(&text) {
                Ok(manifest) => {
                    manifest_version = Some(manifest_version_for_text(&text));
                    match manifest.find(game) {
                        ManifestMatch::Found(found) => {
                            let walk =
                                walk_manifest(found, env, game, &roots, &mut detection.warnings);
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
                Err(error) => {
                    detection.warnings.push(error.clone());
                    status = Some(format!("The Ludusavi manifest could not be read: {error}"));
                }
            },
            Err(error) => {
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
    } else {
        status.unwrap_or_else(|| NO_SAVE_MESSAGE.to_string())
    };

    detection.warnings.sort();
    detection.warnings.dedup();
    detection.manifest_version = manifest_version;
    detection
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
    let roots = canonical_roots(env, game);
    let mut records: Vec<SavePathRecord> = Vec::new();
    let mut saves: Vec<DetectedSave> = Vec::new();
    let mut primary_taken = false;

    for input in inputs {
        let raw = input.path.trim();
        if raw.is_empty() {
            return Err("A save path cannot be empty.".to_string());
        }
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

        // Test readability before persisting.
        if kind == "directory" {
            fs::read_dir(&canonical)
                .map_err(|error| format!("Could not list {}: {error}", canonical.display()))?;
        } else {
            fs::File::open(&canonical)
                .map_err(|error| format!("Could not read {}: {error}", canonical.display()))?;
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

    let managed = crate::saves_manifest::ManifestPaths::for_app_data(app_data).manifest;
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

fn canonical_roots(env: &DetectEnv, game: &GameContext) -> Vec<PathBuf> {
    let mut roots = vec![
        env.home.clone(),
        env.xdg_data.clone(),
        env.xdg_config.clone(),
        game.install_dir.clone(),
    ];
    if let Some(prefix) = &env.prefix {
        roots.push(prefix_drive_c(prefix));
    }
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

/// Parse a Ludusavi manifest. Both the normal `Title: {files: […]}` shape and a
/// wrapping `games:` mapping are accepted. Nothing is executed.
pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let root: YamlValue = serde_yaml::from_str(text)
        .map_err(|error| format!("Invalid Ludusavi manifest: {error}"))?;
    let mapping = root
        .as_mapping()
        .ok_or_else(|| "Invalid Ludusavi manifest: the root is not a mapping.".to_string())?;

    let mut games = Vec::new();
    let mut push_game = |name: &str, value: &YamlValue| {
        if let Some(game) = parse_manifest_game(name, value) {
            games.push(game);
        }
    };

    if let Some(wrapped) = mapping_get(mapping, "games").and_then(YamlValue::as_mapping) {
        for (key, value) in wrapped {
            if let Some(name) = key.as_str() {
                push_game(name, value);
            }
        }
    } else {
        for (key, value) in mapping {
            let Some(name) = key.as_str() else {
                continue;
            };
            if name.starts_with('$') || name.eq_ignore_ascii_case("version") {
                continue;
            }
            push_game(name, value);
        }
    }

    if games.len() > MAX_MANIFEST_GAMES {
        return Err("The Ludusavi manifest lists too many games.".to_string());
    }
    Ok(Manifest { games })
}

fn parse_manifest_game(name: &str, value: &YamlValue) -> Option<ManifestGame> {
    let mapping = value.as_mapping()?;
    let mut game = ManifestGame {
        name: name.to_string(),
        ..ManifestGame::default()
    };

    if let Some(files) = mapping_get(mapping, "files").and_then(YamlValue::as_sequence) {
        for item in files {
            if let Some(file) = parse_manifest_file(item) {
                game.files.push(file);
            }
        }
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

fn manifest_version_for_text(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(text.as_bytes());
    let encoded = hex::encode(digest);
    format!("sha256:{}", &encoded[..12])
}

// ---------------------------------------------------------------------------
// Template expansion
// ---------------------------------------------------------------------------

const TEMPLATES: [&str; 12] = [
    "<winLocalAppData>",
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

    let mut command = Command::new(binary);
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
    let mut command = Command::new(binary);
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

        let env = DetectEnv {
            home: home.clone(),
            os_user_name: "player".to_string(),
            platform: Platform::Linux,
            xdg_data: home.join(".local/share"),
            xdg_config: home.join(".config"),
            windows: WindowsDirs::default(),
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
}
