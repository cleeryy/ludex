//! Sunshine host integration (#97) and Moonlight client integration (#98).
//!
//! Both applications are external: Ludex never bundles, installs or updates
//! them. It only edits Sunshine's `apps.json` after an explicit action (with a
//! backup), and it launches the local Moonlight binary with the host and app the
//! user picked. Everything here is best-effort filesystem inspection; a missing
//! install is a clear status, never an error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::db::InstalledGame;
use crate::settings::LocalSettings;

/// Marker object written inside every Ludex-managed Sunshine app entry.
pub const MANAGED_KEY: &str = "ludex";
/// Marker version, bumped if the shape ever changes.
const MANAGED_VERSION: i64 = 1;
/// Prefix of the launch URI used when a game has no local command.
pub const LUDEX_URI_PREFIX: &str = "ludex://play/";

/* ------------------------------- Sunshine -------------------------------- */

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SunshinePaths {
    /// Absolute path to `apps.json`.
    pub apps_json: String,
    /// Absolute path to `sunshine.conf`.
    pub sunshine_conf: String,
    /// Where the folder came from (`Program Files`, `XDG`, `override`…).
    pub source: String,
    /// True when the path came from a Ludex setting.
    pub overridden: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SunshineDetection {
    pub platform: String,
    pub paths: Option<SunshinePaths>,
    pub binary: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SunshineStatus {
    pub detected: bool,
    pub platform: String,
    pub apps_path: Option<String>,
    pub conf_path: Option<String>,
    pub binary: Option<String>,
    pub source: Option<String>,
    pub overridden: bool,
    pub apps_found: bool,
    pub entries: usize,
    pub managed_entries: usize,
    pub warnings: Vec<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishedApp {
    pub game_id: i64,
    pub title: String,
    pub name: String,
    pub cmd: String,
    pub working_dir: String,
    pub image_path: Option<String>,
    /// The command is a `ludex://` URI, which needs a registered handler.
    pub needs_ludex_scheme: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedGame {
    pub game_id: i64,
    pub title: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewEntry {
    pub game_id: i64,
    pub title: String,
    pub name: String,
    pub cmd: String,
    pub working_dir: String,
    pub image_path: Option<String>,
    /// `add`, `update` or `unchanged`.
    pub action: String,
    pub needs_ludex_scheme: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRemoval {
    pub name: String,
    pub game_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SunshinePreview {
    pub status: SunshineStatus,
    pub entries: Vec<PreviewEntry>,
    pub removals: Vec<PreviewRemoval>,
    pub skipped: Vec<SkippedGame>,
    pub can_sync: bool,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeReport {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub managed_entries: usize,
    pub changed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SunshineReport {
    pub written: bool,
    pub dry_run: bool,
    pub apps_path: Option<String>,
    pub backup_path: Option<String>,
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub managed_entries: usize,
    pub skipped: Vec<SkippedGame>,
    pub message: String,
}

/// Windows, Linux and macOS candidate folders, in priority order.
pub fn sunshine_candidates(
    platform: &str,
    home: Option<&Path>,
    env: &BTreeMap<String, String>,
) -> Vec<SunshinePaths> {
    let mut out = Vec::new();
    let mut push = |dir: PathBuf, source: &str| {
        out.push(SunshinePaths {
            apps_json: dir.join("apps.json").to_string_lossy().to_string(),
            sunshine_conf: dir.join("sunshine.conf").to_string_lossy().to_string(),
            source: source.to_string(),
            overridden: false,
        });
    };

    match platform {
        "windows" => {
            if let Some(root) = env.get("ProgramFiles") {
                push(Path::new(root).join("Sunshine").join("config"), "Program Files");
                push(Path::new(root).join("Sunshine"), "Program Files (portable)");
            }
            if let Some(root) = env.get("ProgramFiles(x86)") {
                push(
                    Path::new(root).join("Sunshine").join("config"),
                    "Program Files (x86)",
                );
            }
            if let Some(root) = env.get("LOCALAPPDATA") {
                push(Path::new(root).join("Sunshine"), "Local AppData");
            }
        }
        "macos" => {
            if let Some(home) = home {
                push(home.join(".config").join("sunshine"), "XDG");
                push(
                    home.join("Library").join("Application Support").join("Sunshine"),
                    "Application Support",
                );
            }
        }
        _ => {
            if let Some(home) = home {
                push(home.join(".config").join("sunshine"), "XDG");
                push(
                    home.join(".var").join("app").join("dev.lizardbyte.app.Sunshine").join("config").join("sunshine"),
                    "Flatpak",
                );
            }
            push(PathBuf::from("/etc/sunshine"), "System");
        }
    }
    out
}

fn env_map() -> BTreeMap<String, String> {
    std::env::vars().collect()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn platform_name() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

fn looks_like_sunshine_dir(paths: &SunshinePaths) -> bool {
    Path::new(&paths.apps_json).is_file() || Path::new(&paths.sunshine_conf).is_file()
}

/// Detect a Sunshine install, honouring the two user overrides.
pub fn detect_sunshine(
    settings: &LocalSettings,
    platform: &str,
    home: Option<&Path>,
    env: &BTreeMap<String, String>,
) -> SunshineDetection {
    let mut warnings = Vec::new();
    let candidates = sunshine_candidates(platform, home, env);

    let apps_override = settings.sunshine_apps_path.trim();
    let conf_override = settings.sunshine_conf_path.trim();
    let mut paths = None;

    if !apps_override.is_empty() || !conf_override.is_empty() {
        let apps = if apps_override.is_empty() {
            Path::new(conf_override)
                .parent()
                .map(|parent| parent.join("apps.json"))
                .unwrap_or_else(|| PathBuf::from("apps.json"))
        } else {
            PathBuf::from(apps_override)
        };
        let conf = if conf_override.is_empty() {
            apps.parent()
                .map(|parent| parent.join("sunshine.conf"))
                .unwrap_or_else(|| PathBuf::from("sunshine.conf"))
        } else {
            PathBuf::from(conf_override)
        };
        let parent_ok = apps
            .parent()
            .map(|parent| parent.as_os_str().is_empty() || parent.is_dir())
            .unwrap_or(false);
        if parent_ok {
            paths = Some(SunshinePaths {
                apps_json: apps.to_string_lossy().to_string(),
                sunshine_conf: conf.to_string_lossy().to_string(),
                source: "override".to_string(),
                overridden: true,
            });
        } else {
            warnings.push(format!(
                "The configured Sunshine folder does not exist: {}. Detection falls back to the default locations.",
                apps.parent().unwrap_or(Path::new("")).display()
            ));
        }
    }

    if paths.is_none() {
        paths = candidates.into_iter().find(looks_like_sunshine_dir);
    }

    let binary = sunshine_binary(platform, home, env);
    if paths.is_none() && binary.is_none() {
        warnings.push(
            "Sunshine was not found. Install Sunshine on the machine that will host the stream, then rescan."
                .to_string(),
        );
    }

    SunshineDetection {
        platform: platform.to_string(),
        paths,
        binary,
        warnings,
    }
}

fn sunshine_binary(
    platform: &str,
    _home: Option<&Path>,
    env: &BTreeMap<String, String>,
) -> Option<String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    match platform {
        "windows" => {
            if let Some(root) = env.get("ProgramFiles") {
                candidates.push(Path::new(root).join("Sunshine").join("sunshine.exe"));
            }
            if let Some(root) = env.get("LOCALAPPDATA") {
                candidates.push(
                    Path::new(root)
                        .join("Programs")
                        .join("Sunshine")
                        .join("sunshine.exe"),
                );
            }
        }
        "macos" => {
            if let Some(home) = _home {
                candidates.push(
                    home.join("Applications")
                        .join("Sunshine.app")
                        .join("Contents")
                        .join("MacOS")
                        .join("sunshine"),
                );
            }
        }
        _ => {
            candidates.push(PathBuf::from("/usr/bin/sunshine"));
            candidates.push(PathBuf::from("/usr/local/bin/sunshine"));
            candidates.push(PathBuf::from(
                "/var/lib/flatpak/exports/bin/dev.lizardbyte.app.Sunshine",
            ));
        }
    }

    if let Some(path) = candidates.into_iter().find(|path| path.is_file()) {
        return Some(path.to_string_lossy().to_string());
    }
    crate::paths::find_on_path("sunshine").map(|path| path.to_string_lossy().to_string())
}

/// Read Sunshine's app database. Unknown keys survive; a malformed file is an
/// error so nothing is ever overwritten blindly.
pub fn parse_apps_json(text: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_str(text).map_err(|error| {
        format!(
            "Sunshine's apps.json is not valid JSON ({error}). Nothing was changed; fix or restore the file first."
        )
    })?;
    match &value {
        Value::Array(_) => Ok(value),
        Value::Object(map) => match map.get("apps") {
            Some(Value::Array(_)) => Ok(value),
            Some(_) => Err(
                "Sunshine's apps.json has an \"apps\" key that is not an array. Nothing was changed."
                    .to_string(),
            ),
            None => Ok(value),
        },
        _ => Err(
            "Sunshine's apps.json must contain an object or an array of apps. Nothing was changed."
                .to_string(),
        ),
    }
}

fn apps_ref(value: &Value) -> Result<&Vec<Value>, String> {
    match value {
        Value::Array(apps) => Ok(apps),
        Value::Object(map) => match map.get("apps") {
            Some(Value::Array(apps)) => Ok(apps),
            None => Err("missing".to_string()),
            Some(_) => Err("not-an-array".to_string()),
        },
        _ => Err("invalid".to_string()),
    }
}

fn apps_mut(value: &mut Value) -> Result<&mut Vec<Value>, String> {
    match value {
        Value::Array(apps) => Ok(apps),
        Value::Object(map) => {
            if !map.contains_key("apps") {
                map.insert("apps".to_string(), Value::Array(Vec::new()));
            }
            match map.get_mut("apps") {
                Some(Value::Array(apps)) => Ok(apps),
                _ => Err("The \"apps\" key in Sunshine's apps.json is not an array.".to_string()),
            }
        }
        _ => Err("Sunshine's apps.json must contain an object or an array.".to_string()),
    }
}

fn as_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Marker written by Ludex, if present.
fn managed_marker(app: &Value) -> Option<(Option<i64>, Option<String>)> {
    let marker = app.get(MANAGED_KEY)?.as_object()?;
    let managed = marker.get("managed").and_then(Value::as_bool).unwrap_or(true);
    if !managed {
        return None;
    }
    let game_id = marker.get("gameId").and_then(Value::as_i64);
    let server = as_string(marker.get("serverId"));
    Some((game_id, server))
}

fn ludex_uri_game_id(app: &Value) -> Option<i64> {
    let cmd = as_string(app.get("cmd"))?;
    let rest = cmd.strip_prefix(LUDEX_URI_PREFIX)?;
    rest.split(['?', '#']).next()?.trim().parse().ok()
}

/// A Sunshine entry is managed by Ludex when it carries the marker object or a
/// `ludex://play/<id>` command (hand-written entries from older syncs).
pub fn is_managed_app(app: &Value) -> bool {
    app.is_object() && (managed_marker(app).is_some() || ludex_uri_game_id(app).is_some())
}

fn managed_game_id(app: &Value) -> Option<i64> {
    managed_marker(app)
        .and_then(|(game_id, _)| game_id)
        .or_else(|| ludex_uri_game_id(app))
}

fn managed_server(app: &Value) -> Option<String> {
    managed_marker(app).and_then(|(_, server)| server)
}

fn quote_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    if raw.contains([' ', '\t', '"']) {
        format!("\"{}\"", raw.replace('"', "\\\""))
    } else {
        raw.into_owned()
    }
}

/// Build the Sunshine entry for one installed game.
///
/// The order is: an external launch URI (Steam), the recorded executable, then
/// a `ludex://play/<id>` URI. The last case is marked because it needs a
/// registered protocol handler; the UI shows it instead of pretending it works.
pub fn publish_app(
    game: &InstalledGame,
    image_path: Option<String>,
) -> Result<PublishedApp, SkippedGame> {
    let title = game.title.trim();
    if title.is_empty() {
        return Err(SkippedGame {
            game_id: game.game_id,
            title: game.title.clone(),
            reason: "The game has no title.".to_string(),
        });
    }

    let mut needs_ludex_scheme = false;
    let cmd = if let Some(uri) = crate::util::non_empty(game.launch_uri.as_deref()) {
        uri.to_string()
    } else if let Some(executable) = crate::util::non_empty(game.executable.as_deref()) {
        let raw = Path::new(executable);
        let full = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            Path::new(&game.install_path).join(raw)
        };
        quote_path(&full)
    } else {
        needs_ludex_scheme = true;
        format!("{LUDEX_URI_PREFIX}{}", game.game_id)
    };

    Ok(PublishedApp {
        game_id: game.game_id,
        title: title.to_string(),
        name: title.to_string(),
        cmd,
        working_dir: game.install_path.clone(),
        image_path,
        needs_ludex_scheme,
    })
}

fn generated_app(app: &PublishedApp, server_id: &str) -> Value {
    let mut map = Map::new();
    map.insert("name".to_string(), Value::String(app.name.clone()));
    map.insert("cmd".to_string(), Value::String(app.cmd.clone()));
    map.insert(
        "working-dir".to_string(),
        Value::String(app.working_dir.clone()),
    );
    if let Some(image) = app
        .image_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        map.insert("image-path".to_string(), Value::String(image.to_string()));
    }
    map.insert("detached".to_string(), Value::String("true".to_string()));
    map.insert(
        MANAGED_KEY.to_string(),
        json!({
            "managed": true,
            "version": MANAGED_VERSION,
            "gameId": app.game_id,
            "serverId": server_id,
        }),
    );
    Value::Object(map)
}

fn app_matches(existing: &Value, generated: &Value) -> bool {
    const FIELDS: [&str; 6] = [
        "name",
        "cmd",
        "working-dir",
        "image-path",
        "detached",
        MANAGED_KEY,
    ];
    FIELDS
        .iter()
        .all(|field| existing.get(field) == generated.get(field))
}

fn same_server(existing: &Value, server_id: &str) -> bool {
    match managed_server(existing) {
        Some(found) => found == server_id,
        // Entries written before per-server markers are adopted by the active
        // server so the first sync after an upgrade can refresh them.
        None => true,
    }
}

/// Merge the desired entries into Sunshine's app list.
///
/// - Non-managed apps are never modified or removed.
/// - Managed entries for another server are left alone.
/// - Re-running with the same input returns `changed: false` and counts them
///   as unchanged, so a second sync does not rewrite the file.
pub fn merge_apps(
    existing: Option<&str>,
    desired: &[PublishedApp],
    server_id: &str,
    remove_missing: bool,
) -> Result<(Value, MergeReport), String> {
    let mut root = match existing.map(str::trim).filter(|text| !text.is_empty()) {
        Some(text) => parse_apps_json(text)?,
        None => json!({ "env": {}, "apps": [] }),
    };

    let mut report = MergeReport::default();

    {
        let apps = apps_mut(&mut root)?;

        let mut existing_index: BTreeMap<i64, usize> = BTreeMap::new();
        for (index, app) in apps.iter().enumerate() {
            if !is_managed_app(app) || !same_server(app, server_id) {
                continue;
            }
            if let Some(game_id) = managed_game_id(app) {
                existing_index.entry(game_id).or_insert(index);
            }
        }

        for app in desired {
            let generated = generated_app(app, server_id);
            match existing_index.get(&app.game_id).copied() {
                Some(index) => {
                    if app_matches(&apps[index], &generated) {
                        report.unchanged += 1;
                    } else {
                        apps[index] = generated;
                        report.updated += 1;
                    }
                }
                None => {
                    apps.push(generated);
                    report.added += 1;
                }
            }
        }

        if remove_missing {
            let keep: Vec<i64> = desired.iter().map(|app| app.game_id).collect();
            let mut kept = Vec::with_capacity(apps.len());
            for app in apps.drain(..) {
                if is_managed_app(&app)
                    && same_server(&app, server_id)
                    && managed_game_id(&app)
                        .map(|game_id| !keep.contains(&game_id))
                        .unwrap_or(true)
                {
                    report.removed += 1;
                } else {
                    kept.push(app);
                }
            }
            *apps = kept;
        }

        report.managed_entries = apps.iter().filter(|app| is_managed_app(app)).count();
    }

    report.changed = report.added + report.updated + report.removed > 0;
    Ok((root, report))
}

/// Remove every Ludex-managed entry, whichever server wrote it.
pub fn remove_managed_apps(existing: &str) -> Result<(Value, usize), String> {
    let mut root = parse_apps_json(existing)?;
    let mut removed = 0;
    {
        let apps = apps_mut(&mut root)?;
        let mut kept = Vec::with_capacity(apps.len());
        for app in apps.drain(..) {
            if is_managed_app(&app) {
                removed += 1;
            } else {
                kept.push(app);
            }
        }
        *apps = kept;
    }
    Ok((root, removed))
}

/// Per-entry preview: what a sync would add, update and remove.
pub fn preview_apps(
    existing: Option<&str>,
    desired: &[PublishedApp],
    server_id: &str,
) -> Result<(Vec<PreviewEntry>, Vec<PreviewRemoval>), String> {
    let root = match existing.map(str::trim).filter(|text| !text.is_empty()) {
        Some(text) => parse_apps_json(text)?,
        None => json!({ "env": {}, "apps": [] }),
    };
    let apps = apps_ref(&root)?.clone();

    let mut entries = Vec::with_capacity(desired.len());
    for app in desired {
        let generated = generated_app(app, server_id);
        let action = apps
            .iter()
            .find(|existing| {
                is_managed_app(existing)
                    && same_server(existing, server_id)
                    && managed_game_id(existing) == Some(app.game_id)
            })
            .map(|existing| {
                if app_matches(existing, &generated) {
                    "unchanged"
                } else {
                    "update"
                }
            })
            .unwrap_or("add");
        entries.push(PreviewEntry {
            game_id: app.game_id,
            title: app.title.clone(),
            name: app.name.clone(),
            cmd: app.cmd.clone(),
            working_dir: app.working_dir.clone(),
            image_path: app.image_path.clone(),
            action: action.to_string(),
            needs_ludex_scheme: app.needs_ludex_scheme,
        });
    }

    let keep: Vec<i64> = desired.iter().map(|app| app.game_id).collect();
    let removals = apps
        .iter()
        .filter(|app| {
            is_managed_app(app)
                && same_server(app, server_id)
                && managed_game_id(app)
                    .map(|game_id| !keep.contains(&game_id))
                    .unwrap_or(true)
        })
        .map(|app| PreviewRemoval {
            name: as_string(app.get("name")).unwrap_or_else(|| "Ludex entry".to_string()),
            game_id: managed_game_id(app),
        })
        .collect();

    Ok((entries, removals))
}

/// Serialize an apps database for writing.
pub fn render_apps(value: &Value) -> Result<String, String> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|error| format!("Could not serialize Sunshine's apps.json: {error}"))?;
    text.push('\n');
    Ok(text)
}

/// Backup file path next to `apps.json` (`apps.json.ludex-backup-….bak`).
pub fn backup_path(apps_path: &Path) -> PathBuf {
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let suffix = crate::util::short_id(&uuid::Uuid::new_v4().to_string());
    apps_path.with_file_name(format!("apps.json.ludex-backup-{stamp}-{suffix}.bak"))
}

/// Copy the current `apps.json` before touching it. A failure aborts the write.
pub fn backup_apps(apps_path: &Path) -> Result<PathBuf, String> {
    let backup = backup_path(apps_path);
    std::fs::copy(apps_path, &backup).map_err(|error| {
        format!(
            "Could not back up {} before writing: {error}. Nothing was changed.",
            apps_path.display()
        )
    })?;
    Ok(backup)
}

/// Backup then atomically replace `apps.json` (temp file in the same folder,
/// fsync, rename). Returns the backup path, if the file already existed.
pub fn write_apps_with_backup(apps_path: &Path, content: &str) -> Result<Option<PathBuf>, String> {
    if let Some(parent) = apps_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Could not create the Sunshine folder {}: {error}",
                parent.display()
            )
        })?;
    }

    let backup = if apps_path.is_file() {
        Some(backup_apps(apps_path)?)
    } else {
        None
    };

    let temporary = apps_path.with_file_name("apps.json.ludex.tmp");
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary).map_err(|error| {
            format!(
                "Could not write {}: {error}. The previous file is untouched.",
                temporary.display()
            )
        })?;
        file.write_all(content.as_bytes()).map_err(|error| {
            format!(
                "Could not write {}: {error}. The previous file is untouched.",
                temporary.display()
            )
        })?;
        file.sync_all()
            .map_err(|error| format!("Could not flush the new apps.json: {error}"))?;
    }

    if let Err(first_error) = std::fs::rename(&temporary, apps_path) {
        if apps_path.exists() {
            std::fs::remove_file(apps_path).map_err(|error| {
                format!(
                    "Could not replace {} (rename failed: {first_error}; remove failed: {error}). The backup is kept.",
                    apps_path.display()
                )
            })?;
            std::fs::rename(&temporary, apps_path).map_err(|error| {
                format!(
                    "Could not replace {}: {error}. The backup is kept.",
                    apps_path.display()
                )
            })?;
        } else {
            return Err(format!(
                "Could not write {}: {first_error}",
                apps_path.display()
            ));
        }
    }

    Ok(backup)
}

/// `sunshine_status` payload for the settings panel.
pub fn sunshine_status(settings: &LocalSettings) -> SunshineStatus {
    let detection = detect_sunshine(settings, platform_name(), home_dir().as_deref(), &env_map());
    let apps_path = detection.paths.as_ref().map(|paths| paths.apps_json.clone());
    let apps_found = apps_path
        .as_deref()
        .map(|path| Path::new(path).is_file())
        .unwrap_or(false);

    let mut warnings = detection.warnings.clone();
    let mut entries = 0;
    let mut managed_entries = 0;
    if let Some(path) = apps_path.as_deref().filter(|_| apps_found) {
        match std::fs::read_to_string(path) {
            Ok(text) => match parse_apps_json(&text) {
                Ok(value) => match apps_ref(&value) {
                    Ok(apps) => {
                        entries = apps.len();
                        managed_entries = apps.iter().filter(|app| is_managed_app(app)).count();
                    }
                    Err(_) => warnings.push(
                        "Sunshine's apps.json has no app list yet; the first sync creates one."
                            .to_string(),
                    ),
                },
                Err(error) => warnings.push(error),
            },
            Err(error) => warnings.push(format!("Could not read {path}: {error}")),
        }
    }

    let detected = detection.paths.is_some() || detection.binary.is_some();
    let message = if !detected {
        "Sunshine was not found on this machine. Install Sunshine to publish the Ludex library as streaming apps."
            .to_string()
    } else if apps_path.is_none() {
        "Sunshine is installed but its config folder was not found. Set the folder manually in Settings → Streaming."
            .to_string()
    } else if managed_entries == 0 {
        "No Ludex game is published yet. Publish the library to create the first entries."
            .to_string()
    } else {
        format!("{managed_entries} Ludex app(s) published to Sunshine.")
    };

    SunshineStatus {
        detected,
        platform: detection.platform,
        apps_path,
        conf_path: detection.paths.as_ref().map(|paths| paths.sunshine_conf.clone()),
        binary: detection.binary,
        source: detection.paths.as_ref().map(|paths| paths.source.clone()),
        overridden: detection
            .paths
            .as_ref()
            .map(|paths| paths.overridden)
            .unwrap_or(false),
        apps_found,
        entries,
        managed_entries,
        warnings,
        message,
    }
}

/// Read `apps.json` for the given status, if any.
pub fn read_apps_text(status: &SunshineStatus) -> Result<Option<String>, String> {
    let Some(path) = status.apps_path.as_deref() else {
        return Ok(None);
    };
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Could not read {path}: {error}")),
    }
}

/* ------------------------------- Moonlight -------------------------------- */

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoonlightHost {
    pub name: String,
    pub address: String,
    pub local_address: Option<String>,
    pub remote_address: Option<String>,
    pub manual_address: Option<String>,
    pub paired: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoonlightStatus {
    pub detected: bool,
    pub platform: String,
    pub binary: Option<String>,
    pub config_path: Option<String>,
    pub config_found: bool,
    pub hosts: Vec<MoonlightHost>,
    pub supported: bool,
    pub warnings: Vec<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamLaunch {
    pub launched: bool,
    pub host: String,
    pub app: String,
    pub command: Vec<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamTarget {
    pub published: bool,
    pub app_name: Option<String>,
    pub game_id: i64,
    pub hosts: Vec<MoonlightHost>,
    pub moonlight: MoonlightStatus,
    pub sunshine: SunshineStatus,
    pub message: String,
}

#[derive(Debug, Default, Clone)]
struct HostEntry {
    address: Option<String>,
    local_address: Option<String>,
    remote_address: Option<String>,
    manual_address: Option<String>,
    custom_name: Option<String>,
    certificate: bool,
    paired: Option<bool>,
}

fn strip_quotes(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

fn normalized_field(segment: &str) -> String {
    segment
        .chars()
        .filter(|character| !matches!(character, '_' | '-' | ' '))
        .flat_map(char::to_lowercase)
        .collect()
}

fn host_from_key<'a>(segments: &[&'a str], section: &'a str) -> Option<(String, String)> {
    if segments.is_empty() {
        return None;
    }
    let field = normalized_field(segments[segments.len() - 1]);
    if segments.len() >= 2 {
        let host = segments[..segments.len() - 1].join("\\");
        return Some((host, field));
    }
    let ignorable = section.is_empty()
        || section.eq_ignore_ascii_case("general")
        || section.eq_ignore_ascii_case("hosts")
        || section.eq_ignore_ascii_case("host");
    if ignorable {
        None
    } else {
        Some((section.to_string(), field))
    }
}

/// Parse Moonlight's Qt `Moonlight.conf` host list.
///
/// `QSettings` writes host fields as `host\address=…` (sometimes as
/// `[hosts]` keys or nested sections, depending on the Moonlight build), so the
/// parser accepts `/` and `\` separators plus a per-host section.
pub fn parse_moonlight_conf(text: &str) -> Vec<MoonlightHost> {
    let mut entries: BTreeMap<String, HostEntry> = BTreeMap::new();
    let mut section = String::new();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = strip_quotes(value);
        let key = key.trim();
        if key.is_empty() || value.is_empty() {
            continue;
        }
        let segments: Vec<&str> = key
            .split(['/', '\\'])
            .map(str::trim)
            .filter(|segment| !segment.is_empty())
            .collect();
        let Some((host, field)) = host_from_key(&segments, &section) else {
            continue;
        };
        let entry = entries.entry(host).or_default();
        match field.as_str() {
            "address" => entry.address = Some(value),
            "localaddress" => entry.local_address = Some(value),
            "remoteaddress" => entry.remote_address = Some(value),
            "manualaddress" => entry.manual_address = Some(value),
            "customname" | "name" => entry.custom_name = Some(value),
            "srvcert" | "servercert" => entry.certificate = true,
            "paired" | "pairstatus" => {
                entry.paired = Some(value.eq_ignore_ascii_case("true") || value == "1");
            }
            _ => {}
        }
    }

    let mut hosts: Vec<MoonlightHost> = entries
        .into_iter()
        .filter_map(|(key, entry)| {
            let address = entry
                .manual_address
                .clone()
                .or_else(|| entry.address.clone())
                .or_else(|| entry.local_address.clone())
                .or_else(|| entry.remote_address.clone())?;
            Some(MoonlightHost {
                name: entry.custom_name.unwrap_or(key),
                address,
                local_address: entry.local_address,
                remote_address: entry.remote_address,
                manual_address: entry.manual_address,
                paired: entry.certificate || entry.paired.unwrap_or(false),
            })
        })
        .collect();
    hosts.sort_by_key(|host| host.name.to_lowercase());
    hosts
}

/// Moonlight config candidates. macOS preferences are a plist, which Ludex does
/// not parse: the host list is then reported as unsupported.
pub fn moonlight_conf_candidates(
    platform: &str,
    home: Option<&Path>,
    env: &BTreeMap<String, String>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    match platform {
        "windows" => {
            if let Some(root) = env.get("LOCALAPPDATA") {
                out.push(
                    Path::new(root)
                        .join("Moonlight Game Streaming Project")
                        .join("Moonlight.conf"),
                );
            }
            if let Some(root) = env.get("APPDATA") {
                out.push(
                    Path::new(root)
                        .join("Moonlight Game Streaming Project")
                        .join("Moonlight.conf"),
                );
            }
        }
        "macos" => {
            if let Some(home) = home {
                out.push(
                    home.join("Library")
                        .join("Preferences")
                        .join("com.moonlight-stream.Moonlight.plist"),
                );
            }
        }
        _ => {
            if let Some(home) = home {
                out.push(
                    home.join(".config")
                        .join("Moonlight Game Streaming Project")
                        .join("Moonlight.conf"),
                );
                out.push(
                    home.join(".var")
                        .join("app")
                        .join("com.moonlight_stream.Moonlight")
                        .join("config")
                        .join("Moonlight Game Streaming Project")
                        .join("Moonlight.conf"),
                );
            }
        }
    }
    out
}

fn moonlight_binary(platform: &str, home: Option<&Path>, env: &BTreeMap<String, String>) -> Option<String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    match platform {
        "windows" => {
            if let Some(root) = env.get("ProgramFiles") {
                candidates.push(
                    Path::new(root)
                        .join("Moonlight Game Streaming")
                        .join("Moonlight.exe"),
                );
                candidates.push(Path::new(root).join("Moonlight").join("Moonlight.exe"));
            }
            if let Some(root) = env.get("LOCALAPPDATA") {
                candidates.push(
                    Path::new(root)
                        .join("Moonlight Game Streaming")
                        .join("Moonlight.exe"),
                );
            }
        }
        "macos" => {
            if let Some(home) = home {
                candidates.push(
                    home.join("Applications")
                        .join("Moonlight.app")
                        .join("Contents")
                        .join("MacOS")
                        .join("Moonlight"),
                );
            }
        }
        _ => {
            candidates.push(PathBuf::from("/usr/bin/moonlight"));
            candidates.push(PathBuf::from("/usr/local/bin/moonlight"));
            if let Some(home) = home {
                candidates.push(
                    home.join(".local")
                        .join("share")
                        .join("flatpak")
                        .join("exports")
                        .join("bin")
                        .join("com.moonlight_stream.Moonlight"),
                );
            }
            candidates.push(PathBuf::from(
                "/var/lib/flatpak/exports/bin/com.moonlight_stream.Moonlight",
            ));
        }
    }
    if let Some(path) = candidates.into_iter().find(|path| path.is_file()) {
        return Some(path.to_string_lossy().to_string());
    }
    crate::paths::find_on_path("moonlight")
        .or_else(|| crate::paths::find_on_path("moonlight-qt"))
        .map(|path| path.to_string_lossy().to_string())
}

/// Detect Moonlight and parse its host list.
pub fn moonlight_status(settings: &LocalSettings) -> MoonlightStatus {
    let platform = platform_name();
    let home = home_dir();
    let env = env_map();
    let mut warnings = Vec::new();

    let binary = if !settings.moonlight_binary.trim().is_empty() {
        let configured = PathBuf::from(settings.moonlight_binary.trim());
        if configured.is_file() {
            Some(configured.to_string_lossy().to_string())
        } else {
            warnings.push(format!(
                "The configured Moonlight binary does not exist: {}",
                configured.display()
            ));
            None
        }
    } else {
        moonlight_binary(platform, home.as_deref(), &env)
    };

    let candidates = moonlight_conf_candidates(platform, home.as_deref(), &env);
    let (config_path, config_found) = if !settings.moonlight_config_path.trim().is_empty() {
        let configured = PathBuf::from(settings.moonlight_config_path.trim());
        let found = configured.is_file();
        warnings.push(if found {
            "The configured Moonlight config file is used.".to_string()
        } else {
            format!(
                "The configured Moonlight config file does not exist: {}",
                configured.display()
            )
        });
        (Some(configured), found)
    } else {
        match candidates.iter().find(|path| path.is_file()) {
            Some(path) => (Some(path.clone()), true),
            None => (candidates.first().cloned(), false),
        }
    };

    let supported = platform != "macos";
    let mut hosts = Vec::new();
    let mut message;
    if !supported {
        warnings.push(
            "Moonlight stores its hosts in a macOS preferences plist, which Ludex does not read. Use Moonlight directly."
                .to_string(),
        );
        message = "Moonlight host auto-detection is not available on macOS.".to_string();
    } else if let Some(path) = config_path.as_deref().filter(|_| config_found) {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                hosts = parse_moonlight_conf(&text);
                message = if hosts.is_empty() {
                    "Moonlight is configured but no host is saved yet. Pair a Sunshine host in Moonlight first."
                        .to_string()
                } else {
                    format!("{} host(s) found in Moonlight.", hosts.len())
                };
            }
            Err(error) => {
                warnings.push(format!("Could not read {}: {error}", path.display()));
                message = "Moonlight's host list could not be read.".to_string();
            }
        }
    } else {
        message =
            "Moonlight's config file was not found. Pair a Sunshine host in Moonlight first."
                .to_string();
    }

    let detected = binary.is_some();
    if !detected {
        warnings.push(
            "Moonlight was not found. Install Moonlight (moonlight-qt) to stream from a Sunshine host."
                .to_string(),
        );
        message =
            "Moonlight was not found on this machine. Install it to stream from a Sunshine host."
                .to_string();
    }

    MoonlightStatus {
        detected,
        platform: platform.to_string(),
        binary,
        config_path: config_path.map(|path| path.to_string_lossy().to_string()),
        config_found,
        hosts,
        supported,
        warnings,
        message,
    }
}

/// `moonlight stream <host> <app>`.
pub fn build_stream_args(host: &str, app: &str) -> Result<Vec<String>, String> {
    let host = host.trim();
    let app = app.trim();
    if host.is_empty() {
        return Err("Choose a Moonlight host first.".to_string());
    }
    if app.is_empty() {
        return Err("Choose a game to stream first.".to_string());
    }
    if host.chars().any(char::is_control) || app.chars().any(char::is_control) {
        return Err("Host and app names must not contain control characters.".to_string());
    }
    Ok(vec![
        "stream".to_string(),
        host.to_string(),
        app.to_string(),
    ])
}

/// Start Moonlight for one host/app pair, detached from Ludex.
pub fn spawn_stream(binary: &str, host: &str, app: &str) -> Result<StreamLaunch, String> {
    let args = build_stream_args(host, app)?;
    let mut command = Command::new(binary);
    command
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = command
        .spawn()
        .map_err(|error| format!("Could not start Moonlight at {binary}: {error}"))?;
    // Moonlight owns its process; Ludex only needs the start to succeed.
    drop(child);
    let command_line = std::iter::once(binary.to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<String>>();
    Ok(StreamLaunch {
        launched: true,
        host: host.to_string(),
        app: app.to_string(),
        command: command_line,
        message: format!("Moonlight is starting the stream of {app} from {host}."),
    })
}

/// Pick the host for a stream: an explicit request first, then the only paired
/// host. An unpaired host is refused with an actionable message.
pub fn select_host(hosts: &[MoonlightHost], requested: Option<&str>) -> Result<MoonlightHost, String> {
    if hosts.is_empty() {
        return Err(
            "Moonlight has no saved host. Pair a Sunshine host in Moonlight first.".to_string(),
        );
    }
    let host = match requested.map(str::trim).filter(|value| !value.is_empty()) {
        Some(requested) => hosts
            .iter()
            .find(|host| host.name == requested || host.address == requested)
            .cloned()
            .ok_or_else(|| format!("Moonlight has no saved host named {requested}."))?,
        None if hosts.len() == 1 => hosts[0].clone(),
        None => {
            let names = hosts
                .iter()
                .map(|host| host.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "Choose a Moonlight host first ({names})."
            ));
        }
    };
    if !host.paired {
        return Err(format!(
            "{} is not paired in Moonlight. Pair it in Moonlight first.",
            host.name
        ));
    }
    Ok(host)
}

/// The Sunshine app name for a game, when it is published.
pub fn published_name(existing: Option<&str>, game_id: i64) -> Result<Option<String>, String> {
    let Some(text) = existing.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    let root = parse_apps_json(text)?;
    let apps = apps_ref(&root)?;
    Ok(apps
        .iter()
        .filter(|app| is_managed_app(app))
        .find(|app| managed_game_id(app) == Some(game_id))
        .and_then(|app| as_string(app.get("name"))))
}

/// Resolve the local cover cache file for a game, if one exists.
pub fn cached_cover(dir: &Path, game_id: i64) -> Option<String> {
    ["jpg", "jpeg", "png", "webp", "gif", "avif"]
        .iter()
        .map(|extension| dir.join(format!("{game_id}.{extension}")))
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().to_string())
}

/// Store downloaded cover bytes in the local cache, returning the file path.
pub fn store_cover(dir: &Path, game_id: i64, bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() || bytes.len() > 8 * 1024 * 1024 {
        return None;
    }
    let extension = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "jpg"
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "webp"
    } else if bytes.starts_with(b"GIF8") {
        "gif"
    } else {
        return None;
    };
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(format!("{game_id}.{extension}"));
    let temporary = dir.join(format!("{game_id}.{extension}.tmp"));
    std::fs::write(&temporary, bytes).ok()?;
    if std::fs::rename(&temporary, &path).is_err() {
        let _ = std::fs::remove_file(&temporary);
        return None;
    }
    Some(path.to_string_lossy().to_string())
}

/// Fetch and cache a cover, bounded by a 5 s timeout and an 8 MiB cap.
pub async fn fetch_cover(
    http: &reqwest::Client,
    base: &str,
    url: &str,
    dir: &Path,
    game_id: i64,
) -> Option<String> {
    if let Some(cached) = cached_cover(dir, game_id) {
        return Some(cached);
    }
    let resolved = if let Some(rest) = url.strip_prefix('/') {
        format!("{}/{}", base.trim_end_matches('/'), rest)
    } else if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        return None;
    };
    let response = http
        .get(resolved)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = response.bytes().await.ok()?;
    store_cover(dir, game_id, &bytes)
}

/// Map of `gameId → coverUrl` from the server library, one request per 100
/// games. A failure is an empty map; covers are optional.
pub async fn fetch_cover_index(
    http: &reqwest::Client,
    base: &str,
    token: Option<&str>,
) -> BTreeMap<i64, String> {
    let mut covers = BTreeMap::new();
    for page in 1..=20 {
        let mut request = http
            .get(format!(
                "{}/api/games?limit=100&page={page}",
                base.trim_end_matches('/')
            ))
            .timeout(Duration::from_secs(8));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let Ok(response) = request.send().await else {
            break;
        };
        if !response.status().is_success() {
            break;
        }
        let Ok(value) = response.json::<Value>().await else {
            break;
        };
        let items = value
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let count = items.len();
        for item in items {
            let Some(id) = item.get("id").and_then(Value::as_i64) else {
                continue;
            };
            if let Some(url) = as_string(item.get("coverUrl")) {
                covers.insert(id, url);
            }
        }
        if count < 100 {
            break;
        }
    }
    covers
}

/* --------------------------------- tests ---------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;

    fn game(id: i64, title: &str, executable: Option<&str>) -> InstalledGame {
        InstalledGame {
            game_id: id,
            version_id: 1,
            title: title.to_string(),
            version: Some("1.0".to_string()),
            install_path: "/games/celeste".to_string(),
            executable: executable.map(str::to_string),
            size_bytes: 0,
            installed_at: "2026-11-11T00:00:00Z".to_string(),
            last_launched_at: None,
            archive_hash: None,
            install_root: Some("/games".to_string()),
            source: "ludex".to_string(),
            launch_uri: None,
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-streaming-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const USER_APPS: &str = r#"{
      "env": {"PATH": "/usr/bin"},
      "apps": [
        {"name": "Desktop", "cmd": "gnome-session", "detached": "false"},
        {"name": "Steam Big Picture", "cmd": "steam://open/bigpicture"}
      ]
    }"#;

    #[test]
    fn merge_preserves_user_apps_and_adds_ludex_entries() {
        let apps = vec![
            publish_app(&game(1, "Celeste", Some("Celeste.exe")), None).unwrap(),
            publish_app(&game(2, "Hades", None), Some("/art/2.jpg".to_string())).unwrap(),
        ];
        let (root, report) = merge_apps(Some(USER_APPS), &apps, "srv_1", true).unwrap();

        assert_eq!(report.added, 2);
        assert_eq!(report.updated, 0);
        assert_eq!(report.removed, 0);
        assert!(report.changed);

        let list = apps_ref(&root).unwrap();
        assert_eq!(list.len(), 4);
        assert_eq!(as_string(list[0].get("name")).as_deref(), Some("Desktop"));
        assert!(!is_managed_app(&list[0]));
        let ludex = list.iter().find(|app| managed_game_id(app) == Some(1)).unwrap();
        assert_eq!(
            as_string(ludex.get("cmd")).as_deref(),
            Some("/games/celeste/Celeste.exe")
        );
        assert_eq!(as_string(ludex.get("detached")).as_deref(), Some("true"));
        assert!(is_managed_app(
            list.iter().find(|app| managed_game_id(app) == Some(2)).unwrap()
        ));
        // The user's top-level keys survive.
        assert_eq!(
            root.get("env").and_then(|env| env.get("PATH")),
            Some(&Value::String("/usr/bin".to_string()))
        );
    }

    #[test]
    fn second_sync_is_a_no_op() {
        let apps = vec![publish_app(&game(1, "Celeste", Some("Celeste.exe")), None).unwrap()];
        let (root, first) = merge_apps(Some(USER_APPS), &apps, "srv_1", true).unwrap();
        assert!(first.changed);
        let text = render_apps(&root).unwrap();

        let (_, second) = merge_apps(Some(&text), &apps, "srv_1", true).unwrap();
        assert!(!second.changed);
        assert_eq!(second.added, 0);
        assert_eq!(second.updated, 0);
        assert_eq!(second.removed, 0);
        assert_eq!(second.unchanged, 1);
        assert_eq!(second.managed_entries, 1);
    }

    #[test]
    fn merge_updates_changed_fields_in_place() {
        let mut installed = game(1, "Celeste", Some("Celeste.exe"));
        let apps = vec![publish_app(&installed, None).unwrap()];
        let (root, _) = merge_apps(Some(USER_APPS), &apps, "srv_1", true).unwrap();
        let text = render_apps(&root).unwrap();

        installed.title = "Celeste (Deluxe)".to_string();
        let apps = vec![publish_app(&installed, None).unwrap()];
        let (root, report) = merge_apps(Some(&text), &apps, "srv_1", true).unwrap();
        assert_eq!(report.updated, 1);
        assert_eq!(report.added, 0);
        let list = apps_ref(&root).unwrap();
        let entry = list.iter().find(|app| managed_game_id(app) == Some(1)).unwrap();
        assert_eq!(
            as_string(entry.get("name")).as_deref(),
            Some("Celeste (Deluxe)")
        );
        // Still exactly one Ludex entry for the game.
        assert_eq!(
            list.iter()
                .filter(|app| managed_game_id(app) == Some(1))
                .count(),
            1
        );
    }

    #[test]
    fn removals_only_touch_managed_entries() {
        let apps = vec![
            publish_app(&game(1, "One", Some("one.exe")), None).unwrap(),
            publish_app(&game(2, "Two", Some("two.exe")), None).unwrap(),
        ];
        let (root, _) = merge_apps(Some(USER_APPS), &apps, "srv_1", true).unwrap();
        let text = render_apps(&root).unwrap();

        let apps = vec![publish_app(&game(1, "One", Some("one.exe")), None).unwrap()];
        let (root, report) = merge_apps(Some(&text), &apps, "srv_1", true).unwrap();
        assert_eq!(report.removed, 1);
        let list = apps_ref(&root).unwrap();
        assert_eq!(list.len(), 3);
        assert!(list.iter().any(|app| as_string(app.get("name")).as_deref() == Some("Desktop")));
        assert!(!list.iter().any(|app| managed_game_id(app) == Some(2)));
    }

    #[test]
    fn managed_entries_from_another_server_survive() {
        let apps = vec![publish_app(&game(1, "One", Some("one.exe")), None).unwrap()];
        let (root, _) = merge_apps(Some(USER_APPS), &apps, "server-a", true).unwrap();
        let text = render_apps(&root).unwrap();

        let other = vec![publish_app(&game(9, "Nine", Some("nine.exe")), None).unwrap()];
        let (root, report) = merge_apps(Some(&text), &other, "server-b", true).unwrap();
        assert_eq!(report.removed, 0);
        let list = apps_ref(&root).unwrap();
        assert!(list.iter().any(|app| managed_game_id(app) == Some(1)));
        assert!(list.iter().any(|app| managed_game_id(app) == Some(9)));
        assert_eq!(report.managed_entries, 2);
    }

    #[test]
    fn remove_managed_clears_every_server() {
        let mut apps = vec![publish_app(&game(1, "One", Some("one.exe")), None).unwrap()];
        let (root, _) = merge_apps(Some(USER_APPS), &apps, "server-a", true).unwrap();
        apps = vec![publish_app(&game(2, "Two", Some("two.exe")), None).unwrap()];
        let (root, _) = merge_apps(Some(&render_apps(&root).unwrap()), &apps, "server-b", true).unwrap();

        let (root, removed) = remove_managed_apps(&render_apps(&root).unwrap()).unwrap();
        assert_eq!(removed, 2);
        let list = apps_ref(&root).unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|app| !is_managed_app(app)));
    }

    #[test]
    fn legacy_ludex_uri_entries_are_adopted_and_refreshed() {
        let legacy = r#"{"apps":[{"name":"Old","cmd":"ludex://play/4"}]}"#;
        let apps = vec![publish_app(&game(4, "New", Some("new.exe")), None).unwrap()];
        let (root, report) = merge_apps(Some(legacy), &apps, "srv_1", true).unwrap();
        assert_eq!(report.updated, 1);
        let list = apps_ref(&root).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].get("name"), Some(&Value::String("New".to_string())));
    }

    #[test]
    fn invalid_json_is_never_rewritten() {
        let dir = temp_dir("invalid");
        let path = dir.join("apps.json");
        std::fs::write(&path, "{not json").unwrap();

        let merged = merge_apps(Some("{not json"), &[], "srv_1", true);
        assert!(merged.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_creates_backup_with_previous_content() {
        let dir = temp_dir("backup");
        let path = dir.join("apps.json");
        std::fs::write(&path, USER_APPS).unwrap();

        let backup = write_apps_with_backup(&path, "{\"apps\":[]}\n")
            .unwrap()
            .expect("backup path");
        assert!(backup.is_file());
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), USER_APPS);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"apps\":[]}\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_backup_is_reported_and_aborts_the_write() {
        let dir = temp_dir("backup-fails");
        let path = dir.join("apps.json");
        std::fs::create_dir_all(&path).unwrap();

        let error = backup_apps(&path).unwrap_err();
        assert!(error.contains("back up"));
        // Writing over a directory also fails; nothing is replaced.
        assert!(write_apps_with_backup(&path, "{}").is_err());
        assert!(path.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sunshine_candidates_follow_the_platform() {
        let mut env = BTreeMap::new();
        env.insert("ProgramFiles".to_string(), "C:\\Program Files".to_string());
        env.insert("LOCALAPPDATA".to_string(), "C:\\Users\\me\\AppData\\Local".to_string());
        let windows = sunshine_candidates("windows", None, &env);
        assert!(windows[0].apps_json.contains("Sunshine"));
        assert!(windows[0].apps_json.ends_with("apps.json"));
        assert!(windows.iter().any(|candidate| candidate.source == "Local AppData"));

        let home = Path::new("/home/me");
        let linux = sunshine_candidates("linux", Some(home), &BTreeMap::new());
        assert_eq!(
            linux[0].apps_json,
            "/home/me/.config/sunshine/apps.json"
        );
        assert!(linux.iter().any(|candidate| candidate.source == "Flatpak"));

        let mac = sunshine_candidates("macos", Some(home), &BTreeMap::new());
        assert!(mac[0].apps_json.ends_with(".config/sunshine/apps.json"));
    }

    #[test]
    fn detection_uses_an_existing_fixture_and_overrides() {
        let dir = temp_dir("detect");
        std::fs::write(dir.join("sunshine.conf"), "port = 47989\n").unwrap();
        let mut settings = LocalSettings {
            sunshine_apps_path: dir.join("apps.json").to_string_lossy().to_string(),
            sunshine_conf_path: dir.join("sunshine.conf").to_string_lossy().to_string(),
            ..LocalSettings::default()
        };

        let detection = detect_sunshine(&settings, "linux", None, &BTreeMap::new());
        let paths = detection.paths.expect("paths");
        assert!(paths.overridden);
        assert_eq!(paths.source, "override");
        assert!(paths.apps_json.ends_with("apps.json"));

        settings.sunshine_apps_path = dir.join("missing").join("apps.json").to_string_lossy().to_string();
        let detection = detect_sunshine(&settings, "linux", None, &BTreeMap::new());
        assert!(detection.paths.is_none());
        assert!(detection.warnings.iter().any(|warning| warning.contains("does not exist")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detection_finds_a_flatpak_fixture() {
        let root = temp_dir("flatpak");
        let home = root.join("home");
        let sunshine = home
            .join(".var")
            .join("app")
            .join("dev.lizardbyte.app.Sunshine")
            .join("config")
            .join("sunshine");
        std::fs::create_dir_all(&sunshine).unwrap();
        std::fs::write(sunshine.join("apps.json"), "{\"apps\":[]}").unwrap();

        let settings = LocalSettings::default();
        let detection = detect_sunshine(&settings, "linux", Some(&home), &BTreeMap::new());
        let paths = detection.paths.expect("flatpak paths");
        assert_eq!(paths.source, "Flatpak");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn published_app_falls_back_to_the_ludex_uri() {
        let app = publish_app(&game(7, "No Exe", None), None).unwrap();
        assert_eq!(app.cmd, "ludex://play/7");
        assert!(app.needs_ludex_scheme);
    }

    #[test]
    fn steam_imports_publish_their_launch_uri() {
        let mut installed = game(3, "Steam Game", None);
        installed.launch_uri = Some("steam://rungameid/123".to_string());
        installed.source = "steam".to_string();
        let app = publish_app(&installed, None).unwrap();
        assert_eq!(app.cmd, "steam://rungameid/123");
        assert!(!app.needs_ludex_scheme);
    }

    #[test]
    fn preview_reports_actions_without_writing() {
        let apps = vec![publish_app(&game(1, "One", Some("one.exe")), None).unwrap()];
        let (entries, removals) = preview_apps(Some(USER_APPS), &apps, "srv_1").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "add");
        assert!(removals.is_empty());

        let (root, _) = merge_apps(Some(USER_APPS), &apps, "srv_1", true).unwrap();
        let (entries, removals) = preview_apps(Some(&render_apps(&root).unwrap()), &apps, "srv_1").unwrap();
        assert_eq!(entries[0].action, "unchanged");
        assert!(removals.is_empty());
    }

    #[test]
    fn parse_accepts_the_legacy_array_shape() {
        let value = parse_apps_json(r#"[{"name":"Desktop"}]"#).unwrap();
        assert!(value.is_array());
        let (root, report) = merge_apps(
            Some(r#"[{"name":"Desktop"}]"#),
            &[publish_app(&game(1, "One", Some("one.exe")), None).unwrap()],
            "srv_1",
            true,
        )
        .unwrap();
        assert!(root.is_array());
        assert_eq!(report.added, 1);
    }

    #[test]
    fn moonlight_conf_parses_qsettings_hosts() {
        let text = r#"
[General]
locale=en

[hosts]
Living-room\address=192.168.1.20
Living-room\localaddress=192.168.1.20
Living-room\remoteaddress=203.0.113.9
Living-room\customname=Living Room
Living-room\srvcert=abc123
Bedroom/address=192.168.1.30
Bedroom/manualaddress=10.0.0.30
Bedroom\pairstatus=0
"#;
        let hosts = parse_moonlight_conf(text);
        assert_eq!(hosts.len(), 2);
        let living = hosts.iter().find(|host| host.name == "Living Room").unwrap();
        assert_eq!(living.address, "192.168.1.20");
        assert!(living.paired);
        let bedroom = hosts.iter().find(|host| host.name == "Bedroom").unwrap();
        assert_eq!(bedroom.address, "10.0.0.30");
        assert!(!bedroom.paired);
    }

    #[test]
    fn moonlight_conf_accepts_per_host_sections_and_ignores_general() {
        let text = r#"
[General]
address=not-a-host
[Steam Deck]
address=192.168.1.40
customname=Deck
"#;
        let hosts = parse_moonlight_conf(text);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "Deck");
        assert_eq!(hosts[0].address, "192.168.1.40");
        assert!(!hosts[0].paired);
    }

    #[test]
    fn moonlight_stream_args_are_validated() {
        assert_eq!(
            build_stream_args("myhost", "Celeste").unwrap(),
            vec!["stream", "myhost", "Celeste"]
        );
        assert!(build_stream_args("  ", "Celeste").is_err());
        assert!(build_stream_args("myhost", " ").is_err());
        assert!(build_stream_args("my\nhost", "Celeste").is_err());
    }

    #[test]
    fn host_selection_prefers_the_request_and_refuses_unpaired() {
        let hosts = vec![
            MoonlightHost {
                name: "One".to_string(),
                address: "10.0.0.1".to_string(),
                local_address: None,
                remote_address: None,
                manual_address: None,
                paired: true,
            },
            MoonlightHost {
                name: "Two".to_string(),
                address: "10.0.0.2".to_string(),
                local_address: None,
                remote_address: None,
                manual_address: None,
                paired: false,
            },
        ];
        assert_eq!(select_host(&hosts, Some("Two")).unwrap_err(), "Two is not paired in Moonlight. Pair it in Moonlight first.");
        assert_eq!(select_host(&hosts, Some("One")).unwrap().name, "One");
        assert!(select_host(&hosts, None).unwrap_err().starts_with("Choose"));
        assert!(select_host(&[], None).is_err());
    }

    #[test]
    fn published_name_only_returns_managed_entries() {
        let apps = vec![publish_app(&game(1, "One", Some("one.exe")), None).unwrap()];
        let (root, _) = merge_apps(Some(USER_APPS), &apps, "srv_1", true).unwrap();
        let text = render_apps(&root).unwrap();
        assert_eq!(published_name(Some(&text), 1).unwrap().as_deref(), Some("One"));
        assert_eq!(published_name(Some(&text), 2).unwrap(), None);
        assert_eq!(published_name(None, 1).unwrap(), None);
    }

    #[test]
    fn cover_cache_is_keyed_by_game_id() {
        let dir = temp_dir("covers");
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let path = store_cover(&dir, 42, &png).expect("stored");
        assert!(path.ends_with("42.png"));
        assert_eq!(
            cached_cover(&dir, 42).as_deref(),
            Some(path.as_str())
        );
        assert!(store_cover(&dir, 43, b"not an image").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sunshine_status_reports_a_fixture() {
        let dir = temp_dir("status");
        std::fs::write(dir.join("sunshine.conf"), "").unwrap();
        std::fs::write(dir.join("apps.json"), USER_APPS).unwrap();
        let settings = LocalSettings {
            sunshine_apps_path: dir.join("apps.json").to_string_lossy().to_string(),
            sunshine_conf_path: dir.join("sunshine.conf").to_string_lossy().to_string(),
            ..LocalSettings::default()
        };

        let status = sunshine_status(&settings);
        assert!(status.detected);
        assert_eq!(status.entries, 2);
        assert_eq!(status.managed_entries, 0);
        assert!(!status.message.contains("not found"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn moonlight_candidates_follow_the_platform() {
        let mut env = BTreeMap::new();
        env.insert(
            "LOCALAPPDATA".to_string(),
            "C:\\Users\\me\\AppData\\Local".to_string(),
        );
        let windows = moonlight_conf_candidates("windows", None, &env);
        assert!(windows[0].ends_with("Moonlight Game Streaming Project/Moonlight.conf"));

        let home = Path::new("/home/me");
        let linux = moonlight_conf_candidates("linux", Some(home), &BTreeMap::new());
        assert_eq!(
            linux[0],
            home.join(".config/Moonlight Game Streaming Project/Moonlight.conf")
        );

        let mac = moonlight_conf_candidates("macos", Some(home), &BTreeMap::new());
        assert!(mac[0].ends_with("com.moonlight-stream.Moonlight.plist"));
    }
}
