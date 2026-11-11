//! Emulator registry and ROM launching for the desktop app (#52).
//!
//! The registry is a small JSON document in `{appDataDir}/emulators.json`. It
//! is user-editable, ships only as presets and never contains an emulator
//! binary. A launch always goes through
//! [`ludex_core::rom::build_launch_command`] and `std::process::Command`, so a
//! command template is never handed to a shell.
//!
//! Sessions recorded here reuse the desktop `sessions` table, which means ROM
//! playtime appears next to installed games and is synced like any other
//! session.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use ludex_core::rom::{build_launch_command, LaunchCommand, RomError};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::launch;
use crate::state::{AppState, ProcessEntry};
use crate::util::now_rfc3339;

/// One configured emulator.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RomEmulator {
    /// Stable id (`retroarch-snes`, `dolphin`, …).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Command template containing `%ROM%`.
    pub command: String,
    /// Platform slugs this emulator handles.
    #[serde(default)]
    pub platforms: Vec<String>,
    /// Whether the emulator can be picked.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Built-in preset, kept for the UI badge.
    #[serde(default)]
    pub builtin: bool,
}

const fn default_enabled() -> bool {
    true
}

/// The persisted registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RomRegistry {
    /// Configured emulators.
    #[serde(default)]
    pub emulators: Vec<RomEmulator>,
    /// Default emulator id per platform.
    #[serde(default)]
    pub defaults: BTreeMap<String, String>,
}

impl Default for RomRegistry {
    fn default() -> Self {
        Self {
            emulators: presets(),
            defaults: BTreeMap::new(),
        }
    }
}

/// Built-in presets. Commands are examples: the user points them at their own
/// RetroArch core directory or standalone binary.
#[must_use]
pub fn presets() -> Vec<RomEmulator> {
    let retro = |id: &str, name: &str, core: &str, platforms: &[&str]| RomEmulator {
        id: id.to_string(),
        name: name.to_string(),
        command: format!("retroarch -L {core}_libretro.so %ROM%"),
        platforms: platforms
            .iter()
            .map(|platform| (*platform).to_string())
            .collect(),
        enabled: true,
        builtin: true,
    };
    vec![
        retro("retroarch-snes", "RetroArch · SNES", "snes9x", &["snes"]),
        retro("retroarch-nes", "RetroArch · NES", "mesen", &["nes"]),
        retro(
            "retroarch-gba",
            "RetroArch · Game Boy Advance",
            "mgba",
            &["gba", "gb", "gbc"],
        ),
        retro(
            "retroarch-n64",
            "RetroArch · Nintendo 64",
            "mupen64plus_next",
            &["n64"],
        ),
        retro(
            "retroarch-psx",
            "RetroArch · PlayStation",
            "swanstation",
            &["psx"],
        ),
        retro(
            "retroarch-genesis",
            "RetroArch · Mega Drive",
            "genesis_plus_gx",
            &["genesis", "mastersystem", "gamegear"],
        ),
        retro(
            "retroarch-arcade",
            "RetroArch · Arcade",
            "fbneo",
            &["arcade", "neogeo"],
        ),
        RomEmulator {
            id: "dolphin".to_string(),
            name: "Dolphin".to_string(),
            command: "dolphin-emu -b -e %ROM%".to_string(),
            platforms: vec!["gamecube".to_string(), "wii".to_string()],
            enabled: true,
            builtin: true,
        },
        RomEmulator {
            id: "pcsx2".to_string(),
            name: "PCSX2".to_string(),
            command: "pcsx2 %ROM%".to_string(),
            platforms: vec!["ps2".to_string()],
            enabled: true,
            builtin: true,
        },
        RomEmulator {
            id: "ppsspp".to_string(),
            name: "PPSSPP".to_string(),
            command: "ppsspp %ROM%".to_string(),
            platforms: vec!["psp".to_string()],
            enabled: true,
            builtin: true,
        },
        RomEmulator {
            id: "snes9x".to_string(),
            name: "Snes9x".to_string(),
            command: "snes9x %ROM%".to_string(),
            platforms: vec!["snes".to_string()],
            enabled: true,
            builtin: true,
        },
    ]
}

/// Returns the registry path inside the application data directory.
#[must_use]
pub fn registry_path(app_data: &Path) -> PathBuf {
    app_data.join("emulators.json")
}

/// Loads the registry, falling back to presets when the file is missing or
/// invalid. A corrupt file is preserved next to the new one.
pub fn load(app_data: &Path) -> RomRegistry {
    let path = registry_path(app_data);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return RomRegistry::default();
    };
    match serde_json::from_str::<RomRegistry>(&text) {
        Ok(mut registry) => {
            normalize(&mut registry);
            registry
        }
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "invalid emulator registry; using presets");
            let _ = std::fs::rename(&path, path.with_extension("json.invalid"));
            RomRegistry::default()
        }
    }
}

/// Saves the registry atomically.
pub fn save(app_data: &Path, registry: &RomRegistry) -> Result<(), String> {
    std::fs::create_dir_all(app_data)
        .map_err(|error| format!("Could not create the data folder: {error}"))?;
    let path = registry_path(app_data);
    let temporary = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(registry)
        .map_err(|error| format!("Could not serialise the emulator registry: {error}"))?;
    std::fs::write(&temporary, text)
        .map_err(|error| format!("Could not write the emulator registry: {error}"))?;
    std::fs::rename(&temporary, &path)
        .map_err(|error| format!("Could not replace the emulator registry: {error}"))
}

/// Validates a registry: unique ids, valid platform slugs and launchable
/// commands. Invalid emulators are dropped rather than breaking every launch.
pub fn normalize(registry: &mut RomRegistry) {
    let mut seen = std::collections::HashSet::new();
    registry.emulators.retain(|emulator| {
        let id = emulator.id.trim();
        if id.is_empty() || !seen.insert(id.to_ascii_lowercase()) {
            return false;
        }
        if emulator.command.contains(';') || emulator.command.contains("&&") {
            return false;
        }
        validate_command(&emulator.command).is_ok()
    });
    let known: std::collections::HashSet<String> = registry
        .emulators
        .iter()
        .map(|emulator| emulator.id.clone())
        .collect();
    registry.defaults.retain(|platform, emulator| {
        known.contains(emulator) && platform_by_id(platform).is_some()
    });
    if registry.emulators.is_empty() {
        registry.emulators = presets();
    }
}

fn platform_by_id(id: &str) -> Option<&'static ludex_core::rom::Platform> {
    ludex_core::rom::platform(id)
}

/// Validates a command template against a synthetic ROM path.
pub fn validate_command(command: &str) -> Result<LaunchCommand, String> {
    build_launch_command(command, Path::new("/roms/sample.sfc")).map_err(|error| error.to_string())
}

/// Picks the emulator for a platform, in order: explicit id, platform default,
/// first enabled emulator supporting the platform.
#[must_use]
pub fn resolve<'a>(
    registry: &'a RomRegistry,
    platform: &str,
    emulator_id: Option<&str>,
) -> Option<&'a RomEmulator> {
    if let Some(id) = emulator_id {
        if let Some(emulator) = registry
            .emulators
            .iter()
            .find(|emulator| emulator.id == id && emulator.enabled)
        {
            return Some(emulator);
        }
    }
    if let Some(id) = registry.defaults.get(platform) {
        if let Some(emulator) = registry
            .emulators
            .iter()
            .find(|emulator| emulator.id == *id && emulator.enabled)
        {
            return Some(emulator);
        }
    }
    registry.emulators.iter().find(|emulator| {
        emulator.enabled && emulator.platforms.iter().any(|value| value == platform)
    })
}

/// Result returned to the web UI after a ROM launch.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RomLaunchResult {
    /// Play-session id, usable with `stop_game`.
    pub session_id: String,
    /// Emulator display name.
    pub emulator_name: String,
    /// Emulator id.
    pub emulator_id: String,
}

/// Registries used by tests and by commands; loaded on demand so an external
/// edit of `emulators.json` is picked up without restarting the app.
fn registry(app: &AppHandle) -> RomRegistry {
    load(&app.state::<AppState>().app_data)
}

/// Launches a ROM through the resolved emulator and starts a play session.
pub fn launch_rom(
    app: &AppHandle,
    game_id: i64,
    platform: &str,
    rom_path: &str,
    emulator_id: Option<&str>,
) -> Result<RomLaunchResult, String> {
    let path = PathBuf::from(rom_path.trim());
    if !path.is_file() {
        return Err(format!("The ROM file does not exist: {}", path.display()));
    }
    let registry = registry(app);
    let emulator = resolve(&registry, platform, emulator_id)
        .ok_or_else(|| format!("No enabled emulator is configured for platform `{platform}`."))?;
    let command = build_launch_command(&emulator.command, &path)
        .map_err(|error| describe_command_error(&error))?;

    let mut process = Command::new(&command.program);
    process
        .args(&command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        process.creation_flags(CREATE_NO_WINDOW);
    }
    let child = process
        .spawn()
        .map_err(|error| format!("Could not start {}: {error}", command.program))?;

    let state = app.state::<AppState>();
    let session_id = Uuid::new_v4().to_string();
    let pid = child.id();
    let started = chrono::Utc::now();
    let entry = Arc::new(ProcessEntry {
        child: std::sync::Mutex::new(child),
        pid,
        game_id,
        started_at: started,
        started: std::time::Instant::now(),
    });
    {
        let mut processes = state
            .processes
            .lock()
            .map_err(|_| "The process list is unavailable; restart Ludex.".to_string())?;
        if processes.values().any(|running| running.game_id == game_id) {
            if let Ok(mut child) = entry.child.lock() {
                let _ = child.kill();
            }
            return Err("This game is already running.".to_string());
        }
        processes.insert(session_id.clone(), Arc::clone(&entry));
    }
    let session_inserted = match state.db.lock() {
        Ok(connection) => {
            crate::db::insert_session(&connection, &session_id, game_id, &now_rfc3339())
        }
        Err(_) => Err("The local database is unavailable; restart Ludex.".to_string()),
    };
    if let Err(error) = session_inserted {
        if let Ok(mut processes) = state.processes.lock() {
            processes.remove(&session_id);
        }
        if let Ok(mut child) = entry.child.lock() {
            let _ = child.kill();
        }
        return Err(error);
    }

    let app_handle = app.clone();
    let session = session_id.clone();
    std::thread::spawn(move || {
        let state = app_handle.state::<AppState>();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            let exited = entry
                .child
                .lock()
                .map(|mut child| child.try_wait().ok().flatten().is_some())
                .unwrap_or(true);
            if exited {
                break;
            }
        }
        let seconds = i64::try_from(entry.started.elapsed().as_secs()).unwrap_or(i64::MAX);
        let minutes = seconds / 60;
        if let Ok(connection) = state.db.lock() {
            let _ =
                crate::db::finish_session(&connection, &session, &now_rfc3339(), minutes, seconds);
        }
        if let Ok(mut processes) = state.processes.lock() {
            processes.remove(&session);
        }
        tracing::info!(game_id, minutes, "ROM session finished");
    });

    Ok(RomLaunchResult {
        session_id,
        emulator_name: emulator.name.clone(),
        emulator_id: emulator.id.clone(),
    })
}

fn describe_command_error(error: &RomError) -> String {
    match error {
        RomError::MissingRomPlaceholder => "The emulator command must contain %ROM%.".to_string(),
        RomError::ShellSyntax(syntax) => format!(
            "The emulator command contains shell syntax (`{syntax}`); Ludex never runs a shell."
        ),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Response for the emulator settings page.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RomSettings {
    /// Registry document.
    pub registry: RomRegistry,
    /// Platform catalog for the picker.
    pub platforms: Vec<RomPlatform>,
}

/// One platform shown in the picker.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RomPlatform {
    /// Platform slug.
    pub id: String,
    /// Human-readable label.
    pub label: String,
}

/// Lists emulators and platforms.
#[tauri::command]
pub fn get_rom_settings(app: AppHandle) -> Result<RomSettings, String> {
    Ok(RomSettings {
        registry: registry(&app),
        platforms: ludex_core::rom::PLATFORMS
            .iter()
            .map(|platform| RomPlatform {
                id: platform.id.to_string(),
                label: platform.label.to_string(),
            })
            .collect(),
    })
}

/// Creates or replaces one emulator.
#[tauri::command]
pub fn save_rom_emulator(app: AppHandle, emulator: RomEmulator) -> Result<RomRegistry, String> {
    if validate_command(&emulator.command).is_err() {
        return Err("The command must be launchable and contain %ROM%.".to_string());
    }
    if emulator.name.trim().is_empty() {
        return Err("An emulator name is required.".to_string());
    }
    let mut registry = registry(&app);
    match registry
        .emulators
        .iter_mut()
        .find(|existing| existing.id == emulator.id)
    {
        Some(existing) => {
            *existing = RomEmulator {
                builtin: existing.builtin,
                ..emulator
            };
        }
        None => registry.emulators.push(RomEmulator {
            builtin: false,
            ..emulator
        }),
    }
    normalize(&mut registry);
    let state = app.state::<AppState>();
    save(&state.app_data, &registry)?;
    Ok(registry)
}

/// Deletes a user emulator. Built-in presets can be disabled but not removed.
#[tauri::command]
pub fn delete_rom_emulator(app: AppHandle, id: String) -> Result<RomRegistry, String> {
    let mut registry = registry(&app);
    let Some(index) = registry
        .emulators
        .iter()
        .position(|emulator| emulator.id == id)
    else {
        return Err("Emulator not found.".to_string());
    };
    if registry.emulators[index].builtin {
        registry.emulators[index].enabled = false;
    } else {
        registry.emulators.remove(index);
    }
    registry.defaults.retain(|_, emulator| emulator != &id);
    let state = app.state::<AppState>();
    save(&state.app_data, &registry)?;
    Ok(registry)
}

/// Sets or clears the default emulator for a platform.
#[tauri::command]
pub fn set_rom_default(
    app: AppHandle,
    platform: String,
    emulator_id: Option<String>,
) -> Result<RomRegistry, String> {
    if platform_by_id(&platform).is_none() {
        return Err(format!("Unknown platform `{platform}`."));
    }
    let mut registry = registry(&app);
    match emulator_id.filter(|id| !id.trim().is_empty()) {
        Some(id) => {
            registry.defaults.insert(platform, id);
        }
        None => {
            registry.defaults.remove(&platform);
        }
    }
    normalize(&mut registry);
    let state = app.state::<AppState>();
    save(&state.app_data, &registry)?;
    Ok(registry)
}

/// Validates an emulator command for the editor.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandValidation {
    /// Whether the template can be launched.
    pub valid: bool,
    /// Reason when invalid.
    pub error: Option<String>,
    /// Resolved program.
    pub program: Option<String>,
    /// Resolved arguments.
    pub args: Vec<String>,
}

/// Validates a command template without running it.
#[tauri::command]
pub fn validate_rom_command(command: String) -> CommandValidation {
    match validate_command(&command) {
        Ok(command) => CommandValidation {
            valid: true,
            error: None,
            program: Some(command.program),
            args: command.args,
        },
        Err(error) => CommandValidation {
            valid: false,
            error: Some(error),
            program: None,
            args: Vec::new(),
        },
    }
}

/// Launches a ROM.
#[tauri::command]
pub async fn launch_rom_game(
    app: AppHandle,
    game_id: i64,
    platform: String,
    rom_path: String,
    emulator_id: Option<String>,
) -> Result<RomLaunchResult, String> {
    launch_rom(&app, game_id, &platform, &rom_path, emulator_id.as_deref())
}

/// Stops a ROM session (the same process map as installed games).
#[tauri::command]
pub fn stop_rom_game(app: AppHandle, session_id: String) -> Result<(), String> {
    launch::stop(&app, &session_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ludex-roms-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn presets_are_valid_and_round_trip() {
        let dir = temp_dir();
        let dir = dir.as_path();
        let registry = RomRegistry::default();
        for emulator in &registry.emulators {
            assert!(
                validate_command(&emulator.command).is_ok(),
                "{}",
                emulator.command
            );
        }
        save(dir, &registry).expect("save");
        let loaded = load(dir);
        assert_eq!(loaded.emulators.len(), registry.emulators.len());
        assert!(loaded
            .emulators
            .iter()
            .any(|emulator| emulator.id == "dolphin"));
    }

    #[test]
    fn resolution_prefers_game_then_platform_then_support() {
        let mut registry = RomRegistry::default();
        registry
            .defaults
            .insert("snes".to_string(), "snes9x".to_string());
        let resolved = resolve(&registry, "snes", None).expect("default");
        assert_eq!(resolved.id, "snes9x");
        let resolved = resolve(&registry, "snes", Some("retroarch-snes")).expect("override");
        assert_eq!(resolved.id, "retroarch-snes");
        let resolved = resolve(&registry, "gamecube", None).expect("support");
        assert_eq!(resolved.id, "dolphin");
        assert!(resolve(&registry, "gba", Some("missing")).is_some());
        assert!(resolve(&registry, "unknown", None).is_none());
    }

    #[test]
    fn invalid_registries_fall_back_to_presets() {
        let dir = temp_dir();
        std::fs::write(registry_path(&dir), "{not json").expect("write");
        let registry = load(&dir);
        assert!(!registry.emulators.is_empty());
        assert!(registry_path(&dir).with_extension("json.invalid").exists());
    }

    #[test]
    fn shell_syntax_is_rejected() {
        assert!(validate_command("retroarch %ROM%").is_ok());
        assert!(validate_command("retroarch %ROM%; rm -rf /").is_err());
        assert!(validate_command("retroarch").is_err());
        let command = build_launch_command("retroarch %ROM%", Path::new("/roms/It's fine.sfc"))
            .expect("launch");
        assert_eq!(command.args, vec!["/roms/It's fine.sfc"]);
    }

    #[test]
    fn normalize_drops_duplicates_and_invalid_defaults() {
        let mut registry = RomRegistry::default();
        registry
            .defaults
            .insert("snes".to_string(), "missing".to_string());
        registry.emulators.push(RomEmulator {
            id: "dolphin".to_string(),
            name: "Duplicate".to_string(),
            command: "dolphin %ROM%".to_string(),
            platforms: vec![],
            enabled: true,
            builtin: false,
        });
        normalize(&mut registry);
        assert_eq!(
            registry
                .emulators
                .iter()
                .filter(|emulator| emulator.id == "dolphin")
                .count(),
            1
        );
        assert!(registry.defaults.is_empty());
    }
}
