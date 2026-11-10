//! Built-in config adapters (#106): Unreal Engine, GOG, Godot and Unity.
//!
//! Adapters are a read-only detection layer on top of [`crate::game_config`]:
//! they find the engine marker files, map the useful settings to typed fields
//! (resolution, fullscreen, vsync, language, quality) and point each field at
//! the exact file and key. Writes still go through the same backup + diff path;
//! an adapter never edits anything by itself.
//!
//! Several adapters can apply at once: a GOG build of a Unity game is both a
//! GOG package and a Unity player.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::game_config::{self, ConfigDocument};

const MAX_SCAN_FILES: usize = 5_000;
const MAX_SCAN_DEPTH: usize = 6;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterField {
    /// Stable id for the UI (`resolutionWidth`, `fullscreenMode`, …).
    pub id: String,
    pub label: String,
    /// `select`, `toggle`, `number` or `text`.
    pub control: String,
    pub value: Option<String>,
    pub options: Vec<AdapterOption>,
    pub file: Option<String>,
    pub key: Option<String>,
    pub value_type: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterFile {
    pub path: String,
    pub label: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Adapter {
    pub id: String,
    pub label: String,
    pub detected: bool,
    /// Marker that proved the engine (`*.uproject`, `goggame-*.info`, …).
    pub evidence: Option<String>,
    pub files: Vec<AdapterFile>,
    pub fields: Vec<AdapterField>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterReport {
    pub game_id: i64,
    pub install_path: String,
    pub adapters: Vec<Adapter>,
    /// Files that exist but are outside the install folder (registry, user
    /// data) are reported in the adapter notes instead of being edited.
    pub notes: Vec<String>,
}

fn option(value: &str, label: &str) -> AdapterOption {
    AdapterOption {
        value: value.to_string(),
        label: label.to_string(),
    }
}

fn relative(install_root: &Path, path: &Path) -> String {
    path.strip_prefix(install_root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn is_skipped_dir(name: &str) -> bool {
    name.starts_with('.') || name.eq_ignore_ascii_case("drive_c")
}

fn scan_files(install_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk(install_root, 0, &mut files);
    files
}

fn walk(directory: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    if depth > MAX_SCAN_DEPTH || files.len() >= MAX_SCAN_FILES {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if files.len() >= MAX_SCAN_FILES {
            return;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !is_skipped_dir(&name) {
                walk(&path, depth + 1, files);
            }
        } else if file_type.is_file() {
            files.push(path);
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn find_named(files: &[PathBuf], matcher: impl Fn(&str) -> bool) -> Option<PathBuf> {
    files.iter().find(|path| matcher(&file_name(path))).cloned()
}

fn find_basename(files: &[PathBuf], name: &str) -> Option<PathBuf> {
    files
        .iter()
        .find(|path| file_name(path).eq_ignore_ascii_case(name))
        .cloned()
}

fn find_extension(files: &[PathBuf], extension: &str) -> Option<PathBuf> {
    files
        .iter()
        .find(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .map(|value| value.eq_ignore_ascii_case(extension))
                .unwrap_or(false)
        })
        .cloned()
}

fn has_dir(files: &[PathBuf], name: &str) -> bool {
    files.iter().any(|path| {
        path.components().any(|component| {
            component
                .as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(name)
        })
    })
}

fn has_file_suffix(files: &[PathBuf], suffix: &str) -> bool {
    files.iter().any(|path| {
        path.to_string_lossy()
            .replace('\\', "/")
            .to_lowercase()
            .ends_with(suffix)
    })
}

/// Read a config file safely; failures become a note, never an error.
fn read_document(
    install_root: &Path,
    path: &Path,
    relative_path: &str,
    notes: &mut Vec<String>,
) -> Option<ConfigDocument> {
    match game_config::read(install_root, relative_path) {
        Ok(document) => Some(document),
        Err(error) => {
            notes.push(format!("{}: {error}", path.display()));
            None
        }
    }
}

fn entry_value(
    document: &ConfigDocument,
    names: &[&str],
    suffix_separator: char,
) -> Option<(String, String)> {
    document.entries.iter().find_map(|entry| {
        let name = entry
            .key
            .rsplit(suffix_separator)
            .next()
            .unwrap_or(entry.key.as_str());
        if names.contains(&name) {
            Some((entry.key.clone(), entry.value.clone()))
        } else {
            None
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn field(
    id: &str,
    label: &str,
    control: &str,
    file: Option<String>,
    key: Option<String>,
    value: Option<String>,
    value_type: &str,
    options: Vec<AdapterOption>,
    note: Option<String>,
) -> AdapterField {
    AdapterField {
        id: id.to_string(),
        label: label.to_string(),
        control: control.to_string(),
        value,
        options,
        file,
        key,
        value_type: value_type.to_string(),
        note,
    }
}

/* --------------------------------- unreal -------------------------------- */

fn unreal_adapter(install_root: &Path, files: &[PathBuf]) -> Adapter {
    let mut notes = Vec::new();
    let uproject = find_extension(files, "uproject");
    let pak = find_extension(files, "pak");
    let engine_dir = has_dir(files, "Engine");
    let binaries = has_dir(files, "Binaries");
    let marker = uproject
        .as_ref()
        .map(|path| relative(install_root, path))
        .or_else(|| pak.as_ref().map(|path| relative(install_root, path)))
        .or_else(|| engine_dir.then(|| "Engine/".to_string()))
        .or_else(|| binaries.then(|| "Binaries/".to_string()));

    let game_user_settings = find_basename(files, "GameUserSettings.ini");
    let engine_ini = find_basename(files, "Engine.ini");
    let mut adapter_files = Vec::new();
    for (path, label) in [
        (game_user_settings.as_ref(), "GameUserSettings.ini"),
        (engine_ini.as_ref(), "Engine.ini"),
    ] {
        adapter_files.push(AdapterFile {
            path: path
                .map(|path| relative(install_root, path))
                .unwrap_or_else(|| label.to_string()),
            label: label.to_string(),
            exists: path.is_some(),
        });
    }

    let mut fields = Vec::new();
    if let Some(path) = game_user_settings.as_deref() {
        let relative_path = relative(install_root, path);
        if let Some(document) = read_document(install_root, path, &relative_path, &mut notes) {
            let mut numeric = |id: &str, label: &str, names: &[&str]| {
                if let Some((key, value)) = entry_value(&document, names, '/') {
                    fields.push(field(
                        id,
                        label,
                        "number",
                        Some(relative_path.clone()),
                        Some(key),
                        Some(value),
                        "number",
                        Vec::new(),
                        None,
                    ));
                }
            };
            numeric("resolutionWidth", "Resolution width", &["ResolutionSizeX"]);
            numeric(
                "resolutionHeight",
                "Resolution height",
                &["ResolutionSizeY"],
            );
            numeric("frameRateLimit", "Frame rate limit", &["FrameRateLimit"]);

            if let Some((key, value)) = entry_value(&document, &["FullscreenMode"], '/') {
                fields.push(field(
                    "fullscreenMode",
                    "Window mode",
                    "select",
                    Some(relative_path.clone()),
                    Some(key),
                    Some(value),
                    "number",
                    vec![
                        option("0", "Fullscreen"),
                        option("1", "Borderless"),
                        option("2", "Windowed"),
                    ],
                    None,
                ));
            }
            if let Some((key, value)) = entry_value(&document, &["bUseVSync"], '/') {
                fields.push(field(
                    "vsync",
                    "V-Sync",
                    "toggle",
                    Some(relative_path.clone()),
                    Some(key),
                    Some(value),
                    "boolean",
                    Vec::new(),
                    None,
                ));
            }
            notes.push(format!(
                "Editing {relative_path} is safe: Ludex backs it up first. Unreal may rewrite it on exit."
            ));
        }
    } else {
        notes.push(
            "GameUserSettings.ini is not inside the install folder. Unreal writes it to %LOCALAPPDATA%\\<Game>\\Saved\\Config\\ on Windows and ~/.config/Epic/... on Linux; Ludex does not edit files outside the install folder."
                .to_string(),
        );
    }

    if let Some(path) = engine_ini.as_deref() {
        let relative_path = relative(install_root, path);
        if let Some(document) = read_document(install_root, path, &relative_path, &mut notes) {
            if let Some((key, value)) = entry_value(&document, &["Culture"], '/') {
                fields.push(field(
                    "language",
                    "Language",
                    "text",
                    Some(relative_path),
                    Some(key),
                    Some(value),
                    "string",
                    Vec::new(),
                    Some("Unreal culture code, e.g. en, fr, de.".to_string()),
                ));
            }
        }
    }

    Adapter {
        id: "unreal".to_string(),
        label: "Unreal Engine".to_string(),
        detected: marker.is_some(),
        evidence: marker,
        files: adapter_files,
        fields,
        notes,
    }
}

/* ---------------------------------- gog ---------------------------------- */

fn gog_adapter(install_root: &Path, files: &[PathBuf]) -> Adapter {
    let info = find_named(files, |name| {
        name.to_ascii_lowercase().starts_with("goggame-")
            && name.to_ascii_lowercase().ends_with(".info")
    });
    let mut notes = Vec::new();
    let mut fields = Vec::new();
    let mut adapter_files = Vec::new();

    if let Some(path) = info.as_deref() {
        let relative_path = relative(install_root, path);
        adapter_files.push(AdapterFile {
            path: relative_path.clone(),
            label: "GOG game info".to_string(),
            exists: true,
        });
        if let Some(document) = read_document(install_root, path, &relative_path, &mut notes) {
            if let Some(entry) = document
                .entries
                .iter()
                .find(|entry| entry.key == "language")
            {
                let language_options: Vec<AdapterOption> = document
                    .entries
                    .iter()
                    .filter(|entry| entry.key.starts_with("langs["))
                    .map(|entry| option(&entry.value, &entry.value))
                    .collect();
                fields.push(field(
                    "language",
                    "Language",
                    if language_options.is_empty() {
                        "text"
                    } else {
                        "select"
                    },
                    Some(relative_path.clone()),
                    Some(entry.key.clone()),
                    Some(entry.value.clone()),
                    "string",
                    language_options,
                    Some("GOG language code (en, fr, de, …).".to_string()),
                ));
            }
            for (id, label) in [("gameId", "GOG game id"), ("buildId", "GOG build")] {
                if let Some(entry) = document.entries.iter().find(|entry| entry.key == id) {
                    fields.push(field(
                        id,
                        label,
                        "text",
                        None,
                        None,
                        Some(entry.value.clone()),
                        "string",
                        Vec::new(),
                        Some("Read-only metadata from the GOG package.".to_string()),
                    ));
                }
            }
        }
        notes.push(
            "GOG Galaxy also stores these values in the Windows registry (HKCU\\Software\\GOG.com). Ludex edits the portable .info file only."
                .to_string(),
        );
    }

    Adapter {
        id: "gog".to_string(),
        label: "GOG".to_string(),
        detected: info.is_some(),
        evidence: info.map(|path| relative(install_root, &path)),
        files: adapter_files,
        fields,
        notes,
    }
}

/* --------------------------------- godot --------------------------------- */

fn godot_adapter(install_root: &Path, files: &[PathBuf]) -> Adapter {
    let project = find_basename(files, "project.godot");
    let pck = find_extension(files, "pck");
    let mut notes = Vec::new();
    let mut fields = Vec::new();
    let mut adapter_files = Vec::new();
    let evidence = project
        .as_ref()
        .map(|path| relative(install_root, path))
        .or_else(|| pck.as_ref().map(|path| relative(install_root, path)));

    if let Some(path) = project.as_deref() {
        let relative_path = relative(install_root, path);
        adapter_files.push(AdapterFile {
            path: relative_path.clone(),
            label: "project.godot".to_string(),
            exists: true,
        });
        if let Some(document) = read_document(install_root, path, &relative_path, &mut notes) {
            let mut add = |id: &str,
                           label: &str,
                           key: &str,
                           control: &str,
                           options: Vec<AdapterOption>,
                           value_type: &str| {
                if let Some(entry) = document.entries.iter().find(|entry| entry.key == key) {
                    fields.push(field(
                        id,
                        label,
                        control,
                        Some(relative_path.clone()),
                        Some(entry.key.clone()),
                        Some(entry.value.clone()),
                        value_type,
                        options,
                        None,
                    ));
                }
            };
            add(
                "resolutionWidth",
                "Resolution width",
                "display/window/size/viewport_width",
                "number",
                Vec::new(),
                "number",
            );
            add(
                "resolutionHeight",
                "Resolution height",
                "display/window/size/viewport_height",
                "number",
                Vec::new(),
                "number",
            );
            add(
                "fullscreenMode",
                "Window mode",
                "display/window/size/mode",
                "select",
                vec![
                    option("0", "Windowed"),
                    option("1", "Minimized"),
                    option("2", "Maximized"),
                    option("3", "Fullscreen"),
                    option("4", "Exclusive fullscreen"),
                ],
                "number",
            );
            add(
                "vsync",
                "V-Sync",
                "display/window/vsync/vsync_mode",
                "select",
                vec![
                    option("0", "Disabled"),
                    option("1", "Enabled"),
                    option("2", "Adaptive"),
                    option("3", "Mailbox"),
                ],
                "number",
            );
            add(
                "language",
                "Language",
                "internationalization/locale/fallback",
                "text",
                Vec::new(),
                "string",
            );
            add(
                "msaa",
                "Anti-aliasing",
                "rendering/anti_aliasing/quality/msaa_3d",
                "select",
                vec![
                    option("0", "Disabled"),
                    option("1", "2x"),
                    option("2", "4x"),
                    option("3", "8x"),
                ],
                "number",
            );
        }
        notes.push(
            "Godot stores runtime overrides in user:// (outside the install folder); project.godot holds the shipped defaults."
                .to_string(),
        );
    } else if pck.is_some() {
        notes.push(
            "Only the exported .pck is present: the project settings were compiled into the package and cannot be edited safely."
                .to_string(),
        );
    }

    Adapter {
        id: "godot".to_string(),
        label: "Godot".to_string(),
        detected: evidence.is_some(),
        evidence,
        files: adapter_files,
        fields,
        notes,
    }
}

/* --------------------------------- unity --------------------------------- */

fn unity_adapter(install_root: &Path, files: &[PathBuf]) -> Adapter {
    let marker = has_file_suffix(files, "_data/globalgamemanagers")
        .then(|| "*_Data/globalgamemanagers".to_string())
        .or_else(|| {
            files
                .iter()
                .find(|path| file_name(path).eq_ignore_ascii_case("UnityPlayer.dll"))
                .map(|path| relative(install_root, path))
        })
        .or_else(|| {
            find_named(files, |name| name.eq_ignore_ascii_case("prefs"))
                .map(|path| relative(install_root, &path))
        });

    let prefs = find_named(files, |name| name.eq_ignore_ascii_case("prefs"));
    let mut notes = Vec::new();
    let mut fields = Vec::new();
    let mut adapter_files = Vec::new();

    if let Some(path) = prefs.as_deref() {
        let relative_path = relative(install_root, path);
        adapter_files.push(AdapterFile {
            path: relative_path.clone(),
            label: "Unity player prefs".to_string(),
            exists: true,
        });
        if let Some(document) = read_document(install_root, path, &relative_path, &mut notes) {
            let find = |prefix: &str| {
                document.entries.iter().find(|entry| {
                    let name = entry.key.rsplit('/').next().unwrap_or(&entry.key);
                    name.split("_h")
                        .next()
                        .map(|base| base.eq_ignore_ascii_case(prefix))
                        .unwrap_or(false)
                })
            };
            let mut add = |id: &str,
                           label: &str,
                           prefix: &str,
                           control: &str,
                           options: Vec<AdapterOption>,
                           value_type: &str,
                           note: Option<String>| {
                if let Some(entry) = find(prefix) {
                    fields.push(field(
                        id,
                        label,
                        control,
                        Some(relative_path.clone()),
                        Some(entry.key.clone()),
                        Some(entry.value.clone()),
                        value_type,
                        options,
                        note,
                    ));
                }
            };
            add(
                "resolutionWidth",
                "Resolution width",
                "Screenmanager Resolution Width",
                "number",
                Vec::new(),
                "number",
                None,
            );
            add(
                "resolutionHeight",
                "Resolution height",
                "Screenmanager Resolution Height",
                "number",
                Vec::new(),
                "number",
                None,
            );
            add(
                "fullscreenMode",
                "Window mode",
                "Screenmanager Fullscreen mode",
                "select",
                vec![
                    option("0", "Exclusive fullscreen"),
                    option("1", "Fullscreen window"),
                    option("2", "Maximized window"),
                    option("3", "Windowed"),
                ],
                "number",
                None,
            );
            add(
                "vsync",
                "V-Sync",
                "QualitySettings vSync Count",
                "select",
                vec![
                    option("0", "Off"),
                    option("1", "Every v-blank"),
                    option("2", "Every second v-blank"),
                ],
                "number",
                None,
            );
            add(
                "graphicsQuality",
                "Graphics quality",
                "UnityGraphicsQuality",
                "number",
                Vec::new(),
                "number",
                Some("Unity quality level index (0 is the lowest).".to_string()),
            );
        }
        notes.push(
            "Unity player prefs live in the Windows registry (HKCU\\Software\\<Company>\\<Product>) on Windows; the .prefs file is used on Linux/macOS."
                .to_string(),
        );
    } else if marker.is_some() {
        notes.push(
            "Unity settings are stored in the Windows registry or ~/.config/unity3d/<Company>/<Product>/prefs, outside the install folder; Ludex shows them as read-only."
                .to_string(),
        );
    }

    Adapter {
        id: "unity".to_string(),
        label: "Unity".to_string(),
        detected: marker.is_some(),
        evidence: marker,
        files: adapter_files,
        fields,
        notes,
    }
}

/// Detect every adapter that applies to an install and build its typed fields.
pub fn report(game_id: i64, install_root: &Path) -> AdapterReport {
    let files = scan_files(install_root);
    let adapters = vec![
        unreal_adapter(install_root, &files),
        gog_adapter(install_root, &files),
        godot_adapter(install_root, &files),
        unity_adapter(install_root, &files),
    ];
    AdapterReport {
        game_id,
        install_path: install_root.to_string_lossy().to_string(),
        adapters,
        notes: vec![
            "Adapters never edit on their own: every change goes through the backup + diff path (#105)."
                .to_string(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ludex-adapter-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn adapter<'a>(report: &'a AdapterReport, id: &str) -> &'a Adapter {
        report
            .adapters
            .iter()
            .find(|adapter| adapter.id == id)
            .unwrap()
    }

    #[test]
    fn detects_unreal_and_maps_game_user_settings() {
        let root = temp_dir("unreal");
        write(&root, "Game/Game.uproject", "{}");
        write(
            &root,
            "Game/Saved/Config/Windows/GameUserSettings.ini",
            "[/Script/Engine.GameUserSettings]\nResolutionSizeX=1920\nResolutionSizeY=1080\nFullscreenMode=1\nbUseVSync=True\nFrameRateLimit=144.000000\n[ScalabilityGroups]\nsg.ResolutionQuality=100\n",
        );
        write(
            &root,
            "Game/Saved/Config/Windows/Engine.ini",
            "[Internationalization]\nCulture=fr\n",
        );

        let report = report(7, &root);
        let unreal = adapter(&report, "unreal");
        assert!(unreal.detected);
        assert!(unreal.evidence.is_some());
        let width = unreal
            .fields
            .iter()
            .find(|field| field.id == "resolutionWidth")
            .unwrap();
        assert_eq!(width.value.as_deref(), Some("1920"));
        assert_eq!(width.control, "number");
        assert!(width
            .file
            .as_deref()
            .unwrap()
            .ends_with("GameUserSettings.ini"));
        let mode = unreal
            .fields
            .iter()
            .find(|field| field.id == "fullscreenMode")
            .unwrap();
        assert_eq!(mode.control, "select");
        assert_eq!(mode.options.len(), 3);
        let vsync = unreal
            .fields
            .iter()
            .find(|field| field.id == "vsync")
            .unwrap();
        assert_eq!(vsync.control, "toggle");
        assert_eq!(vsync.value.as_deref(), Some("True"));
        let language = unreal
            .fields
            .iter()
            .find(|field| field.id == "language")
            .unwrap();
        assert_eq!(language.value.as_deref(), Some("fr"));
    }

    #[test]
    fn detects_gog_info_and_maps_language() {
        let root = temp_dir("gog");
        write(
            &root,
            "goggame-1234.info",
            "{\"gameId\": \"1234\", \"language\": \"en\", \"buildId\": \"5678\"}",
        );

        let report = report(3, &root);
        let gog = adapter(&report, "gog");
        assert!(gog.detected);
        assert_eq!(gog.evidence.as_deref(), Some("goggame-1234.info"));
        let language = gog
            .fields
            .iter()
            .find(|field| field.id == "language")
            .unwrap();
        assert_eq!(language.value.as_deref(), Some("en"));
        assert_eq!(language.key.as_deref(), Some("language"));
        assert_eq!(language.file.as_deref(), Some("goggame-1234.info"));
    }

    #[test]
    fn detects_godot_project_and_maps_display_fields() {
        let root = temp_dir("godot");
        write(
            &root,
            "project.godot",
            "; engine config\n[display]\nwindow/size/viewport_width=1280\nwindow/size/viewport_height=720\nwindow/size/mode=0\nwindow/vsync/vsync_mode=1\n",
        );

        let report = report(5, &root);
        let godot = adapter(&report, "godot");
        assert!(godot.detected);
        let mode = godot
            .fields
            .iter()
            .find(|field| field.id == "fullscreenMode")
            .unwrap();
        assert_eq!(mode.value.as_deref(), Some("0"));
        assert_eq!(mode.control, "select");
        let vsync = godot
            .fields
            .iter()
            .find(|field| field.id == "vsync")
            .unwrap();
        assert_eq!(
            vsync.key.as_deref(),
            Some("display/window/vsync/vsync_mode")
        );
    }

    #[test]
    fn detects_unity_data_dir_and_maps_prefs() {
        let root = temp_dir("unity");
        write(&root, "Game_Data/globalgamemanagers", "binary");
        write(
            &root,
            "prefs",
            "Screenmanager Resolution Width_h182942802=1920\nScreenmanager Resolution Height_h2627697771=1080\nScreenmanager Fullscreen mode_h3630240806=3\nQualitySettings vSync Count_h2012106271=1\nUnityGraphicsQuality_h1669003810=2\n",
        );

        let report = report(8, &root);
        let unity = adapter(&report, "unity");
        assert!(unity.detected);
        let width = unity
            .fields
            .iter()
            .find(|field| field.id == "resolutionWidth")
            .unwrap();
        assert_eq!(width.value.as_deref(), Some("1920"));
        assert_eq!(
            width.key.as_deref(),
            Some("Screenmanager Resolution Width_h182942802")
        );
        let mode = unity
            .fields
            .iter()
            .find(|field| field.id == "fullscreenMode")
            .unwrap();
        assert_eq!(mode.options.len(), 4);
    }

    #[test]
    fn no_markers_means_no_adapters_detected() {
        let root = temp_dir("plain");
        write(&root, "game.exe", "MZ");
        let report = report(1, &root);
        assert!(report.adapters.iter().all(|adapter| !adapter.detected));
        assert!(report
            .adapters
            .iter()
            .all(|adapter| adapter.fields.is_empty()));
    }

    #[test]
    fn detection_ignores_wine_prefixes() {
        let root = temp_dir("prefix");
        write(
            &root,
            ".prefix/drive_c/Game/Saved/Config/Windows/GameUserSettings.ini",
            "[/Script/Engine.GameUserSettings]\nResolutionSizeX=800\n",
        );
        let report = report(2, &root);
        assert!(!adapter(&report, "unreal").detected);
    }
}
