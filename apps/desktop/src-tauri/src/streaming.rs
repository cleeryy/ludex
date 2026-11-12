//! Sunshine host integration (#97) and Moonlight client integration (#98).
//!
//! Both applications are external: Ludex never bundles, installs or updates
//! them. It only edits Sunshine's `apps.json` after an explicit action (with a
//! backup), and it launches the local Moonlight binary with the host and app the
//! user picked. Everything here is best-effort filesystem inspection; a missing
//! install is a clear status, never an error.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::db::InstalledGame;
use crate::settings::LocalSettings;

/// Marker object written inside every Ludex-managed Sunshine app entry.
pub const MANAGED_KEY: &str = "ludex";
/// Marker field that identifies the single console entry (#172).
pub const CONSOLE_MARKER: &str = "console";
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
    /// Windows denies writes to the detected `apps.json` because it lives in
    /// `Program Files`; a one-time elevation can fix it (#182).
    pub needs_elevation: bool,
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

/// The single managed Sunshine app that starts Ludex in console mode (#172).
///
/// It has no game id: it opens Big Picture, and games are launched from there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleEntry {
    pub name: String,
    pub cmd: String,
    pub working_dir: String,
    pub image_path: Option<String>,
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
    /// True for the console entry, which uses `game_id: 0` (#172).
    pub console: bool,
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

/// Rejection payload of a Sunshine command. The settings panel branches on
/// `needsElevation` instead of matching the message text (#182).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SunshineError {
    /// `sunshine_error`, `sunshine_write_failed` or `sunshine_needs_elevation`.
    pub code: String,
    pub message: String,
    /// A `PermissionDenied` write under `Program Files`: the one-time grant may
    /// fix it. Never set for other failures.
    pub needs_elevation: bool,
}

impl SunshineError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            needs_elevation: false,
        }
    }

    /// Map a failed `apps.json` write, keeping the elevation flag (#182).
    pub fn write_failure(failure: WriteFailure) -> Self {
        let code = if failure.needs_elevation {
            "sunshine_needs_elevation"
        } else {
            "sunshine_write_failed"
        };
        Self {
            code: code.to_string(),
            message: failure.message,
            needs_elevation: failure.needs_elevation,
        }
    }
}

impl From<String> for SunshineError {
    fn from(message: String) -> Self {
        Self::new("sunshine_error", message)
    }
}

/// Result of the explicit, user-triggered `grant_sunshine_write_access` (#182).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SunshineGrant {
    /// `granted`, `cancelled` (UAC declined) or `failed`.
    pub status: String,
    /// Config folder the permission change targeted.
    pub path: Option<String>,
    pub message: String,
}

/// Failed write of `apps.json`, with the flag the settings panel uses to offer
/// the one-time elevation (#182).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteFailure {
    pub message: String,
    /// `PermissionDenied` on a path under `Program Files`.
    pub needs_elevation: bool,
}

impl WriteFailure {
    pub fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            needs_elevation: false,
        }
    }
}

impl From<WriteFailure> for String {
    fn from(failure: WriteFailure) -> Self {
        failure.message
    }
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
                push(
                    Path::new(root).join("Sunshine").join("config"),
                    "Program Files",
                );
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
                    home.join("Library")
                        .join("Application Support")
                        .join("Sunshine"),
                    "Application Support",
                );
            }
        }
        _ => {
            if let Some(home) = home {
                push(home.join(".config").join("sunshine"), "XDG");
                push(
                    home.join(".var")
                        .join("app")
                        .join("dev.lizardbyte.app.Sunshine")
                        .join("config")
                        .join("sunshine"),
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

/// How Sunshine's `apps.json` was encoded on disk. Writes reuse the original
/// encoding and BOM, so a file saved by a Windows editor round-trips (#150).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextEncoding {
    #[default]
    Utf8,
    Utf16Le,
    Utf16Be,
    Windows1252,
}

/// Encoding and BOM to use when writing a text file back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextFormat {
    pub encoding: TextEncoding,
    /// Whether a UTF-8 file started with a BOM. UTF-16 always gets its BOM.
    pub bom: bool,
}

/// A decoded text file plus the format needed to write it back unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppsFile {
    pub text: String,
    pub format: TextFormat,
}

impl std::ops::Deref for AppsFile {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

/// Encode text with a file's original encoding and BOM.
pub fn encode_text(text: &str, format: TextFormat) -> Result<Vec<u8>, String> {
    match format.encoding {
        TextEncoding::Utf8 => {
            let mut bytes = Vec::with_capacity(text.len() + 3);
            if format.bom {
                bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            }
            bytes.extend_from_slice(text.as_bytes());
            Ok(bytes)
        }
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
            let little_endian = format.encoding == TextEncoding::Utf16Le;
            let mut bytes = Vec::with_capacity(text.len() * 2 + 2);
            // A BOM is required: our decoder only recognises UTF-16 by its BOM.
            bytes.extend_from_slice(if little_endian {
                &[0xFF, 0xFE]
            } else {
                &[0xFE, 0xFF]
            });
            for unit in text.encode_utf16() {
                if little_endian {
                    bytes.extend_from_slice(&unit.to_le_bytes());
                } else {
                    bytes.extend_from_slice(&unit.to_be_bytes());
                }
            }
            Ok(bytes)
        }
        TextEncoding::Windows1252 => {
            let (encoded, _, had_errors) = encoding_rs::WINDOWS_1252.encode(text);
            if had_errors {
                return Err(
                    "The new app list cannot be represented in the file's Windows-1252 encoding."
                        .to_string(),
                );
            }
            Ok(encoded.into_owned())
        }
    }
}

/// Decode a text file, detecting UTF-8/UTF-16 BOMs and falling back to
/// Windows-1252 for legacy files, like the config editor does (#150).
fn decode_text(bytes: &[u8], path: &Path) -> Result<AppsFile, String> {
    // BOMs first: UTF-16 text legitimately contains NUL bytes.
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        let (text, had_errors) = encoding_rs::UTF_8.decode_without_bom_handling(rest);
        if had_errors {
            return Err(format!(
                "{} has a UTF-8 BOM but is not UTF-8.",
                path.display()
            ));
        }
        return Ok(AppsFile {
            text: text.into_owned(),
            format: TextFormat {
                encoding: TextEncoding::Utf8,
                bom: true,
            },
        });
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let (text, had_errors) = encoding_rs::UTF_16LE.decode_without_bom_handling(rest);
        if had_errors {
            return Err(format!(
                "{} contains invalid UTF-16LE text.",
                path.display()
            ));
        }
        return Ok(AppsFile {
            text: text.into_owned(),
            format: TextFormat {
                encoding: TextEncoding::Utf16Le,
                bom: true,
            },
        });
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let (text, had_errors) = encoding_rs::UTF_16BE.decode_without_bom_handling(rest);
        if had_errors {
            return Err(format!(
                "{} contains invalid UTF-16BE text.",
                path.display()
            ));
        }
        return Ok(AppsFile {
            text: text.into_owned(),
            format: TextFormat {
                encoding: TextEncoding::Utf16Be,
                bom: true,
            },
        });
    }
    if bytes.contains(&0) {
        return Err(format!("{} looks like a binary file.", path.display()));
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Ok(AppsFile {
            text: text.to_string(),
            format: TextFormat::default(),
        });
    }
    let (text, had_errors) = encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes);
    if had_errors {
        return Err(format!(
            "{} is not UTF-8, UTF-16 or Windows-1252 text.",
            path.display()
        ));
    }
    Ok(AppsFile {
        text: text.into_owned(),
        format: TextFormat {
            encoding: TextEncoding::Windows1252,
            bom: false,
        },
    })
}

/// Sunshine starts with an empty `apps.json`; Windows editors may also save a
/// BOM or UTF-16. Decode the file before handing it to serde_json.
pub fn read_text_file(path: &Path) -> Result<AppsFile, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    decode_text(&bytes, path)
}

/// Read Sunshine's app database. Unknown keys survive; a malformed file is an
/// error so nothing is ever overwritten blindly.
pub fn parse_apps_json(text: &str) -> Result<Value, String> {
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() {
        // Sunshine starts with an empty file until the first app is added.
        return Ok(serde_json::json!({ "apps": [] }));
    }
    let value: Value = serde_json::from_str(text).map_err(|error| {
        format!(
            "Sunshine's apps.json is not valid JSON ({error}). Nothing was changed; fix or restore the file first."
        )
    })?;
    match value {
        Value::Array(_) => Ok(value),
        Value::Object(mut map) => match map.get("apps") {
            Some(Value::Array(_)) => Ok(Value::Object(map)),
            Some(_) => Err(
                "Sunshine's apps.json has an \"apps\" key that is not an array. Nothing was changed."
                    .to_string(),
            ),
            None => {
                // A config object without an app list yet: start from empty
                // without discarding the other keys.
                map.insert("apps".to_string(), Value::Array(Vec::new()));
                Ok(Value::Object(map))
            }
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
    let managed = marker
        .get("managed")
        .and_then(Value::as_bool)
        .unwrap_or(true);
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

/// True for the managed console entry (#172). It carries the Ludex marker with
/// `console: true` and no `gameId`.
pub fn is_console_app(app: &Value) -> bool {
    managed_marker(app).is_some()
        && app
            .get(MANAGED_KEY)
            .and_then(|marker| marker.get(CONSOLE_MARKER))
            .and_then(Value::as_bool)
            .unwrap_or(false)
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

/// Sunshine entry for the console app (#172). Same fields as a game entry, but
/// the marker says `console: true` and carries no game id.
pub fn generated_console_app(entry: &ConsoleEntry, server_id: &str) -> Value {
    let mut map = Map::new();
    map.insert("name".to_string(), Value::String(entry.name.clone()));
    map.insert("cmd".to_string(), Value::String(entry.cmd.clone()));
    map.insert(
        "working-dir".to_string(),
        Value::String(entry.working_dir.clone()),
    );
    if let Some(image) = entry
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
            "console": true,
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
///
/// This is the per-game-only form kept for callers and tests; a console sync
/// uses [`merge_entries`].
#[allow(dead_code)]
pub fn merge_apps(
    existing: Option<&str>,
    desired: &[PublishedApp],
    server_id: &str,
    remove_missing: bool,
) -> Result<(Value, MergeReport), String> {
    merge_entries(existing, desired, None, server_id, remove_missing)
}

/// Merge games and, optionally, the single console app (#172).
///
/// The console entry is identified by `ludex.console = true`, not by a game id,
/// so enabling console mode adds one `Ludex` app and removes every per-game
/// entry of the active server in the same write (one backup, one file). A
/// second run is a no-op, and duplicates left by hand edits collapse to one.
pub fn merge_entries(
    existing: Option<&str>,
    desired: &[PublishedApp],
    console: Option<&ConsoleEntry>,
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
        let mut console_indices: Vec<usize> = Vec::new();
        for (index, app) in apps.iter().enumerate() {
            if !is_managed_app(app) || !same_server(app, server_id) {
                continue;
            }
            if is_console_app(app) {
                console_indices.push(index);
                continue;
            }
            if let Some(game_id) = managed_game_id(app) {
                existing_index.entry(game_id).or_insert(index);
            }
        }
        let existing_len = apps.len();

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

        if let Some(entry) = console {
            let generated = generated_console_app(entry, server_id);
            match console_indices.first().copied() {
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

        // Entries to drop: stale games when the caller asked for a full sync,
        // and duplicate console entries (always collapsed when a console entry
        // is desired). Only pre-existing indices are candidates; entries pushed
        // above live past `existing_len`.
        let keep: Vec<i64> = desired.iter().map(|app| app.game_id).collect();
        let mut drop: BTreeSet<usize> = BTreeSet::new();
        for (index, app) in apps.iter().take(existing_len).enumerate() {
            let managed = is_managed_app(app) && same_server(app, server_id);
            if !managed {
                continue;
            }
            if is_console_app(app) {
                let is_primary = console.is_some() && console_indices.first() == Some(&index);
                if !is_primary && (remove_missing || console.is_some()) {
                    drop.insert(index);
                }
                continue;
            }
            if remove_missing {
                match managed_game_id(app) {
                    Some(game_id) if keep.contains(&game_id) => {}
                    // A managed entry with no game id is stale; remove it so a
                    // corrupted marker cannot pin an app forever.
                    _ => {
                        drop.insert(index);
                    }
                }
            }
        }
        report.removed = drop.len();
        if !drop.is_empty() {
            let mut kept = Vec::with_capacity(apps.len() - drop.len());
            for (index, app) in apps.drain(..).enumerate() {
                if !drop.contains(&index) {
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
///
/// The per-game-only form; a console preview uses [`preview_entries`].
#[allow(dead_code)]
pub fn preview_apps(
    existing: Option<&str>,
    desired: &[PublishedApp],
    server_id: &str,
) -> Result<(Vec<PreviewEntry>, Vec<PreviewRemoval>), String> {
    let (entries, _, removals) = preview_entries(existing, desired, None, server_id)?;
    Ok((entries, removals))
}

/// Preview a sync that may publish the console app (#172).
///
/// Returns the game entries, the console entry when it would be written, and
/// the removals. Per-game entries of the active server become removals when
/// console mode replaced them, which is how the UI explains the migration
/// before it happens.
pub fn preview_entries(
    existing: Option<&str>,
    desired: &[PublishedApp],
    console: Option<&ConsoleEntry>,
    server_id: &str,
) -> Result<(Vec<PreviewEntry>, Option<PreviewEntry>, Vec<PreviewRemoval>), String> {
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
            console: false,
        });
    }

    let console_preview = console.map(|entry| {
        let generated = generated_console_app(entry, server_id);
        let action = apps
            .iter()
            .find(|existing| {
                is_managed_app(existing)
                    && same_server(existing, server_id)
                    && is_console_app(existing)
            })
            .map(|existing| {
                if app_matches(existing, &generated) {
                    "unchanged"
                } else {
                    "update"
                }
            })
            .unwrap_or("add");
        PreviewEntry {
            game_id: 0,
            title: entry.name.clone(),
            name: entry.name.clone(),
            cmd: entry.cmd.clone(),
            working_dir: entry.working_dir.clone(),
            image_path: entry.image_path.clone(),
            action: action.to_string(),
            needs_ludex_scheme: false,
            console: true,
        }
    });

    let keep: Vec<i64> = desired.iter().map(|app| app.game_id).collect();
    let mut console_seen = false;
    let removals = apps
        .iter()
        .filter(|app| {
            if !is_managed_app(app) || !same_server(app, server_id) {
                return false;
            }
            if is_console_app(app) {
                // The first console entry is the one a sync would reuse; any
                // other is a duplicate and the one a per-game sync removes.
                if console.is_some() && !console_seen {
                    console_seen = true;
                    return false;
                }
                return true;
            }
            managed_game_id(app)
                .map(|game_id| !keep.contains(&game_id))
                .unwrap_or(true)
        })
        .map(|app| PreviewRemoval {
            name: as_string(app.get("name")).unwrap_or_else(|| "Ludex entry".to_string()),
            game_id: managed_game_id(app),
        })
        .collect();

    Ok((entries, console_preview, removals))
}

/// Serialize an apps database for writing.
pub fn render_apps(value: &Value) -> Result<String, String> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|error| format!("Could not serialize Sunshine's apps.json: {error}"))?;
    text.push('\n');
    Ok(text)
}

/// True when a path lives under a Windows `Program Files` folder, where
/// writing requires elevation (#150).
fn is_program_files_path(path: &Path) -> bool {
    path.to_string_lossy()
        .to_ascii_lowercase()
        .contains("program files")
}

/// Value of an environment variable, matching the name case-insensitively
/// because Windows does not preserve the casing (#182).
fn env_value<'a>(env: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    env.iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
}

/// The Windows `Program Files` roots from `env`, in detection order (#182).
pub fn program_files_roots(env: &BTreeMap<String, String>) -> Vec<PathBuf> {
    ["ProgramFiles", "ProgramFiles(x86)"]
        .iter()
        .filter_map(|key| env_value(env, key))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Case-insensitive containment test below one of `roots`. The separator is
/// normalised, so `C:/Program Files/…` matches too (#182).
pub fn path_under_roots(path: &Path, roots: &[PathBuf]) -> bool {
    let normalize = |value: &Path| {
        value
            .to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };
    let candidate = normalize(path);
    roots.iter().any(|root| {
        let root = normalize(root);
        !root.is_empty() && (candidate == root || candidate.starts_with(&(root + "\\")))
    })
}

/// True when `error` is a `PermissionDenied` denial below `Program Files`: the
/// case the one-time elevation can fix (#182).
pub fn needs_elevation_at(path: &Path, error: &std::io::Error, roots: &[PathBuf]) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied && path_under_roots(path, roots)
}

fn needs_elevation(path: &Path, error: &std::io::Error) -> bool {
    needs_elevation_at(path, error, &program_files_roots(&env_map()))
}

/// Probe without writing or changing ACLs: an existing `apps.json` that cannot
/// be opened for write inside `Program Files` needs elevation (#182).
fn sunshine_needs_elevation(apps_path: &Path) -> bool {
    if !path_under_roots(apps_path, &program_files_roots(&env_map())) {
        return false;
    }
    match std::fs::OpenOptions::new().write(true).open(apps_path) {
        Ok(_) => false,
        Err(error) => error.kind() == std::io::ErrorKind::PermissionDenied,
    }
}

/// Actionable hint for a write failure caused by permissions. `Program Files`
/// is named explicitly because that is where Sunshine installs by default;
/// Ludex never copies `apps.json` elsewhere on its own.
fn permission_hint(path: &Path, error: &std::io::Error) -> Option<String> {
    let denied = error.kind() == std::io::ErrorKind::PermissionDenied;
    if !denied && !is_program_files_path(path) {
        return None;
    }
    let mut hint = String::from(
        " Windows does not allow Ludex to write there: point Sunshine at a user-writable config folder (Settings → Streaming, or Sunshine's own config) or run Ludex with elevated permissions, then sync again.",
    );
    if is_program_files_path(path) {
        hint.push_str(" The folder is inside Program Files.");
    }
    Some(hint)
}

fn write_failure(action: &str, path: &Path, error: &std::io::Error) -> String {
    let mut message = format!("{action} {}: {error}.", path.display());
    if let Some(hint) = permission_hint(path, error) {
        message.push_str(&hint);
    }
    message.push_str(" Nothing was changed.");
    message
}

fn write_failure_status(action: &str, path: &Path, error: &std::io::Error) -> WriteFailure {
    WriteFailure {
        message: write_failure(action, path, error),
        needs_elevation: needs_elevation(path, error),
    }
}

/// Explain a failed `apps.json` write; public so the message stays testable.
#[allow(dead_code)]
pub fn write_failure_message(path: &Path, error: &std::io::Error) -> String {
    write_failure("Could not write", path, error)
}

/// Backup file path next to `apps.json` (`apps.json.ludex-backup-….bak`).
pub fn backup_path(apps_path: &Path) -> PathBuf {
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let suffix = crate::util::short_id(&uuid::Uuid::new_v4().to_string());
    apps_path.with_file_name(format!("apps.json.ludex-backup-{stamp}-{suffix}.bak"))
}

/// Copy the current `apps.json` before touching it. A failure aborts the write;
/// the backup is a byte-exact copy of the original encoding (#150).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn backup_apps(apps_path: &Path) -> Result<PathBuf, String> {
    backup_apps_status(apps_path).map_err(String::from)
}

fn backup_apps_status(apps_path: &Path) -> Result<PathBuf, WriteFailure> {
    let backup = backup_path(apps_path);
    std::fs::copy(apps_path, &backup)
        .map_err(|error| write_failure_status("Could not back up", apps_path, &error))?;
    Ok(backup)
}

/// Backup then atomically replace `apps.json` (temp file in the same folder,
/// fsync, rename). `format` is the encoding read from the existing file, so a
/// UTF-16 or BOM-prefixed file keeps its encoding. Returns the backup path, if
/// the file already existed.
pub fn write_apps_with_backup(
    apps_path: &Path,
    content: &str,
    format: TextFormat,
) -> Result<Option<PathBuf>, String> {
    write_apps_with_backup_status(apps_path, content, format).map_err(String::from)
}

/// Same as [`write_apps_with_backup`], keeping the structured flag the settings
/// panel branches on (#182).
pub fn write_apps_with_backup_status(
    apps_path: &Path,
    content: &str,
    format: TextFormat,
) -> Result<Option<PathBuf>, WriteFailure> {
    let bytes = encode_text(content, format).map_err(WriteFailure::plain)?;
    if let Some(parent) = apps_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            write_failure_status("Could not create the Sunshine folder", parent, &error)
        })?;
    }

    let backup = if apps_path.is_file() {
        Some(backup_apps_status(apps_path)?)
    } else {
        None
    };

    let temporary = apps_path.with_file_name("apps.json.ludex.tmp");
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary)
            .map_err(|error| write_failure_status("Could not write", &temporary, &error))?;
        file.write_all(&bytes)
            .map_err(|error| write_failure_status("Could not write", &temporary, &error))?;
        file.sync_all()
            .map_err(|error| write_failure_status("Could not flush", &temporary, &error))?;
    }

    if let Err(first_error) = std::fs::rename(&temporary, apps_path) {
        if apps_path.exists() {
            std::fs::remove_file(apps_path).map_err(|error| {
                WriteFailure {
                    message: format!(
                        "Could not replace {} (rename failed: {first_error}; remove failed: {error}). The backup is kept.",
                        apps_path.display()
                    ),
                    needs_elevation: needs_elevation(apps_path, &error)
                        || needs_elevation(apps_path, &first_error),
                }
            })?;
            std::fs::rename(&temporary, apps_path)
                .map_err(|error| write_failure_status("Could not replace", apps_path, &error))?;
        } else {
            return Err(write_failure_status(
                "Could not write",
                apps_path,
                &first_error,
            ));
        }
    }

    Ok(backup)
}

/// `sunshine_status` payload for the settings panel.
pub fn sunshine_status(settings: &LocalSettings) -> SunshineStatus {
    let detection = detect_sunshine(settings, platform_name(), home_dir().as_deref(), &env_map());
    let apps_path = detection
        .paths
        .as_ref()
        .map(|paths| paths.apps_json.clone());
    let apps_found = apps_path
        .as_deref()
        .map(|path| Path::new(path).is_file())
        .unwrap_or(false);

    let mut warnings = detection.warnings.clone();
    let mut entries = 0;
    let mut managed_entries = 0;
    let mut apps_error: Option<String> = None;
    if let Some(path) = apps_path.as_deref().filter(|_| apps_found) {
        match read_text_file(Path::new(path)) {
            Ok(file) => match parse_apps_json(&file.text) {
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
                Err(error) => {
                    warnings.push(error.clone());
                    apps_error = Some(error);
                }
            },
            Err(error) => {
                warnings.push(error.clone());
                apps_error = Some(error);
            }
        }
    }

    let detected = detection.paths.is_some() || detection.binary.is_some();
    let needs_elevation = apps_path
        .as_deref()
        .map(Path::new)
        .map(sunshine_needs_elevation)
        .unwrap_or(false);
    let message = if !detected {
        "Sunshine was not found on this machine. Install Sunshine to publish the Ludex library as streaming apps."
            .to_string()
    } else if apps_path.is_none() {
        "Sunshine is installed but its config folder was not found. Set the folder manually in Settings → Streaming."
            .to_string()
    } else if let Some(error) = apps_error {
        error
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
        conf_path: detection
            .paths
            .as_ref()
            .map(|paths| paths.sunshine_conf.clone()),
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
        needs_elevation,
        warnings,
        message,
    }
}

/// Read `apps.json` for the given status, if any. The returned file keeps the
/// on-disk encoding so a sync writes it back in the same format (#150).
pub fn read_apps_text(status: &SunshineStatus) -> Result<Option<AppsFile>, String> {
    let Some(path) = status.apps_path.as_deref() else {
        return Ok(None);
    };
    if !Path::new(path).is_file() {
        return Ok(None);
    }
    read_text_file(Path::new(path)).map(Some)
}

/* ------------------- One-time elevation (#182) ---------------------------- */

/// Config folder of the detected Sunshine install: the parent of `apps.json`.
/// Detection is the same one `sunshine_status` uses, so the frontend cannot
/// point the permission change at an arbitrary folder.
pub fn sunshine_grant_plan(settings: &LocalSettings) -> Result<PathBuf, String> {
    sunshine_grant_plan_for(settings, platform_name(), home_dir().as_deref(), &env_map())
}

pub fn sunshine_grant_plan_for(
    settings: &LocalSettings,
    platform: &str,
    home: Option<&Path>,
    env: &BTreeMap<String, String>,
) -> Result<PathBuf, String> {
    let detection = detect_sunshine(settings, platform, home, env);
    let Some(paths) = detection.paths else {
        return Err(
            "Sunshine's config folder was not found. Set it in Settings → Streaming or install Sunshine first."
                .to_string(),
        );
    };
    let apps = PathBuf::from(&paths.apps_json);
    apps.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            format!(
                "Sunshine's apps.json has no config folder: {}.",
                apps.display()
            )
        })
}

/// Canonicalise `dir` and accept it only below a `Program Files` root. The
/// canonical form also rejects `..` traversals and symlinks (#182).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn validate_program_files_dir(dir: &Path, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let canonical = std::fs::canonicalize(dir).map_err(|error| {
        format!(
            "Could not resolve the Sunshine config folder {}: {error}.",
            dir.display()
        )
    })?;
    for root in roots {
        let Ok(root) = std::fs::canonicalize(root) else {
            continue;
        };
        if canonical == root || canonical.starts_with(&root) {
            return Ok(canonical);
        }
    }
    Err(format!(
        "Refusing to change permissions on {}: it is not inside Program Files.",
        canonical.display()
    ))
}

/// Grant the current Windows account modify rights on Sunshine's config folder
/// after one UAC prompt. Never called automatically and never writes
/// `apps.json`; the UI retries the publish afterwards (#182).
pub fn grant_sunshine_write_access(settings: &LocalSettings) -> Result<SunshineGrant, String> {
    if !cfg!(windows) {
        return Err(
            "Granting write access is only available on Windows. Point Sunshine at a user-writable config folder instead."
                .to_string(),
        );
    }
    let config_dir = sunshine_grant_plan(settings)?;
    grant_config_write_access(&config_dir)
}

#[cfg(not(windows))]
fn grant_config_write_access(config_dir: &Path) -> Result<SunshineGrant, String> {
    let _ = config_dir;
    Err(
        "Granting write access is only available on Windows. Point Sunshine at a user-writable config folder instead."
            .to_string(),
    )
}

#[cfg(windows)]
fn grant_config_write_access(config_dir: &Path) -> Result<SunshineGrant, String> {
    let roots = program_files_roots(&env_map());
    let config_dir = validate_program_files_dir(config_dir, &roots)?;
    let apps_path = config_dir.join("apps.json");
    if sunshine_config_writable(&apps_path) {
        return Ok(SunshineGrant {
            status: "granted".to_string(),
            path: Some(config_dir.to_string_lossy().to_string()),
            message: "Sunshine's config folder is already writable; nothing was changed."
                .to_string(),
        });
    }

    let dir = config_dir.to_string_lossy();
    if dir.contains('"') {
        return Err(format!(
            "Refusing to change permissions on {dir}: the path contains an unsupported character."
        ));
    }

    let script = windows_grant_script(&config_dir);
    let encoded = encode_powershell(&script);
    let output = {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("powershell.exe");
        command
            .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // CREATE_NO_WINDOW: the helper itself must not flash a console.
            .creation_flags(0x0800_0000);
        command.output().map_err(|error| {
            format!("Could not start PowerShell for the permission prompt: {error}.")
        })?
    };

    let mut detail = String::from_utf8_lossy(&output.stderr).to_string();
    if detail.trim().is_empty() {
        detail = String::from_utf8_lossy(&output.stdout).to_string();
    }
    Ok(grant_outcome(
        output.status.code(),
        &detail,
        &config_dir,
        &apps_path,
        sunshine_config_writable(&apps_path),
    ))
}

/// Map the elevated `icacls` exit code to the typed result (#182). Pure so the
/// mapping is testable without running `icacls`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn grant_outcome(
    exit_code: Option<i32>,
    detail: &str,
    config_dir: &Path,
    apps_path: &Path,
    writable: bool,
) -> SunshineGrant {
    let path = Some(config_dir.to_string_lossy().to_string());
    // ERROR_CANCELLED: the user declined the UAC prompt.
    if exit_code == Some(1223) {
        return SunshineGrant {
            status: "cancelled".to_string(),
            path,
            message: "The elevation prompt was cancelled; nothing was changed.".to_string(),
        };
    }
    if exit_code != Some(0) {
        return SunshineGrant {
            status: "failed".to_string(),
            path,
            message: format!(
                "Windows did not grant write access: {}",
                grant_detail(detail)
            ),
        };
    }
    if !writable {
        return SunshineGrant {
            status: "failed".to_string(),
            path,
            message: format!(
                "Windows accepted the permission change, but {} is still not writable. Point Sunshine at a user-writable config folder instead.",
                apps_path.display()
            ),
        };
    }
    SunshineGrant {
        status: "granted".to_string(),
        path,
        message: format!(
            "This account can now modify {}. Retry the publish.",
            config_dir.display()
        ),
    }
}

/// First usable line of the helper output, bounded and single-line (#182).
#[cfg_attr(not(windows), allow(dead_code))]
fn grant_detail(detail: &str) -> String {
    let line = detail
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("the permission change failed");
    line.chars().take(300).collect()
}

/// PowerShell script launched with `-EncodedCommand`: it starts exactly one
/// elevated `icacls` for the current identity (#182).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn windows_grant_script(config_dir: &Path) -> String {
    let dir = config_dir.to_string_lossy().replace('\'', "''");
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         $dir = '{dir}'\n\
         $user = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name\n\
         if ($dir.Contains('\"') -or $user.Contains('\"')) {{ Write-Error 'Unsupported character in the Sunshine path.'; exit 1 }}\n\
         try {{\n\
         \x20 $arguments = @('\"' + $dir + '\"', '/grant', '\"' + $user + ':(OI)(CI)M\"', '/Q')\n\
         \x20 $process = Start-Process -FilePath 'icacls.exe' -ArgumentList ($arguments -join ' ') -Verb RunAs -Wait -PassThru\n\
         \x20 exit $process.ExitCode\n\
         }} catch {{\n\
         \x20 if ($_.Exception.HResult -eq -2147023673) {{ exit 1223 }}\n\
         \x20 Write-Error $_.Exception.Message\n\
         \x20 exit 1\n\
         }}\n"
    )
}

/// Encode a PowerShell script for `-EncodedCommand`: UTF-16LE, base64 (#182).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn encode_powershell(script: &str) -> String {
    let mut bytes = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    base64_encode(&bytes)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0] as u32;
        let second = *chunk.get(1).unwrap_or(&0) as u32;
        let third = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (first << 16) | (second << 8) | third;
        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((triple >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// True when this account can create the temp/backup files next to
/// `apps.json` (#182). Used only after the elevated grant to report a partial
/// success instead of letting the next publish fail again.
#[cfg(windows)]
fn sunshine_config_writable(apps_path: &Path) -> bool {
    if apps_path.is_file()
        && std::fs::OpenOptions::new()
            .write(true)
            .open(apps_path)
            .is_err()
    {
        return false;
    }
    let Some(dir) = apps_path.parent() else {
        return false;
    };
    let probe = dir.join(".ludex-write-test.tmp");
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
    /// Where the host list came from (`Moonlight.ini`, `Windows registry`,
    /// `Moonlight.conf`, `override`). `None` when no config was found (#151).
    pub config_source: Option<String>,
    pub config_found: bool,
    /// Manual host override used when Moonlight stores its settings elsewhere.
    pub manual_host: Option<String>,
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
            // Qt stores arrays under numeric keys (`hosts\1`): an address is
            // more useful than a bare index when no custom name was saved.
            let name = entry.custom_name.unwrap_or_else(|| {
                if key.chars().all(|character| character.is_ascii_digit()) {
                    address.clone()
                } else {
                    key
                }
            });
            Some(MoonlightHost {
                name,
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

/* ------------------- Moonlight settings sources (#151) -------------------- */

/// Registry root where installed Moonlight-qt stores Qt `QSettings` (#151).
pub const MOONLIGHT_REGISTRY_KEY: &str = r"HKCU\SOFTWARE\Moonlight Game Streaming Project";

/// One value from Moonlight's Qt settings. `path` is the group path relative
/// to the application root (`hosts\1`), `name` the value name (`address`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QtSettingEntry {
    pub path: String,
    pub name: String,
    pub value: String,
}

/// Injectable Moonlight settings source. Windows reads the registry; tests
/// pass a fixture, so no `reg.exe` process is involved (#151).
pub trait MoonlightSettingsSource {
    fn read(&self) -> Result<Vec<QtSettingEntry>, String>;
}

/// No settings source: used on non-Windows platforms.
#[cfg_attr(windows, allow(dead_code))]
pub struct NoMoonlightSettings;

impl MoonlightSettingsSource for NoMoonlightSettings {
    fn read(&self) -> Result<Vec<QtSettingEntry>, String> {
        Ok(Vec::new())
    }
}

/// Read-only `reg.exe query` of Moonlight's `QSettings` key. Ludex never
/// writes to the registry.
#[cfg(windows)]
pub struct WindowsMoonlightSettings;

#[cfg(windows)]
impl MoonlightSettingsSource for WindowsMoonlightSettings {
    fn read(&self) -> Result<Vec<QtSettingEntry>, String> {
        let output = Command::new("reg")
            .args(["query", MOONLIGHT_REGISTRY_KEY, "/s"])
            .output()
            .map_err(|error| format!("Could not run reg.exe: {error}"))?;
        if !output.status.success() {
            // The key does not exist until Moonlight saves a setting.
            return Ok(Vec::new());
        }
        // reg.exe writes the console codepage; fall back to Windows-1252 when
        // a host name is not valid UTF-8.
        let text = match std::str::from_utf8(&output.stdout) {
            Ok(text) => text.to_string(),
            Err(_) => encoding_rs::WINDOWS_1252
                .decode_without_bom_handling(&output.stdout)
                .0
                .into_owned(),
        };
        Ok(parse_reg_query_output(&text))
    }
}

/// Path below Moonlight's `QSettings` root, from a `reg query` key line.
#[cfg_attr(not(windows), allow(dead_code))]
fn registry_relative_path(line: &str) -> Option<String> {
    const SUFFIX: &str = "SOFTWARE\\MOONLIGHT GAME STREAMING PROJECT";
    let upper = line.to_ascii_uppercase();
    let index = upper.find(SUFFIX)?;
    Some(
        line[index + SUFFIX.len()..]
            .trim_start_matches('\\')
            .to_string(),
    )
}

/// Parse `reg query <key> /s` output into Qt settings entries. Kept separate
/// from the Windows-only reader so the registry layout is unit tested on Linux.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_reg_query_output(text: &str) -> Vec<QtSettingEntry> {
    let mut entries = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // Key lines start at column 0; values are indented.
        if !(line.starts_with(' ') || line.starts_with('\t')) {
            current = registry_relative_path(line.trim());
            continue;
        }
        let Some(path) = current.as_deref() else {
            continue;
        };
        let line = line.trim_start();
        let Some((name, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let rest = rest.trim_start();
        let Some((kind, data)) = rest.split_once(char::is_whitespace) else {
            continue;
        };
        if !kind.starts_with("REG_") || name == "(Default)" {
            continue;
        }
        entries.push(QtSettingEntry {
            path: path.to_string(),
            name: name.to_string(),
            value: data.trim().to_string(),
        });
    }
    entries
}

/// Map Qt settings entries (registry layout) to hosts. The first group segment
/// becomes an INI section and the rest a `host\field` key, so the existing Qt
/// host parser handles the registry and `.conf` layouts identically.
pub fn hosts_from_qt_settings(entries: &[QtSettingEntry]) -> Vec<MoonlightHost> {
    let mut text = String::new();
    for entry in entries {
        let segments: Vec<&str> = entry
            .path
            .split(['\\', '/'])
            .map(str::trim)
            .filter(|segment| !segment.is_empty())
            .collect();
        let Some((section, groups)) = segments.split_first() else {
            // Top-level settings (language, window geometry) are not hosts.
            continue;
        };
        let mut key = groups.join("\\");
        if !key.is_empty() {
            key.push('\\');
        }
        key.push_str(&entry.name);
        let value = entry.value.replace(['\r', '\n'], " ");
        text.push_str(&format!("[{section}]\n{key}={value}\n"));
    }
    parse_moonlight_conf(&text)
}

/// Portable Moonlight keeps `Moonlight.ini` next to `Moonlight.exe` (#151).
pub fn portable_moonlight_ini(binary: Option<&str>, current_dir: Option<&Path>) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(binary) = binary {
        if let Some(parent) = Path::new(binary).parent() {
            candidates.push(parent.join("Moonlight.ini"));
        }
    }
    if let Some(dir) = current_dir {
        candidates.push(dir.join("Moonlight.ini"));
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// Read a Moonlight config with the shared text decoder (BOM/UTF-16 aware).
fn read_moonlight_config(path: &Path) -> Result<Vec<MoonlightHost>, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    let file = decode_text(&bytes, path)?;
    Ok(parse_moonlight_conf(&file.text))
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

fn moonlight_binary(
    platform: &str,
    home: Option<&Path>,
    env: &BTreeMap<String, String>,
) -> Option<String> {
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

/// Detection result before it is serialized for the settings panel.
#[derive(Debug, Clone, Default)]
pub struct MoonlightDetection {
    pub binary: Option<String>,
    pub config_path: Option<PathBuf>,
    pub config_source: Option<String>,
    pub config_found: bool,
    pub manual_host: Option<String>,
    pub hosts: Vec<MoonlightHost>,
    pub supported: bool,
    pub warnings: Vec<String>,
    pub message: String,
}

/// Detect Moonlight and parse its host list.
///
/// Order: explicit `moonlightConfigPath`, then the format of the platform:
/// Windows reads portable `Moonlight.ini` next to `Moonlight.exe`, then the
/// installed build's Qt registry settings, then legacy `.conf` files. Linux and
/// Flatpak keep the XDG/Flatpak `Moonlight.conf`. The registry reader is
/// injectable so the layout is tested without Windows (#151).
pub fn detect_moonlight(
    settings: &LocalSettings,
    platform: &str,
    home: Option<&Path>,
    env: &BTreeMap<String, String>,
    current_dir: Option<&Path>,
    registry: &dyn MoonlightSettingsSource,
) -> MoonlightDetection {
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
        moonlight_binary(platform, home, env)
    };

    let supported = platform != "macos";
    let manual_host =
        crate::util::non_empty(Some(settings.moonlight_host.as_str())).map(str::to_string);

    let mut config_path: Option<PathBuf> = None;
    let mut config_source: Option<String> = None;
    let mut config_found = false;
    let mut hosts = Vec::new();
    let mut config_error: Option<String> = None;

    // 1. An explicit override always wins.
    let override_path = settings.moonlight_config_path.trim();
    if !override_path.is_empty() {
        let configured = PathBuf::from(override_path);
        if configured.is_file() {
            config_path = Some(configured);
            config_source = Some("override".to_string());
            config_found = true;
        } else {
            warnings.push(format!(
                "The configured Moonlight config file does not exist: {}",
                configured.display()
            ));
        }
    }

    // 2. Platform-specific detection.
    if supported && config_path.is_none() && config_error.is_none() {
        if platform == "windows" {
            if let Some(path) = portable_moonlight_ini(binary.as_deref(), current_dir) {
                config_path = Some(path);
                config_source = Some("Moonlight.ini (portable)".to_string());
                config_found = true;
            }
        }

        if platform == "windows" && config_path.is_none() && config_error.is_none() {
            // Installed Moonlight-qt: read-only QSettings in the registry.
            match registry.read() {
                Ok(entries) if !entries.is_empty() => {
                    hosts = hosts_from_qt_settings(&entries);
                    config_source = Some(format!("Windows registry ({MOONLIGHT_REGISTRY_KEY})"));
                    config_found = true;
                }
                Ok(_) => {}
                Err(error) => {
                    warnings.push(format!(
                        "Could not read Moonlight's registry settings: {error}"
                    ));
                    config_error = Some(error);
                }
            }
        }

        if config_path.is_none() && config_error.is_none() && !config_found {
            // Older builds and hand-made portable copies may still use a file.
            if let Some(path) = moonlight_conf_candidates(platform, home, env)
                .into_iter()
                .find(|path| path.is_file())
            {
                config_path = Some(path);
                config_source = Some("Moonlight.conf".to_string());
                config_found = true;
            }
        }
    }

    // 3. Read the selected file, preserving its text decoding.
    if let Some(path) = config_path.clone() {
        match read_moonlight_config(&path) {
            Ok(parsed) => hosts = parsed,
            Err(error) => {
                warnings.push(error.clone());
                config_error = Some(error);
            }
        }
    }

    let detected = binary.is_some();
    if !detected {
        warnings.push(
            "Moonlight was not found. Install Moonlight (moonlight-qt) to stream from a Sunshine host."
                .to_string(),
        );
    }
    if !supported {
        warnings.push(
            "Moonlight stores its hosts in a macOS preferences plist, which Ludex does not read. Use Moonlight directly."
                .to_string(),
        );
    }

    let message = if !supported {
        "Moonlight host auto-detection is not available on macOS.".to_string()
    } else if !detected {
        "Moonlight was not found on this machine. Install it to stream from a Sunshine host."
            .to_string()
    } else if let Some(error) = config_error.as_deref() {
        format!("Moonlight's configuration exists but could not be read: {error}")
    } else if !hosts.is_empty() {
        format!("{} host(s) found in Moonlight.", hosts.len())
    } else if let Some(manual) = manual_host.as_deref() {
        format!(
            "No saved Moonlight host was found; the manual host {manual} will be used for streaming."
        )
    } else if config_found {
        "Moonlight is configured but no host is saved yet. Pair a Sunshine host in Moonlight first."
            .to_string()
    } else {
        "Moonlight's configuration was not found. Pair a Sunshine host in Moonlight once (Ludex reads Moonlight.ini, Moonlight.conf or the Windows registry), or set a manual host in Settings → Streaming."
            .to_string()
    };

    MoonlightDetection {
        binary,
        config_path,
        config_source,
        config_found,
        manual_host,
        hosts,
        supported,
        warnings,
        message,
    }
}

/// `moonlight_status` payload for the settings panel and the Stream dialog.
pub fn moonlight_status(settings: &LocalSettings) -> MoonlightStatus {
    let platform = platform_name();
    let home = home_dir();
    let env = env_map();
    let current_dir = std::env::current_dir().ok();
    #[cfg(windows)]
    let registry = WindowsMoonlightSettings;
    #[cfg(not(windows))]
    let registry = NoMoonlightSettings;

    let detection = detect_moonlight(
        settings,
        platform,
        home.as_deref(),
        &env,
        current_dir.as_deref(),
        &registry,
    );
    let detected = detection.binary.is_some();
    MoonlightStatus {
        detected,
        platform: platform.to_string(),
        binary: detection.binary,
        config_path: detection
            .config_path
            .map(|path| path.to_string_lossy().to_string()),
        config_source: detection.config_source,
        config_found: detection.config_found,
        manual_host: detection.manual_host,
        hosts: detection.hosts,
        supported: detection.supported,
        warnings: detection.warnings,
        message: detection.message,
    }
}

/// Hosts offered by the Stream dialog. The manual host is appended as a paired
/// fallback when Moonlight's own list is empty or does not contain it (#151).
pub fn stream_hosts(status: &MoonlightStatus) -> Vec<MoonlightHost> {
    let mut hosts = status.hosts.clone();
    if let Some(host) = status.manual_host.as_deref() {
        if !hosts
            .iter()
            .any(|saved| saved.address == host || saved.name == host)
        {
            hosts.push(MoonlightHost {
                name: host.to_string(),
                address: host.to_string(),
                local_address: None,
                remote_address: None,
                manual_address: Some(host.to_string()),
                paired: true,
            });
        }
    }
    hosts
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
pub fn select_host(
    hosts: &[MoonlightHost],
    requested: Option<&str>,
) -> Result<MoonlightHost, String> {
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
            return Err(format!("Choose a Moonlight host first ({names})."));
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
        let ludex = list
            .iter()
            .find(|app| managed_game_id(app) == Some(1))
            .unwrap();
        assert_eq!(
            as_string(ludex.get("cmd")).as_deref(),
            Some("/games/celeste/Celeste.exe")
        );
        assert_eq!(as_string(ludex.get("detached")).as_deref(), Some("true"));
        assert!(is_managed_app(
            list.iter()
                .find(|app| managed_game_id(app) == Some(2))
                .unwrap()
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
        let entry = list
            .iter()
            .find(|app| managed_game_id(app) == Some(1))
            .unwrap();
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
        assert!(list
            .iter()
            .any(|app| as_string(app.get("name")).as_deref() == Some("Desktop")));
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
        let (root, _) =
            merge_apps(Some(&render_apps(&root).unwrap()), &apps, "server-b", true).unwrap();

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

        let backup = write_apps_with_backup(&path, "{\"apps\":[]}\n", TextFormat::default())
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
        assert!(write_apps_with_backup(&path, "{}", TextFormat::default()).is_err());
        assert!(path.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sunshine_candidates_follow_the_platform() {
        let mut env = BTreeMap::new();
        env.insert("ProgramFiles".to_string(), "C:\\Program Files".to_string());
        env.insert(
            "LOCALAPPDATA".to_string(),
            "C:\\Users\\me\\AppData\\Local".to_string(),
        );
        let windows = sunshine_candidates("windows", None, &env);
        assert!(windows[0].apps_json.contains("Sunshine"));
        assert!(windows[0].apps_json.ends_with("apps.json"));
        assert!(windows
            .iter()
            .any(|candidate| candidate.source == "Local AppData"));

        let home = Path::new("/home/me");
        let linux = sunshine_candidates("linux", Some(home), &BTreeMap::new());
        assert_eq!(linux[0].apps_json, "/home/me/.config/sunshine/apps.json");
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

        settings.sunshine_apps_path = dir
            .join("missing")
            .join("apps.json")
            .to_string_lossy()
            .to_string();
        let detection = detect_sunshine(&settings, "linux", None, &BTreeMap::new());
        assert!(detection.paths.is_none());
        assert!(detection
            .warnings
            .iter()
            .any(|warning| warning.contains("does not exist")));
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
        let (entries, removals) =
            preview_apps(Some(&render_apps(&root).unwrap()), &apps, "srv_1").unwrap();
        assert_eq!(entries[0].action, "unchanged");
        assert!(removals.is_empty());
    }

    #[test]
    fn parse_creates_an_empty_app_list_for_an_object_without_apps() {
        let value = parse_apps_json(r#"{"env":{"PATH":"/usr/bin"}}"#).unwrap();
        assert_eq!(apps_ref(&value).unwrap().len(), 0);
        assert_eq!(
            value.get("env").and_then(|env| env.get("PATH")),
            Some(&Value::String("/usr/bin".to_string()))
        );
    }

    #[test]
    fn parse_accepts_empty_and_bom_prefixed_files() {
        for text in ["", "   \n\t", "\u{feff}{\"apps\":[]}"] {
            let value = parse_apps_json(text).unwrap();
            assert_eq!(apps_ref(&value).unwrap().len(), 0);
        }
    }

    #[test]
    fn read_text_file_decodes_utf16_apps_json() {
        let root = temp_dir("sunshine-utf16");
        let path = root.join("apps.json");
        let text = "{\"apps\":[]}";
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();
        let file = read_text_file(&path).unwrap();
        assert_eq!(file.text, text);
        assert_eq!(file.format.encoding, TextEncoding::Utf16Le);
        assert!(file.format.bom);
        let _ = std::fs::remove_dir_all(&root);
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
        let living = hosts
            .iter()
            .find(|host| host.name == "Living Room")
            .unwrap();
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
        assert_eq!(
            select_host(&hosts, Some("Two")).unwrap_err(),
            "Two is not paired in Moonlight. Pair it in Moonlight first."
        );
        assert_eq!(select_host(&hosts, Some("One")).unwrap().name, "One");
        assert!(select_host(&hosts, None).unwrap_err().starts_with("Choose"));
        assert!(select_host(&[], None).is_err());
    }

    #[test]
    fn published_name_only_returns_managed_entries() {
        let apps = vec![publish_app(&game(1, "One", Some("one.exe")), None).unwrap()];
        let (root, _) = merge_apps(Some(USER_APPS), &apps, "srv_1", true).unwrap();
        let text = render_apps(&root).unwrap();
        assert_eq!(
            published_name(Some(&text), 1).unwrap().as_deref(),
            Some("One")
        );
        assert_eq!(published_name(Some(&text), 2).unwrap(), None);
        assert_eq!(published_name(None, 1).unwrap(), None);
    }

    #[test]
    fn cover_cache_is_keyed_by_game_id() {
        let dir = temp_dir("covers");
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let path = store_cover(&dir, 42, &png).expect("stored");
        assert!(path.ends_with("42.png"));
        assert_eq!(cached_cover(&dir, 42).as_deref(), Some(path.as_str()));
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
    fn sunshine_status_reports_empty_and_malformed_files_differently() {
        let dir = temp_dir("status-empty");
        std::fs::write(dir.join("sunshine.conf"), "").unwrap();
        std::fs::write(dir.join("apps.json"), "   \n\t").unwrap();
        let settings = LocalSettings {
            sunshine_apps_path: dir.join("apps.json").to_string_lossy().to_string(),
            sunshine_conf_path: dir.join("sunshine.conf").to_string_lossy().to_string(),
            ..LocalSettings::default()
        };

        let status = sunshine_status(&settings);
        assert!(status.apps_found);
        assert_eq!(status.entries, 0);
        assert!(!status.message.contains("not valid JSON"));

        std::fs::write(dir.join("apps.json"), "{not json").unwrap();
        let status = sunshine_status(&settings);
        assert!(status.message.contains("not valid JSON"));
        assert!(status.message.contains("Nothing was changed"));
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

    #[test]
    fn apps_json_round_trips_utf8_bom_and_utf16_encodings() {
        let root = temp_dir("apps-encodings");
        let text = "{\"apps\":[{\"name\":\"Café\"}]}\n";
        let mut utf8_bom = vec![0xEF, 0xBB, 0xBF];
        utf8_bom.extend_from_slice(text.as_bytes());
        let mut utf16le = vec![0xFF, 0xFE];
        let mut utf16be = vec![0xFE, 0xFF];
        for unit in text.encode_utf16() {
            utf16le.extend_from_slice(&unit.to_le_bytes());
            utf16be.extend_from_slice(&unit.to_be_bytes());
        }

        for (name, bytes, encoding) in [
            ("bom.json", utf8_bom, TextEncoding::Utf8),
            ("le.json", utf16le, TextEncoding::Utf16Le),
            ("be.json", utf16be, TextEncoding::Utf16Be),
        ] {
            let path = root.join(name);
            std::fs::write(&path, &bytes).unwrap();
            let file = read_text_file(&path).unwrap();
            assert_eq!(file.text, text, "{name} decodes");
            assert_eq!(
                file.format.encoding, encoding,
                "{name} detects its encoding"
            );

            let backup = write_apps_with_backup(&path, &file.text, file.format)
                .unwrap()
                .expect("backup path");
            assert_eq!(
                std::fs::read(&path).unwrap(),
                bytes,
                "{name} keeps its encoding"
            );
            assert_eq!(
                std::fs::read(&backup).unwrap(),
                bytes,
                "{name} backup is byte-exact"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bom_prefixed_malformed_json_is_still_refused() {
        let error = parse_apps_json("\u{feff}{not json").unwrap_err();
        assert!(error.contains("not valid JSON"));
        assert!(error.contains("Nothing was changed"));
    }

    #[test]
    fn program_files_write_failures_are_explained() {
        let denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
        let message = write_failure_message(
            Path::new("C:\\Program Files\\Sunshine\\config\\apps.json"),
            &denied,
        );
        assert!(message.contains("Program Files"));
        assert!(message.contains("user-writable"));
        assert!(message.contains("elevated"));
        assert!(message.contains("Nothing was changed"));

        let message = write_failure_message(Path::new("/opt/sunshine/apps.json"), &denied);
        assert!(message.contains("elevated"));

        let missing = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let message = write_failure_message(Path::new("/opt/sunshine/apps.json"), &missing);
        assert!(!message.contains("elevated"));
        assert!(message.contains("Nothing was changed"));
    }

    fn program_files_roots_fixture() -> Vec<PathBuf> {
        vec![
            PathBuf::from("C:\\Program Files"),
            PathBuf::from("C:\\Program Files (x86)"),
        ]
    }

    #[test]
    fn program_files_roots_are_read_case_insensitively() {
        let mut env = BTreeMap::new();
        env.insert("PROGRAMFILES".to_string(), "C:\\Program Files".to_string());
        env.insert(
            "programfiles(x86)".to_string(),
            "C:\\Program Files (x86)".to_string(),
        );
        assert_eq!(
            program_files_roots(&env),
            vec![
                PathBuf::from("C:\\Program Files"),
                PathBuf::from("C:\\Program Files (x86)")
            ]
        );

        let mut empty = BTreeMap::new();
        empty.insert("ProgramFiles".to_string(), "  ".to_string());
        assert!(program_files_roots(&empty).is_empty());
    }

    #[test]
    fn path_containment_is_case_insensitive_and_rejects_prefix_traps() {
        let roots = program_files_roots_fixture();
        assert!(path_under_roots(
            Path::new("C:\\Program Files\\Sunshine\\config\\apps.json"),
            &roots
        ));
        assert!(path_under_roots(
            Path::new("c:/program files (x86)/Sunshine/config/apps.json"),
            &roots
        ));
        // `Program FilesEvil` starts with the same text but is another folder.
        assert!(!path_under_roots(
            Path::new("C:\\Program FilesEvil\\Sunshine\\config\\apps.json"),
            &roots
        ));
        assert!(!path_under_roots(
            Path::new("C:\\ProgramData\\Sunshine"),
            &roots
        ));
    }

    #[test]
    fn needs_elevation_only_follows_denials_under_program_files() {
        let roots = program_files_roots_fixture();
        let denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
        let missing = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");

        assert!(needs_elevation_at(
            Path::new("C:\\Program Files\\Sunshine\\config\\apps.json"),
            &denied,
            &roots
        ));
        assert!(needs_elevation_at(
            Path::new("C:\\Program Files (x86)\\Sunshine\\config\\apps.json"),
            &denied,
            &roots
        ));
        assert!(!needs_elevation_at(
            Path::new("C:\\Program Files\\Sunshine\\config\\apps.json"),
            &missing,
            &roots
        ));
        assert!(!needs_elevation_at(
            Path::new("C:\\Users\\me\\AppData\\Local\\Sunshine\\apps.json"),
            &denied,
            &roots
        ));
        assert!(!needs_elevation_at(
            Path::new("/opt/sunshine/apps.json"),
            &denied,
            &roots
        ));
    }

    #[test]
    fn the_write_failure_flag_reaches_the_sync_error() {
        let failure = WriteFailure {
            message: "Could not back up C:\\Program Files\\Sunshine\\config\\apps.json."
                .to_string(),
            needs_elevation: true,
        };
        let error = SunshineError::write_failure(failure.clone());
        assert!(error.needs_elevation);
        assert_eq!(error.code, "sunshine_needs_elevation");
        assert_eq!(error.message, failure.message);

        let plain = SunshineError::write_failure(WriteFailure::plain("disk full"));
        assert!(!plain.needs_elevation);
        assert_eq!(plain.code, "sunshine_write_failed");
        assert_eq!(
            SunshineError::from("no folder".to_string()).code,
            "sunshine_error"
        );

        // The web branch reads the camelCase field, not the message (#182).
        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["needsElevation"], Value::Bool(true));
        assert_eq!(json["code"], "sunshine_needs_elevation");
    }

    #[test]
    fn program_files_grant_validation_accepts_only_its_own_tree() {
        let root = temp_dir("grant-validation");
        let program_files = root.join("Program Files");
        let config = program_files.join("Sunshine").join("config");
        std::fs::create_dir_all(&config).unwrap();
        let elsewhere = root.join("Games").join("Sunshine");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let other_arch = root.join("Program Files (x86)").join("Sunshine");
        std::fs::create_dir_all(&other_arch).unwrap();
        let roots = vec![program_files.clone()];

        let accepted = validate_program_files_dir(&config, &roots).unwrap();
        assert_eq!(accepted, std::fs::canonicalize(&config).unwrap());
        assert!(validate_program_files_dir(&elsewhere, &roots).is_err());
        // The other architecture is a separate root, not a child of this one.
        assert!(validate_program_files_dir(&other_arch, &roots).is_err());
        // `..` traversal resolves outside Program Files and is refused.
        let escape = program_files
            .join("Sunshine")
            .join("..")
            .join("..")
            .join("Games");
        assert!(validate_program_files_dir(&escape, &roots).is_err());
        // Missing folders are refused instead of being created.
        assert!(validate_program_files_dir(&root.join("missing"), &roots).is_err());

        let both = vec![program_files, root.join("Program Files (x86)")];
        assert!(validate_program_files_dir(&other_arch, &both).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_grant_plan_comes_from_the_detected_sunshine_config() {
        let root = temp_dir("grant-plan");
        let program_files = root.join("Program Files");
        let config = program_files.join("Sunshine").join("config");
        std::fs::create_dir_all(&config).unwrap();
        let apps = config.join("apps.json");
        let mut env = BTreeMap::new();
        env.insert(
            "ProgramFiles".to_string(),
            program_files.to_string_lossy().to_string(),
        );
        let settings = LocalSettings {
            sunshine_apps_path: apps.to_string_lossy().to_string(),
            ..LocalSettings::default()
        };

        let plan = sunshine_grant_plan_for(&settings, "windows", None, &env).unwrap();
        assert_eq!(plan, config);

        let missing = LocalSettings::default();
        assert!(sunshine_grant_plan_for(&missing, "linux", None, &BTreeMap::new()).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_grant_script_grants_modify_rights_to_the_current_identity() {
        let script = windows_grant_script(Path::new("C:\\Program Files\\Sunshine\\config"));
        assert!(script.contains("Start-Process"));
        assert!(script.contains("icacls.exe"));
        assert!(script.contains("-Verb RunAs"));
        assert!(script.contains("-Wait"));
        assert!(script.contains("-PassThru"));
        assert!(script.contains("[System.Security.Principal.WindowsIdentity]::GetCurrent().Name"));
        assert!(script.contains("'/grant'"));
        assert!(script.contains(":(OI)(CI)M"));
        assert!(script.contains("C:\\Program Files\\Sunshine\\config"));
        assert!(script.contains("exit 1223"));
        // A single quote in the path is doubled for the PowerShell literal.
        let quoted = windows_grant_script(Path::new("C:\\Program Files\\O'Brien\\config"));
        assert!(quoted.contains("O''Brien"));
    }

    #[test]
    fn encoded_commands_are_utf16le_base64() {
        assert_eq!(encode_powershell("A"), "QQA=");
        assert_eq!(encode_powershell("AB"), "QQBCAA==");
        let script = windows_grant_script(Path::new("C:\\Program Files\\Sunshine\\config"));
        let encoded = encode_powershell(&script);
        let utf16_bytes = script.encode_utf16().count() * 2;
        assert_eq!(encoded.len(), (utf16_bytes + 2) / 3 * 4);
        assert!(encoded
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=')));
    }

    #[cfg(not(windows))]
    #[test]
    fn granting_write_access_is_unsupported_off_windows() {
        let error = grant_sunshine_write_access(&LocalSettings::default()).unwrap_err();
        assert!(error.contains("only available on Windows"));
        assert!(error.contains("user-writable"));
    }

    #[test]
    fn grant_outcomes_map_to_granted_cancelled_and_failed() {
        let dir = Path::new("C:\\Program Files\\Sunshine\\config");
        let apps = Path::new("C:\\Program Files\\Sunshine\\config\\apps.json");

        let cancelled = grant_outcome(Some(1223), "", dir, apps, false);
        assert_eq!(cancelled.status, "cancelled");
        assert!(cancelled.message.contains("nothing was changed"));

        let failed = grant_outcome(Some(5), "Access is denied.", dir, apps, false);
        assert_eq!(failed.status, "failed");
        assert!(failed.message.contains("Access is denied"));

        let unchanged = grant_outcome(Some(0), "", dir, apps, false);
        assert_eq!(unchanged.status, "failed");
        assert!(unchanged.message.contains("still not writable"));

        let granted = grant_outcome(Some(0), "", dir, apps, true);
        assert_eq!(granted.status, "granted");
        assert_eq!(
            granted.path.as_deref(),
            Some("C:\\Program Files\\Sunshine\\config")
        );
        assert!(granted.message.contains("Retry the publish"));
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_sunshine_folder_reports_the_permission_error() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        // Permission bits do not apply to root; skip instead of asserting nothing.
        if std::fs::metadata("/")
            .map(|meta| meta.uid() == 0)
            .unwrap_or(false)
        {
            eprintln!("skipping read-only folder test: running as root");
            return;
        }
        let root = temp_dir("readonly");
        let path = root.join("apps.json");
        std::fs::write(&path, "{\"apps\":[]}").unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();

        let error =
            write_apps_with_backup(&path, "{\"apps\":[]}\n", TextFormat::default()).unwrap_err();
        assert!(
            error.to_lowercase().contains("permission") || error.to_lowercase().contains("denied"),
            "unexpected error: {error}"
        );
        assert!(error.contains("Nothing was changed"));

        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    fn fixture_exe(root: &Path) -> String {
        let exe = root.join("Moonlight.exe");
        std::fs::write(&exe, b"").unwrap();
        exe.to_string_lossy().to_string()
    }

    fn console(name: &str) -> ConsoleEntry {
        ConsoleEntry {
            name: name.to_string(),
            cmd: "/usr/bin/ludex --console".to_string(),
            working_dir: "/usr/bin".to_string(),
            image_path: None,
        }
    }

    #[test]
    fn console_entry_is_marked_and_detached() {
        let entry = console("Ludex");
        let app = generated_console_app(&entry, "srv_1");
        assert_eq!(as_string(app.get("name")).as_deref(), Some("Ludex"));
        assert_eq!(
            as_string(app.get("cmd")).as_deref(),
            Some("/usr/bin/ludex --console")
        );
        assert_eq!(as_string(app.get("detached")).as_deref(), Some("true"));
        assert!(is_managed_app(&app));
        assert!(is_console_app(&app));
        assert_eq!(managed_game_id(&app), None);
        assert_eq!(
            app.get(MANAGED_KEY)
                .and_then(|marker| marker.get(CONSOLE_MARKER))
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn console_mode_merge_replaces_per_game_entries_and_is_idempotent() {
        let games = vec![
            publish_app(&game(1, "One", Some("one.exe")), None).unwrap(),
            publish_app(&game(2, "Two", Some("two.exe")), None).unwrap(),
        ];
        let (root, first) = merge_apps(Some(USER_APPS), &games, "srv_1", true).unwrap();
        assert_eq!(first.added, 2);

        let (root, migrated) = merge_entries(
            Some(&render_apps(&root).unwrap()),
            &[],
            Some(&console("Ludex")),
            "srv_1",
            true,
        )
        .unwrap();
        assert_eq!(migrated.added, 1);
        assert_eq!(migrated.removed, 2);
        assert_eq!(migrated.managed_entries, 1);
        assert!(migrated.changed);

        let list = apps_ref(&root).unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list.iter().filter(|app| is_console_app(app)).count(), 1);
        assert!(!list.iter().any(|app| managed_game_id(app).is_some()));
        // User apps survive the migration.
        assert!(list
            .iter()
            .any(|app| as_string(app.get("name")).as_deref() == Some("Desktop")));

        let text = render_apps(&root).unwrap();
        let (root, second) =
            merge_entries(Some(&text), &[], Some(&console("Ludex")), "srv_1", true).unwrap();
        assert!(!second.changed);
        assert_eq!(second.unchanged, 1);
        assert_eq!(second.added, 0);
        assert_eq!(second.removed, 0);
        assert_eq!(second.managed_entries, 1);
        assert_eq!(render_apps(&root).unwrap(), text);
    }

    #[test]
    fn console_merge_collapses_duplicate_console_entries() {
        let duplicate = r#"{"apps":[
          {"name":"Ludex","cmd":"/usr/bin/ludex --console","ludex":{"managed":true,"version":1,"console":true,"serverId":"srv_1"}},
          {"name":"Ludex (copy)","cmd":"/usr/bin/ludex --console","ludex":{"managed":true,"version":1,"console":true,"serverId":"srv_1"}}
        ]}"#;
        let (root, report) =
            merge_entries(Some(duplicate), &[], Some(&console("Ludex")), "srv_1", true).unwrap();
        assert_eq!(report.removed, 1);
        assert_eq!(report.managed_entries, 1);
        let list = apps_ref(&root).unwrap();
        assert_eq!(list.iter().filter(|app| is_console_app(app)).count(), 1);
        assert_eq!(as_string(list[0].get("name")).as_deref(), Some("Ludex"));
    }

    #[test]
    fn console_entry_updates_in_place_when_the_name_changes() {
        let (root, _) =
            merge_entries(Some(USER_APPS), &[], Some(&console("Ludex")), "srv_1", true).unwrap();
        let text = render_apps(&root).unwrap();

        let (root, report) =
            merge_entries(Some(&text), &[], Some(&console("Saloon")), "srv_1", true).unwrap();
        assert_eq!(report.updated, 1);
        assert_eq!(report.added, 0);
        let list = apps_ref(&root).unwrap();
        assert_eq!(list.iter().filter(|app| is_console_app(app)).count(), 1);
        assert_eq!(
            list.iter()
                .find(|app| is_console_app(app))
                .and_then(|app| as_string(app.get("name")))
                .as_deref(),
            Some("Saloon")
        );
    }

    #[test]
    fn going_back_to_per_game_publishing_removes_the_console_entry() {
        let (root, _) =
            merge_entries(Some(USER_APPS), &[], Some(&console("Ludex")), "srv_1", true).unwrap();
        let text = render_apps(&root).unwrap();

        let games = vec![publish_app(&game(1, "One", Some("one.exe")), None).unwrap()];
        let (root, report) = merge_apps(Some(&text), &games, "srv_1", true).unwrap();
        assert_eq!(report.removed, 1);
        assert_eq!(report.added, 1);
        assert_eq!(report.managed_entries, 1);
        let list = apps_ref(&root).unwrap();
        assert!(!list.iter().any(|app| is_console_app(app)));
        assert!(list.iter().any(|app| managed_game_id(app) == Some(1)));
    }

    #[test]
    fn console_entry_from_another_server_is_left_alone() {
        let (root, _) = merge_entries(
            Some(USER_APPS),
            &[],
            Some(&console("Ludex")),
            "server-a",
            true,
        )
        .unwrap();
        let text = render_apps(&root).unwrap();

        let (root, report) =
            merge_entries(Some(&text), &[], Some(&console("Saloon")), "server-b", true).unwrap();
        assert_eq!(report.added, 1);
        assert_eq!(report.removed, 0);
        let list = apps_ref(&root).unwrap();
        assert_eq!(list.iter().filter(|app| is_console_app(app)).count(), 2);
        assert_eq!(report.managed_entries, 2);
    }

    #[test]
    fn console_preview_explains_the_migration_before_writing() {
        let games = vec![publish_app(&game(1, "One", Some("one.exe")), None).unwrap()];
        let (root, _) = merge_apps(Some(USER_APPS), &games, "srv_1", true).unwrap();
        let text = render_apps(&root).unwrap();

        let (entries, console_preview, removals) =
            preview_entries(Some(&text), &[], Some(&console("Ludex")), "srv_1").unwrap();
        assert!(entries.is_empty());
        let preview = console_preview.expect("console preview");
        assert!(preview.console);
        assert_eq!(preview.game_id, 0);
        assert_eq!(preview.action, "add");
        assert_eq!(removals.len(), 1);
        assert_eq!(removals[0].name, "One");

        let (root, _) =
            merge_entries(Some(&text), &[], Some(&console("Ludex")), "srv_1", true).unwrap();
        let (_, console_preview, removals) = preview_entries(
            Some(&render_apps(&root).unwrap()),
            &[],
            Some(&console("Ludex")),
            "srv_1",
        )
        .unwrap();
        assert_eq!(console_preview.unwrap().action, "unchanged");
        assert!(removals.is_empty());
    }

    struct FixtureRegistry(Vec<QtSettingEntry>);

    impl MoonlightSettingsSource for FixtureRegistry {
        fn read(&self) -> Result<Vec<QtSettingEntry>, String> {
            Ok(self.0.clone())
        }
    }

    struct FailingRegistry;

    impl MoonlightSettingsSource for FailingRegistry {
        fn read(&self) -> Result<Vec<QtSettingEntry>, String> {
            Err("access denied".to_string())
        }
    }

    fn registry_entry(path: &str, name: &str, value: &str) -> QtSettingEntry {
        QtSettingEntry {
            path: path.to_string(),
            name: name.to_string(),
            value: value.to_string(),
        }
    }

    #[test]
    fn portable_moonlight_ini_next_to_the_binary_is_read() {
        let root = temp_dir("moonlight-portable");
        let exe = fixture_exe(&root);
        std::fs::write(
            root.join("Moonlight.ini"),
            "[hosts]\nLiving-room\\address=192.168.1.20\nLiving-room\\customname=Living Room\n",
        )
        .unwrap();

        let settings = LocalSettings {
            moonlight_binary: exe,
            ..LocalSettings::default()
        };
        let detection = detect_moonlight(
            &settings,
            "windows",
            None,
            &BTreeMap::new(),
            None,
            &NoMoonlightSettings,
        );
        assert!(detection.config_found);
        assert_eq!(
            detection.config_source.as_deref(),
            Some("Moonlight.ini (portable)")
        );
        assert_eq!(detection.hosts.len(), 1);
        assert_eq!(detection.hosts[0].name, "Living Room");
        assert_eq!(detection.hosts[0].address, "192.168.1.20");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn registry_layout_maps_hosts_through_an_injectable_reader() {
        let root = temp_dir("moonlight-registry");
        let settings = LocalSettings {
            moonlight_binary: fixture_exe(&root),
            ..LocalSettings::default()
        };
        let registry = FixtureRegistry(vec![
            registry_entry("hosts\\1", "address", "192.168.1.20"),
            registry_entry("hosts\\1", "customname", "Living Room"),
            registry_entry("hosts\\1", "srvcert", "abc123"),
            registry_entry("hosts\\2", "address", "10.0.0.30"),
            registry_entry("hosts\\2", "pairstatus", "0"),
            registry_entry("", "language", "en"),
        ]);

        let detection = detect_moonlight(
            &settings,
            "windows",
            None,
            &BTreeMap::new(),
            None,
            &registry,
        );
        assert!(detection.config_found);
        assert!(detection
            .config_source
            .as_deref()
            .unwrap_or_default()
            .starts_with("Windows registry"));
        assert_eq!(detection.hosts.len(), 2);
        let living = detection
            .hosts
            .iter()
            .find(|host| host.name == "Living Room")
            .unwrap();
        assert_eq!(living.address, "192.168.1.20");
        assert!(living.paired);
        let second = detection
            .hosts
            .iter()
            .find(|host| host.address == "10.0.0.30")
            .unwrap();
        // A numeric Qt array key falls back to the address, not "2".
        assert_eq!(second.name, "10.0.0.30");
        assert!(!second.paired);
        assert!(detection.message.contains("host(s) found"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reg_query_output_is_parsed_without_running_reg_exe() {
        let output = "\r\nHKEY_CURRENT_USER\\SOFTWARE\\Moonlight Game Streaming Project\\hosts\r\n    size    REG_DWORD    0x1\r\n\r\nHKEY_CURRENT_USER\\SOFTWARE\\Moonlight Game Streaming Project\\hosts\\1\r\n    address    REG_SZ    192.168.1.20\r\n    customname    REG_SZ    Living Room\r\n    srvcert    REG_BINARY    0A0B0C\r\n";
        let entries = parse_reg_query_output(output);
        assert_eq!(entries.len(), 4);
        let host = entries
            .iter()
            .find(|entry| entry.name == "address")
            .unwrap();
        assert_eq!(host.path, "hosts\\1");
        assert_eq!(host.value, "192.168.1.20");

        let hosts = hosts_from_qt_settings(&entries);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "Living Room");
        assert!(hosts[0].paired);
    }

    #[test]
    fn configured_moonlight_without_a_host_asks_for_a_pairing() {
        let root = temp_dir("moonlight-nohost");
        let settings = LocalSettings {
            moonlight_binary: fixture_exe(&root),
            ..LocalSettings::default()
        };
        let registry = FixtureRegistry(vec![registry_entry("", "language", "en")]);

        let detection = detect_moonlight(
            &settings,
            "windows",
            None,
            &BTreeMap::new(),
            None,
            &registry,
        );
        assert!(detection.config_found);
        assert!(detection.hosts.is_empty());
        assert!(detection.message.contains("Pair a Sunshine host"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_absent_registry_key_is_not_an_error() {
        let root = temp_dir("moonlight-absent");
        let settings = LocalSettings {
            moonlight_binary: fixture_exe(&root),
            ..LocalSettings::default()
        };

        let detection = detect_moonlight(
            &settings,
            "windows",
            None,
            &BTreeMap::new(),
            None,
            &FixtureRegistry(Vec::new()),
        );
        assert!(!detection.config_found);
        assert!(detection.hosts.is_empty());
        assert!(detection.message.contains("configuration was not found"));
        assert!(!detection
            .warnings
            .iter()
            .any(|warning| warning.contains("registry")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unreadable_moonlight_config_is_reported_separately() {
        let root = temp_dir("moonlight-invalid");
        let conf = root.join("Moonlight.conf");
        std::fs::write(&conf, [0x00, 0x01, 0x02]).unwrap();
        let settings = LocalSettings {
            moonlight_binary: fixture_exe(&root),
            moonlight_config_path: conf.to_string_lossy().to_string(),
            ..LocalSettings::default()
        };

        let detection = detect_moonlight(
            &settings,
            "windows",
            None,
            &BTreeMap::new(),
            None,
            &NoMoonlightSettings,
        );
        assert!(detection.config_found);
        assert!(detection.hosts.is_empty());
        assert!(detection.message.contains("could not be read"));
        assert!(detection
            .warnings
            .iter()
            .any(|warning| warning.contains("binary")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_registry_read_error_is_a_warning_not_a_crash() {
        let root = temp_dir("moonlight-registry-error");
        let settings = LocalSettings {
            moonlight_binary: fixture_exe(&root),
            ..LocalSettings::default()
        };

        let detection = detect_moonlight(
            &settings,
            "windows",
            None,
            &BTreeMap::new(),
            None,
            &FailingRegistry,
        );
        assert!(detection.hosts.is_empty());
        assert!(detection
            .warnings
            .iter()
            .any(|warning| warning.contains("registry")));
        assert!(detection.message.contains("could not be read"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn linux_moonlight_conf_is_detected_at_the_existing_path() {
        let root = temp_dir("moonlight-linux");
        let home = root.join("home");
        let conf = home
            .join(".config")
            .join("Moonlight Game Streaming Project")
            .join("Moonlight.conf");
        std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
        std::fs::write(&conf, "[hosts]\nSteam Deck\\address=192.168.1.40\n").unwrap();

        let detection = detect_moonlight(
            &LocalSettings::default(),
            "linux",
            Some(&home),
            &BTreeMap::new(),
            None,
            &NoMoonlightSettings,
        );
        assert_eq!(detection.config_path.as_deref(), Some(conf.as_path()));
        assert_eq!(detection.config_source.as_deref(), Some("Moonlight.conf"));
        assert_eq!(detection.hosts.len(), 1);
        assert_eq!(detection.hosts[0].address, "192.168.1.40");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn manual_host_is_the_stream_fallback() {
        let mut status = MoonlightStatus {
            detected: true,
            platform: "windows".to_string(),
            binary: Some("C:\\Moonlight\\Moonlight.exe".to_string()),
            config_path: None,
            config_source: None,
            config_found: false,
            manual_host: Some("192.168.1.50".to_string()),
            hosts: Vec::new(),
            supported: true,
            warnings: Vec::new(),
            message: "No saved Moonlight host was found; the manual host 192.168.1.50 will be used for streaming."
                .to_string(),
        };

        let hosts = stream_hosts(&status);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].address, "192.168.1.50");
        assert!(hosts[0].paired);
        assert_eq!(select_host(&hosts, None).unwrap().address, "192.168.1.50");

        // A host Moonlight saved always comes first, and the manual fallback is
        // still offered when the saved host is unpaired.
        status.hosts = vec![MoonlightHost {
            name: "Saved".to_string(),
            address: "10.0.0.2".to_string(),
            local_address: None,
            remote_address: None,
            manual_address: None,
            paired: true,
        }];
        let hosts = stream_hosts(&status);
        assert_eq!(hosts[0].name, "Saved");
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[1].address, "192.168.1.50");
        assert!(hosts[1].paired);
        assert_eq!(
            select_host(&hosts, Some("192.168.1.50")).unwrap().address,
            "192.168.1.50"
        );
    }
}
