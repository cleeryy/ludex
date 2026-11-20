//! Declarative per-game profiles (#300): export, import, share and diff.
//!
//! A profile is one TOML document describing how a game is meant to run:
//! launch plan, display wrapper, prerequisites, notes and the build stamps it
//! was written with. It is built from the launch options the desktop app
//! already stores (no second settings model) and it never carries a host path:
//!
//! - the Wine prefix is deliberately absent — it is a per-device folder;
//! - the audio device (#289) is absent too: a sink name only exists on this
//!   host, so an import keeps the local per-game device;
//! - the compat tool is written by **name** (`GE-Proton9-20`), not by its
//!   `kind:path` id, and resolves to the installed build on import;
//! - a working directory is only written when it lives inside the install
//!   folder, as a relative path;
//! - an environment value that holds an absolute path (or the home directory)
//!   is left out with a warning, because it cannot mean anything elsewhere.
//!
//! Parsing is forward compatible: keys this build does not know stay in
//! [`GameProfile::extra`] (or in the matching section `extra` map) and are
//! written back, so a profile produced by a newer Ludex survives an
//! import/export round-trip. Unknown sections such as a future `[audio]`
//! therefore survive too.
//!
//! Import is a proposal, never a silent overwrite. [`propose`] diffs the
//! incoming document against the current one and reports warnings (missing
//! Proton build, unknown prerequisite, absent working folder); nothing is
//! written. Only [`apply`] touches the database, and only after the UI asked
//! for a confirmation.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::compat::CompatTool;
use crate::db;
use crate::linux_gaming::{
    validate_display, validate_env, validate_verbs, DisplayMode, DisplayProfile, LaunchMode,
    LaunchOptions,
};
use crate::runtime::plan::PREREQUISITE_IDS;

/// Version of the document this build writes. A higher number is parsed best
/// effort with a warning instead of being rejected.
pub const PROFILE_SCHEMA_VERSION: u32 = 1;

/// Stamp written until the compatibility rules database (#301) ships its own
/// version.
pub const PROFILE_RULES_VERSION: &str = "unversioned";

/// Keys this build understands; everything else is preserved as [`extra`].
///
/// [`extra`]: GameProfile::extra
const KNOWN_KEYS: [&str; 6] = [
    "schema_version",
    "meta",
    "launch",
    "display",
    "prerequisites",
    "notes",
];

/// Stable warning codes the UI translates.
pub const WARNING_MISSING_BUILD: &str = "missing-build";
pub const WARNING_MISSING_WORKING_DIR: &str = "missing-working-dir";
pub const WARNING_LOCAL_PATH_OMITTED: &str = "local-path-omitted";
pub const WARNING_PREFIX_OMITTED: &str = "prefix-omitted";
pub const WARNING_UNKNOWN_PREREQUISITE: &str = "unknown-prerequisite";
pub const WARNING_MISSING_SCHEMA: &str = "missing-schema";
pub const WARNING_UNKNOWN_SCHEMA: &str = "unknown-schema";

/// Document metadata. Versions are strings so a future format can stamp
/// anything without another schema bump.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProfileMeta {
    pub title: String,
    pub app_version: String,
    pub rules_version: String,
    pub exported_at: String,
    /// Keys a newer release added to `[meta]`; kept for the round-trip.
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

/// The launch plan, minus everything that only means something on this device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProfileLaunch {
    pub mode: LaunchMode,
    /// Compat tool *name*, never an id or a path (`GE-Proton9-20`).
    pub tool: String,
    pub env: BTreeMap<String, String>,
    pub winetricks: Vec<String>,
    pub steam_app_id: Option<u32>,
    /// Folder the game starts in, relative to the install folder. Absent means
    /// "leave the local choice alone".
    pub working_dir: Option<String>,
    /// Keys a newer release added to `[launch]`; kept for the round-trip.
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

/// Display wrapper and its settings. `mode = "desktop"` is a no-op.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProfileDisplay {
    pub mode: DisplayMode,
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
    pub filter: String,
    pub adaptive_sync: bool,
    pub hdr: bool,
    /// Keys a newer release added to `[display]`; kept for the round-trip.
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

impl From<&DisplayProfile> for ProfileDisplay {
    fn from(display: &DisplayProfile) -> Self {
        Self {
            mode: display.mode,
            width: display.width,
            height: display.height,
            refresh: display.refresh,
            filter: display.filter.clone(),
            adaptive_sync: display.adaptive_sync,
            hdr: display.hdr,
            extra: BTreeMap::new(),
        }
    }
}

impl From<&ProfileDisplay> for DisplayProfile {
    fn from(display: &ProfileDisplay) -> Self {
        Self {
            mode: display.mode,
            width: display.width,
            height: display.height,
            refresh: display.refresh,
            filter: display.filter.clone(),
            adaptive_sync: display.adaptive_sync,
            hdr: display.hdr,
        }
    }
}

/// One parsed profile document.
#[derive(Debug, Clone, PartialEq)]
pub struct GameProfile {
    pub schema_version: u32,
    pub meta: ProfileMeta,
    pub launch: ProfileLaunch,
    pub display: ProfileDisplay,
    pub prerequisites: Vec<String>,
    pub notes: String,
    /// Keys this build does not know, kept so a newer profile round-trips.
    pub extra: BTreeMap<String, toml::Value>,
}

impl Default for GameProfile {
    fn default() -> Self {
        Self {
            schema_version: PROFILE_SCHEMA_VERSION,
            meta: ProfileMeta::default(),
            launch: ProfileLaunch::default(),
            display: ProfileDisplay::default(),
            prerequisites: Vec::new(),
            notes: String::new(),
            extra: BTreeMap::new(),
        }
    }
}

/// What the UI receives after an export (#300).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileExport {
    pub toml: String,
    pub warnings: Vec<ProfileWarning>,
}

/// One warning or blocking problem about an import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileWarning {
    /// Stable code (`missing-build`, `local-path-omitted`, …).
    pub code: String,
    pub detail: String,
}

impl ProfileWarning {
    fn new(code: &str, detail: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            detail: detail.into(),
        }
    }
}

/// One changed field between the current and the incoming profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileChange {
    /// Dotted path (`launch.mode`, `launch.env.DXVK_HUD`, `display.width`).
    pub field: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// The proposal shown before anything is written.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportProposal {
    pub profile: GameProfile,
    pub diff: Vec<ProfileChange>,
    pub warnings: Vec<ProfileWarning>,
    pub errors: Vec<String>,
}

/// Serializable view of [`ImportProposal`] for the Tauri command.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileProposal {
    pub title: String,
    pub schema_version: u32,
    pub app_version: String,
    pub rules_version: String,
    pub diff: Vec<ProfileChange>,
    pub warnings: Vec<ProfileWarning>,
    pub errors: Vec<String>,
    pub can_apply: bool,
}

impl ImportProposal {
    pub fn view(&self) -> ProfileProposal {
        ProfileProposal {
            title: self.profile.meta.title.clone(),
            schema_version: self.profile.schema_version,
            app_version: self.profile.meta.app_version.clone(),
            rules_version: self.profile.meta.rules_version.clone(),
            diff: self.diff.clone(),
            warnings: self.warnings.clone(),
            errors: self.errors.clone(),
            can_apply: self.errors.is_empty(),
        }
    }
}

/// Everything an export needs. A struct keeps the call sites readable and the
/// stamps explicit (tests never read the real clock or the crate version).
pub struct ExportRequest<'a> {
    pub title: &'a str,
    pub install_root: &'a Path,
    pub options: &'a LaunchOptions,
    pub prerequisites: &'a [String],
    /// Name of the selected compat tool, when the host scan found it.
    pub tool_name: Option<&'a str>,
    /// Home directory to scrub from environment values, when known.
    pub home: Option<&'a str>,
    pub app_version: &'a str,
    pub rules_version: &'a str,
    pub exported_at: &'a str,
}

/// Export a profile document. Returns the TOML and the paths that had to stay
/// behind.
pub fn export(request: ExportRequest<'_>) -> Result<ProfileExport, String> {
    let (profile, warnings) = build_profile(request)?;
    let toml = profile.to_toml()?;
    Ok(ProfileExport { toml, warnings })
}

/// Build the document a share would carry, without serialising it.
pub fn build_profile(
    request: ExportRequest<'_>,
) -> Result<(GameProfile, Vec<ProfileWarning>), String> {
    let mut warnings = Vec::new();
    let mut launch = ProfileLaunch {
        mode: request.options.mode,
        tool: request
            .tool_name
            .map(str::trim)
            .filter(|name| !name.is_empty() && !looks_like_path(name))
            .unwrap_or_default()
            .to_string(),
        env: BTreeMap::new(),
        winetricks: request.options.winetricks_verbs.clone(),
        steam_app_id: request.options.steam_app_id,
        working_dir: None,
        extra: BTreeMap::new(),
    };
    if request
        .tool_name
        .is_some_and(|name| looks_like_path(name.trim()))
    {
        warnings.push(ProfileWarning::new(
            WARNING_LOCAL_PATH_OMITTED,
            "The compatibility tool was identified by a path; the profile leaves the build choice to the importing device.",
        ));
    }

    for (key, value) in &request.options.env {
        if value_has_local_path(value, request.home) {
            warnings.push(ProfileWarning::new(
                WARNING_LOCAL_PATH_OMITTED,
                format!("{key} holds a local path and was left out of the profile."),
            ));
            continue;
        }
        launch.env.insert(key.clone(), value.clone());
    }

    let working_dir = request.options.working_dir.trim();
    if !working_dir.is_empty() {
        let relative = if Path::new(working_dir).is_absolute() {
            Path::new(working_dir)
                .strip_prefix(request.install_root)
                .ok()
                .and_then(relative_path)
        } else {
            relative_path(Path::new(working_dir))
        };
        match relative {
            Some(relative) => launch.working_dir = Some(relative),
            None => warnings.push(ProfileWarning::new(
                WARNING_LOCAL_PATH_OMITTED,
                "The working directory is outside the install folder and was left out of the profile.",
            )),
        }
    }

    if !request.options.prefix_path.trim().is_empty() {
        warnings.push(ProfileWarning::new(
            WARNING_PREFIX_OMITTED,
            "The custom Wine prefix stays on this device; the profile never carries it.",
        ));
    }

    let mut prerequisites: Vec<String> = Vec::new();
    for value in request.prerequisites {
        let value = value.trim().to_ascii_lowercase();
        if value.is_empty() {
            continue;
        }
        if !prerequisites.contains(&value) {
            prerequisites.push(value);
        }
    }

    let profile = GameProfile {
        schema_version: PROFILE_SCHEMA_VERSION,
        meta: ProfileMeta {
            title: request.title.trim().to_string(),
            app_version: request.app_version.to_string(),
            rules_version: request.rules_version.to_string(),
            exported_at: request.exported_at.to_string(),
            extra: BTreeMap::new(),
        },
        launch,
        display: ProfileDisplay::from(&request.options.display),
        prerequisites,
        notes: String::new(),
        extra: BTreeMap::new(),
    };
    Ok((profile, warnings))
}

/// Parse one TOML document. Unknown keys are kept, never dropped.
pub fn parse(text: &str) -> Result<(GameProfile, Vec<ProfileWarning>), String> {
    let value: toml::Value = toml::from_str(text)
        .map_err(|error| format!("This is not a valid TOML profile: {error}"))?;
    let mut table = match value {
        toml::Value::Table(table) => table,
        _ => return Err("A profile must be a TOML table.".to_string()),
    };

    let mut warnings = Vec::new();
    let schema_version = match table.remove("schema_version") {
        Some(toml::Value::Integer(version)) => u32::try_from(version).unwrap_or(0),
        Some(other) => {
            return Err(format!(
                "schema_version must be an integer, not {}.",
                other.type_str()
            ))
        }
        None => 0,
    };
    let schema_version = if schema_version == 0 {
        warnings.push(ProfileWarning::new(
            WARNING_MISSING_SCHEMA,
            "The profile has no schema version; it was read as version 1.",
        ));
        PROFILE_SCHEMA_VERSION
    } else {
        if schema_version > PROFILE_SCHEMA_VERSION {
            warnings.push(ProfileWarning::new(
                WARNING_UNKNOWN_SCHEMA,
                format!(
                    "The profile was written for schema {schema_version}; this build understands {PROFILE_SCHEMA_VERSION} and reads the known fields only.",
                ),
            ));
        }
        schema_version
    };

    let meta = take_section::<ProfileMeta>(&mut table, "meta")?;
    let launch = take_section::<ProfileLaunch>(&mut table, "launch")?;
    let display = take_section::<ProfileDisplay>(&mut table, "display")?;

    let prerequisites = match table.remove("prerequisites") {
        Some(value) => value
            .try_into::<Vec<String>>()
            .map_err(|error| format!("prerequisites must be a list of strings: {error}"))?,
        None => Vec::new(),
    };
    let notes = match table.remove("notes") {
        Some(value) => value
            .try_into::<String>()
            .map_err(|error| format!("notes must be a string: {error}"))?,
        None => String::new(),
    };

    // Everything left is a key this build does not know. Keep it so a newer
    // profile survives an export from an older build.
    let extra: BTreeMap<String, toml::Value> = table
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    let profile = GameProfile {
        schema_version,
        meta,
        launch,
        display,
        prerequisites,
        notes,
        extra,
    };
    Ok((profile, warnings))
}

fn take_section<T>(table: &mut toml::Table, key: &str) -> Result<T, String>
where
    T: for<'de> Deserialize<'de> + Default,
{
    match table.remove(key) {
        Some(value) => value
            .try_into::<T>()
            .map_err(|error| format!("The [{key}] table is invalid: {error}")),
        None => Ok(T::default()),
    }
}

impl GameProfile {
    /// Serialise the document. Unknown keys are written back at the end, so a
    /// newer profile is not silently downgraded on its way out.
    pub fn to_toml(&self) -> Result<String, String> {
        let mut root = toml::Table::new();
        root.insert(
            "schema_version".to_string(),
            toml::Value::Integer(i64::from(self.schema_version)),
        );

        let mut meta = toml::Table::new();
        insert_string(&mut meta, "title", &self.meta.title);
        insert_string(&mut meta, "app_version", &self.meta.app_version);
        insert_string(&mut meta, "rules_version", &self.meta.rules_version);
        insert_string(&mut meta, "exported_at", &self.meta.exported_at);
        merge_extra(&mut meta, &self.meta.extra);
        root.insert("meta".to_string(), toml::Value::Table(meta));

        let mut launch = toml::Table::new();
        insert_string(&mut launch, "mode", self.launch.mode.as_str());
        if !self.launch.tool.is_empty() {
            insert_string(&mut launch, "tool", &self.launch.tool);
        }
        if let Some(app_id) = self.launch.steam_app_id {
            launch.insert(
                "steam_app_id".to_string(),
                toml::Value::Integer(i64::from(app_id)),
            );
        }
        if let Some(working_dir) = &self.launch.working_dir {
            insert_string(&mut launch, "working_dir", working_dir);
        }
        let winetricks = self
            .launch
            .winetricks
            .iter()
            .map(|verb| toml::Value::String(verb.clone()))
            .collect();
        launch.insert("winetricks".to_string(), toml::Value::Array(winetricks));
        if !self.launch.env.is_empty() {
            let env = self
                .launch
                .env
                .iter()
                .map(|(key, value)| (key.clone(), toml::Value::String(value.clone())))
                .collect();
            launch.insert("env".to_string(), toml::Value::Table(env));
        }
        merge_extra(&mut launch, &self.launch.extra);
        root.insert("launch".to_string(), toml::Value::Table(launch));

        let mut display = toml::Table::new();
        insert_string(&mut display, "mode", display_mode_str(self.display.mode));
        display.insert(
            "width".to_string(),
            toml::Value::Integer(i64::from(self.display.width)),
        );
        display.insert(
            "height".to_string(),
            toml::Value::Integer(i64::from(self.display.height)),
        );
        display.insert(
            "refresh".to_string(),
            toml::Value::Integer(i64::from(self.display.refresh)),
        );
        insert_string(&mut display, "filter", &self.display.filter);
        display.insert(
            "adaptive_sync".to_string(),
            toml::Value::Boolean(self.display.adaptive_sync),
        );
        display.insert("hdr".to_string(), toml::Value::Boolean(self.display.hdr));
        merge_extra(&mut display, &self.display.extra);
        root.insert("display".to_string(), toml::Value::Table(display));

        let prerequisites = self
            .prerequisites
            .iter()
            .map(|id| toml::Value::String(id.clone()))
            .collect();
        root.insert(
            "prerequisites".to_string(),
            toml::Value::Array(prerequisites),
        );
        insert_string(&mut root, "notes", &self.notes);

        for (key, value) in &self.extra {
            if !KNOWN_KEYS.contains(&key.as_str()) {
                root.insert(key.clone(), value.clone());
            }
        }

        toml::to_string_pretty(&root)
            .map_err(|error| format!("Could not serialise the profile: {error}"))
    }
}

/// Merge keys this build does not know, never overwriting a known one.
fn merge_extra(table: &mut toml::Table, extra: &BTreeMap<String, toml::Value>) {
    for (key, value) in extra {
        table.entry(key.clone()).or_insert_with(|| value.clone());
    }
}

fn insert_string(table: &mut toml::Table, key: &str, value: &str) {
    table.insert(key.to_string(), toml::Value::String(value.to_string()));
}

fn display_mode_str(mode: DisplayMode) -> &'static str {
    match mode {
        DisplayMode::Desktop => "desktop",
        DisplayMode::Gamescope => "gamescope",
    }
}

/// Validate and normalise an incoming document, then diff it against the
/// current one. Pure: nothing here writes to the database.
pub fn propose(
    current: &GameProfile,
    mut incoming: GameProfile,
    available_tools: &[String],
    install_root: Option<&Path>,
) -> ImportProposal {
    let mut warnings = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    incoming.meta.title = incoming.meta.title.trim().to_string();
    incoming.launch.tool = incoming.launch.tool.trim().to_string();
    incoming.launch.env = incoming
        .launch
        .env
        .iter()
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .filter(|(key, _)| !key.is_empty())
        .collect();
    incoming.launch.winetricks = incoming
        .launch
        .winetricks
        .iter()
        .map(|verb| verb.trim().to_ascii_lowercase())
        .filter(|verb| !verb.is_empty())
        .collect();
    incoming.display.filter = incoming.display.filter.trim().to_ascii_lowercase();
    let mut prerequisites: Vec<String> = Vec::new();
    for value in &incoming.prerequisites {
        let value = value.trim().to_ascii_lowercase();
        if value.is_empty() || prerequisites.contains(&value) {
            continue;
        }
        prerequisites.push(value);
    }
    incoming.prerequisites = prerequisites;

    match validate_env(&incoming.launch.env) {
        Ok(env) => incoming.launch.env = env,
        Err(error) => errors.push(error),
    }
    match validate_verbs(&incoming.launch.winetricks) {
        Ok(verbs) => incoming.launch.winetricks = verbs,
        Err(error) => errors.push(error),
    }

    let mut display = DisplayProfile::from(&incoming.display);
    match validate_display(&mut display) {
        Ok(()) => incoming.display = ProfileDisplay::from(&display),
        Err(error) => errors.push(error),
    }

    if incoming.launch.steam_app_id == Some(0) {
        incoming.launch.steam_app_id = None;
    }

    if let Some(working_dir) = incoming.launch.working_dir.take() {
        let working_dir = working_dir.trim();
        if !working_dir.is_empty() {
            match relative_path(Path::new(working_dir)) {
                Some(relative) => {
                    let exists = install_root
                        .map(|root| root.join(&relative).is_dir())
                        .unwrap_or(true);
                    if exists {
                        incoming.launch.working_dir = Some(relative);
                    } else {
                        warnings.push(ProfileWarning::new(
                            WARNING_MISSING_WORKING_DIR,
                            format!(
                                "The working directory {relative:?} is not in this install folder; the local choice is kept."
                            ),
                        ));
                    }
                }
                None => errors.push(
                    "The working directory must be a relative path inside the install folder."
                        .to_string(),
                ),
            }
        }
    }

    if !incoming.launch.tool.is_empty() {
        let installed = available_tools
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&incoming.launch.tool));
        if !installed {
            warnings.push(ProfileWarning::new(
                WARNING_MISSING_BUILD,
                format!(
                    "{} is not installed on this device; Ludex will use the best available build.",
                    incoming.launch.tool
                ),
            ));
        }
    }

    for prerequisite in &incoming.prerequisites {
        if !known_prerequisites().contains(&prerequisite.as_str()) {
            warnings.push(ProfileWarning::new(
                WARNING_UNKNOWN_PREREQUISITE,
                format!("{prerequisite} is not a prerequisite this build knows; it is ignored."),
            ));
        }
    }

    let diff = diff(current, &incoming);
    ImportProposal {
        profile: incoming,
        diff,
        warnings,
        errors,
    }
}

/// Pure diff between two profiles. Only changed fields are returned, in a
/// stable order, so "what changed since it worked" is a short list.
pub fn diff(current: &GameProfile, incoming: &GameProfile) -> Vec<ProfileChange> {
    let mut changes = Vec::new();

    push_change(
        &mut changes,
        "launch.mode",
        Some(current.launch.mode.as_str().to_string()),
        Some(incoming.launch.mode.as_str().to_string()),
    );
    push_change(
        &mut changes,
        "launch.tool",
        optional(&current.launch.tool),
        optional(&incoming.launch.tool),
    );
    push_change(
        &mut changes,
        "launch.steam_app_id",
        current.launch.steam_app_id.map(|id| id.to_string()),
        incoming.launch.steam_app_id.map(|id| id.to_string()),
    );
    push_change(
        &mut changes,
        "launch.working_dir",
        current.launch.working_dir.clone(),
        incoming.launch.working_dir.clone(),
    );
    let env_keys: BTreeSet<&String> = current
        .launch
        .env
        .keys()
        .chain(incoming.launch.env.keys())
        .collect();
    for key in env_keys {
        push_change(
            &mut changes,
            &format!("launch.env.{key}"),
            current.launch.env.get(key).cloned(),
            incoming.launch.env.get(key).cloned(),
        );
    }

    push_change(
        &mut changes,
        "launch.winetricks",
        optional(&current.launch.winetricks.join(" ")),
        optional(&incoming.launch.winetricks.join(" ")),
    );

    push_change(
        &mut changes,
        "display.mode",
        Some(display_mode_str(current.display.mode).to_string()),
        Some(display_mode_str(incoming.display.mode).to_string()),
    );
    for (field, before, after) in [
        (
            "display.width",
            current.display.width.to_string(),
            incoming.display.width.to_string(),
        ),
        (
            "display.height",
            current.display.height.to_string(),
            incoming.display.height.to_string(),
        ),
        (
            "display.refresh",
            current.display.refresh.to_string(),
            incoming.display.refresh.to_string(),
        ),
        (
            "display.filter",
            current.display.filter.clone(),
            incoming.display.filter.clone(),
        ),
        (
            "display.adaptive_sync",
            current.display.adaptive_sync.to_string(),
            incoming.display.adaptive_sync.to_string(),
        ),
        (
            "display.hdr",
            current.display.hdr.to_string(),
            incoming.display.hdr.to_string(),
        ),
    ] {
        push_change(&mut changes, field, Some(before), Some(after));
    }

    push_change(
        &mut changes,
        "prerequisites",
        optional(&current.prerequisites.join(" ")),
        optional(&incoming.prerequisites.join(" ")),
    );

    // Unknown keys are part of the document: show when a profile brings one so
    // the proposal is not silently accepting a future field.
    let before = future_entries(current);
    let after = future_entries(incoming);
    let future_keys: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    for key in future_keys {
        push_change(
            &mut changes,
            &format!("future.{key}"),
            before.get(key).cloned(),
            after.get(key).cloned(),
        );
    }

    changes
}

fn push_change(
    changes: &mut Vec<ProfileChange>,
    field: &str,
    before: Option<String>,
    after: Option<String>,
) {
    if before == after {
        return;
    }
    changes.push(ProfileChange {
        field: field.to_string(),
        before,
        after,
    });
}

fn optional(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Every unknown key, qualified by its section: `audio`, `launch.mystery`.
fn future_entries(profile: &GameProfile) -> BTreeMap<String, String> {
    let mut entries = BTreeMap::new();
    for (key, value) in &profile.extra {
        entries.insert(key.clone(), value.to_string());
    }
    for (section, extra) in [
        ("meta", &profile.meta.extra),
        ("launch", &profile.launch.extra),
        ("display", &profile.display.extra),
    ] {
        for (key, value) in extra {
            entries.insert(format!("{section}.{key}"), value.to_string());
        }
    }
    entries
}

/// Apply a reviewed profile. This is the only function that writes, and the
/// command layer calls it after the user confirmed the proposal.
///
/// The Wine prefix is never touched. A working directory this device does not
/// have keeps the local choice; a tool name that is not installed resolves to
/// "best available" instead of failing the import.
pub fn apply(
    connection: &Connection,
    game_id: i64,
    install_root: &Path,
    incoming: &GameProfile,
    tools: &[CompatTool],
    updated_at: &str,
) -> Result<LaunchOptions, String> {
    let current = db::get_launch_options(connection, game_id)?;

    let tool_id = if incoming.launch.tool.is_empty() {
        String::new()
    } else {
        tools
            .iter()
            .find(|tool| tool.name.eq_ignore_ascii_case(&incoming.launch.tool))
            .map(|tool| tool.id.clone())
            .unwrap_or_default()
    };

    let working_dir = match incoming.launch.working_dir.as_deref() {
        Some(relative) => {
            let relative = relative_path(Path::new(relative)).ok_or_else(|| {
                "The working directory must be a relative path inside the install folder."
                    .to_string()
            })?;
            let full = crate::paths::normalize_native_path(&install_root.join(relative));
            if full.is_dir() {
                full.to_string_lossy().to_string()
            } else {
                current.working_dir.clone()
            }
        }
        // Absent means "the importing device decides": keep what is stored.
        None => current.working_dir.clone(),
    };

    let mut options = LaunchOptions {
        mode: incoming.launch.mode,
        tool_id,
        prefix_path: current.prefix_path.clone(),
        env: incoming.launch.env.clone(),
        winetricks_verbs: incoming.launch.winetricks.clone(),
        steam_app_id: incoming.launch.steam_app_id,
        working_dir,
        display: DisplayProfile::from(&incoming.display),
        // Per-device choices the profile never carries (#289/#297): the audio
        // device name only means something on this host, and GameMode is an
        // in-memory decision of the launch plan.
        audio_sink: current.audio_sink.clone(),
        gamemode: false,
    };
    options.normalize();
    options.env = validate_env(&options.env)?;
    options.winetricks_verbs = validate_verbs(&options.winetricks_verbs)?;
    validate_display(&mut options.display)?;
    if options.steam_app_id == Some(0) {
        options.steam_app_id = None;
    }

    let mut prefs = db::get_runtime_prefs(connection, game_id)?;
    prefs.prerequisites = incoming
        .prerequisites
        .iter()
        .filter(|id| known_prerequisites().contains(&id.as_str()))
        .cloned()
        .collect();
    prefs.updated_at = updated_at.to_string();

    db::save_launch_options(connection, game_id, &options, updated_at)?;
    db::save_runtime_prefs(connection, &prefs)?;
    Ok(options)
}

/// Current profile for a game, used as the "before" side of a proposal.
pub fn current_profile(request: ExportRequest<'_>) -> Result<GameProfile, String> {
    build_profile(request).map(|(profile, _)| profile)
}

fn relative_path(path: &Path) -> Option<String> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if out.as_os_str().is_empty() {
        return None;
    }
    Some(out.to_string_lossy().replace('\\', "/"))
}

/// True when a value cannot be shared: an absolute path, a home-relative path
/// or the local home directory in any form.
fn value_has_local_path(value: &str, home: Option<&str>) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    if let Some(home) = home.map(str::trim).filter(|home| !home.is_empty()) {
        if value.contains(home) {
            return true;
        }
    }
    looks_like_path(value)
}

fn looks_like_path(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    if value.starts_with('/')
        || value.starts_with("~/")
        || value.starts_with("\\\\")
        || value.starts_with("$HOME")
        || value.starts_with("${HOME}")
    {
        return true;
    }
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

/// Prerequisites a profile may declare, without duplicating the planner list.
pub fn known_prerequisites() -> &'static [&'static str] {
    PREREQUISITE_IDS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "ludex-profile-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ))
    }

    fn options() -> LaunchOptions {
        LaunchOptions {
            mode: LaunchMode::Auto,
            tool_id: String::new(),
            prefix_path: String::new(),
            env: BTreeMap::new(),
            winetricks_verbs: Vec::new(),
            steam_app_id: None,
            working_dir: String::new(),
            display: DisplayProfile::default(),
            audio_sink: String::new(),
            gamemode: false,
        }
    }

    fn request<'a>(
        root: &'a Path,
        options: &'a LaunchOptions,
        prerequisites: &'a [String],
        tool: Option<&'a str>,
    ) -> ExportRequest<'a> {
        ExportRequest {
            title: "Hades",
            install_root: root,
            options,
            prerequisites,
            tool_name: tool,
            home: Some("/home/alice"),
            app_version: "0.4.25",
            rules_version: PROFILE_RULES_VERSION,
            exported_at: "2026-11-19T10:00:00Z",
        }
    }

    fn parsed(toml: &str) -> GameProfile {
        parse(toml).expect("profile parses").0
    }

    #[test]
    fn a_profile_round_trips_through_toml() {
        let root = install_root();
        let mut options = options();
        options.mode = LaunchMode::Umu;
        options.env.insert("DXVK_HUD".into(), "fps".into());
        options.winetricks_verbs = vec!["vcrun2022".into()];
        options.steam_app_id = Some(1145360);
        options.working_dir = root.join("bin").to_string_lossy().to_string();
        let exported =
            export(request(&root, &options, &[], Some("GE-Proton9-20"))).expect("export succeeds");
        let before = parsed(&exported.toml);
        assert_eq!(before.launch.mode, LaunchMode::Umu);
        assert_eq!(before.launch.tool, "GE-Proton9-20");
        assert_eq!(before.launch.working_dir.as_deref(), Some("bin"));
        assert_eq!(before.meta.title, "Hades");
        assert_eq!(before.meta.app_version, "0.4.25");
        assert_eq!(before.meta.rules_version, PROFILE_RULES_VERSION);

        let again = parsed(&before.to_toml().expect("re-serialises"));
        assert_eq!(before, again);
    }

    #[test]
    fn unknown_keys_survive_a_round_trip() {
        let source = r#"
schema_version = 1
future_flag = true

[meta]
title = "Hades"
app_version = "0.9.0"
rules_version = "rules-7"
future_stamp = "draft"

[launch]
mystery = 3

[audio]
device = "hdmi"
latency_ms = 32
"#;
        let (profile, warnings) = parse(source).expect("parses a future profile");
        assert!(warnings.is_empty());
        assert!(profile.extra.contains_key("future_flag"));
        assert!(profile.extra.contains_key("audio"));
        assert!(profile.meta.extra.contains_key("future_stamp"));
        assert!(profile.launch.extra.contains_key("mystery"));

        let text = profile.to_toml().expect("serialises the unknown keys");
        let reparsed = parsed(&text);
        assert_eq!(profile, reparsed);
        assert!(reparsed.launch.extra.contains_key("mystery"));
        assert_eq!(
            reparsed.extra.get("audio"),
            Some(&toml::Value::Table(toml::Table::from_iter([
                ("device".to_string(), toml::Value::String("hdmi".into())),
                ("latency_ms".to_string(), toml::Value::Integer(32)),
            ])))
        );
    }

    #[test]
    fn a_missing_proton_build_degrades_to_a_warning() {
        let current = GameProfile::default();
        let mut incoming = GameProfile::default();
        incoming.launch.mode = LaunchMode::Proton;
        incoming.launch.tool = "GE-Proton9-20".into();

        let proposal = propose(
            &current,
            incoming,
            &["Proton - Experimental".to_string()],
            None,
        );
        assert!(proposal.errors.is_empty(), "{:?}", proposal.errors);
        assert_eq!(proposal.warnings.len(), 1);
        assert_eq!(proposal.warnings[0].code, WARNING_MISSING_BUILD);
        assert!(proposal.view().can_apply);
    }

    #[test]
    fn a_known_build_is_not_flagged() {
        let mut incoming = GameProfile::default();
        incoming.launch.tool = "GE-Proton9-20".into();
        let proposal = propose(
            &GameProfile::default(),
            incoming,
            &["ge-proton9-20".to_string()],
            None,
        );
        assert!(proposal.warnings.is_empty(), "{:?}", proposal.warnings);
    }

    #[test]
    fn an_import_is_a_proposal_until_it_is_applied() {
        let connection = db::open_in_memory_for_tests();
        let root = install_root();
        let mut current = options();
        current.mode = LaunchMode::Native;
        db::save_launch_options(&connection, 7, &current, "2026-11-19T09:00:00Z").unwrap();
        let current_profile = GameProfile {
            launch: ProfileLaunch {
                mode: LaunchMode::Native,
                ..ProfileLaunch::default()
            },
            ..GameProfile::default()
        };

        let mut incoming = GameProfile::default();
        incoming.launch.mode = LaunchMode::Umu;
        incoming.launch.tool = "GE-Proton9-20".into();
        incoming.launch.env.insert("DXVK_HUD".into(), "fps".into());
        incoming.display.mode = DisplayMode::Gamescope;
        incoming.display.width = 1920;
        incoming.display.height = 1080;

        let proposal = propose(
            &current_profile,
            incoming.clone(),
            &["GE-Proton9-20".to_string()],
            Some(&root),
        );
        assert!(proposal.errors.is_empty(), "{:?}", proposal.errors);
        assert!(!proposal.diff.is_empty());

        // Nothing was written by the proposal.
        let stored = db::get_launch_options(&connection, 7).unwrap();
        assert_eq!(stored.mode, LaunchMode::Native);
        assert!(stored.env.is_empty());

        let tools = vec![CompatTool {
            id: "proton:/home/alice/tools/GE-Proton9-20".into(),
            name: "GE-Proton9-20".into(),
            kind: crate::compat::ToolKind::Proton,
            version: None,
            path: "/home/alice/tools/GE-Proton9-20/proton".into(),
            proton_path: None,
            source: "path".into(),
            steam_root: None,
        }];
        let applied = apply(
            &connection,
            7,
            &root,
            &proposal.profile,
            &tools,
            "2026-11-19T10:00:00Z",
        )
        .expect("apply writes the reviewed profile");
        assert_eq!(applied.mode, LaunchMode::Umu);
        assert_eq!(applied.tool_id, tools[0].id);
        assert_eq!(
            applied.display.mode,
            crate::linux_gaming::DisplayMode::Gamescope
        );

        let stored = db::get_launch_options(&connection, 7).unwrap();
        assert_eq!(stored.mode, LaunchMode::Umu);
        assert_eq!(stored.env.get("DXVK_HUD").map(String::as_str), Some("fps"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn apply_keeps_the_local_wine_prefix() {
        let connection = db::open_in_memory_for_tests();
        let root = install_root();
        let mut current = options();
        current.prefix_path = "/home/alice/prefixes/hades".into();
        db::save_launch_options(&connection, 3, &current, "t0").unwrap();

        let mut incoming = GameProfile::default();
        incoming.launch.mode = LaunchMode::Umu;
        let applied = apply(
            &connection,
            3,
            &root,
            &incoming,
            &[],
            "2026-11-19T10:00:00Z",
        )
        .unwrap();
        assert_eq!(applied.prefix_path, "/home/alice/prefixes/hades");
    }

    #[test]
    fn diff_reports_only_the_changed_fields() {
        let mut current = GameProfile::default();
        current.launch.env.insert("DXVK_HUD".into(), "fps".into());
        current.launch.winetricks = vec!["vcrun2022".into()];
        current.display.width = 1280;

        let mut incoming = current.clone();
        incoming.launch.env.insert("DXVK_HUD".into(), "full".into());
        incoming.launch.env.insert("MANGOHUD".into(), "1".into());
        incoming.launch.winetricks.clear();
        incoming.display.width = 1920;
        incoming.display.hdr = true;

        let changes = diff(&current, &incoming);
        let fields: Vec<&str> = changes.iter().map(|change| change.field.as_str()).collect();
        assert_eq!(
            fields,
            vec![
                "launch.env.DXVK_HUD",
                "launch.env.MANGOHUD",
                "launch.winetricks",
                "display.width",
                "display.hdr",
            ]
        );
        assert_eq!(changes[0].before.as_deref(), Some("fps"));
        assert_eq!(changes[0].after.as_deref(), Some("full"));
        assert_eq!(changes[1].before, None);
        assert_eq!(changes[2].after, None);

        assert!(diff(&incoming, &incoming).is_empty());
    }

    #[test]
    fn the_exported_profile_has_no_absolute_paths_or_usernames() {
        let root = PathBuf::from("/home/alice/Games/Hades");
        let mut options = options();
        options.prefix_path = "/home/alice/Games/Hades/.prefix".into();
        options.working_dir = "/home/alice/Games/Hades/bin".into();
        options.env.insert(
            "DXVK_CONFIG_FILE".into(),
            "/home/alice/.config/dxvk.conf".into(),
        );
        options.env.insert("DXVK_HUD".into(), "fps".into());

        let exported =
            export(request(&root, &options, &[], Some("GE-Proton9-20"))).expect("export succeeds");
        assert!(exported.toml.contains("DXVK_HUD"));
        assert!(exported.toml.contains("working_dir = \"bin\""));
        assert!(!exported.toml.contains("/home"));
        assert!(!exported.toml.contains("alice"));
        assert!(!exported.toml.contains("DXVK_CONFIG_FILE"));
        assert!(!exported.toml.contains("prefix"));
        let codes: Vec<&str> = exported
            .warnings
            .iter()
            .map(|warning| warning.code.as_str())
            .collect();
        assert!(codes.contains(&WARNING_LOCAL_PATH_OMITTED));
        assert!(codes.contains(&WARNING_PREFIX_OMITTED));
    }

    #[test]
    fn a_working_directory_outside_the_install_folder_is_left_out() {
        let root = PathBuf::from("/home/alice/Games/Hades");
        let mut options = options();
        options.working_dir = "/home/alice/Documents".into();
        let exported = export(request(&root, &options, &[], None)).expect("export succeeds");
        assert!(!exported.toml.contains("working_dir"));
        assert!(exported
            .warnings
            .iter()
            .any(|warning| warning.code == WARNING_LOCAL_PATH_OMITTED));
    }

    #[test]
    fn a_missing_working_folder_keeps_the_local_choice() {
        let connection = db::open_in_memory_for_tests();
        let root = install_root();
        std::fs::create_dir_all(&root).unwrap();
        let mut current = options();
        current.working_dir = root.to_string_lossy().to_string();
        db::save_launch_options(&connection, 9, &current, "t0").unwrap();

        let mut incoming = GameProfile::default();
        incoming.launch.working_dir = Some("does-not-exist".into());
        let proposal = propose(&GameProfile::default(), incoming.clone(), &[], Some(&root));
        assert_eq!(proposal.warnings[0].code, WARNING_MISSING_WORKING_DIR);

        let applied = apply(
            &connection,
            9,
            &root,
            &proposal.profile,
            &[],
            "2026-11-19T10:00:00Z",
        )
        .unwrap();
        assert_eq!(applied.working_dir, root.to_string_lossy().to_string());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_newer_schema_is_read_with_a_warning() {
        let source = r#"
schema_version = 9

[meta]
title = "Hades"
"#;
        let (profile, warnings) = parse(source).expect("best-effort parse");
        assert_eq!(profile.schema_version, 9);
        assert_eq!(warnings[0].code, WARNING_UNKNOWN_SCHEMA);
        let text = profile.to_toml().expect("re-serialises");
        assert!(text.contains("schema_version = 9"));
    }

    #[test]
    fn a_profile_without_a_schema_is_read_as_version_one() {
        let (profile, warnings) = parse("[meta]\ntitle = \"Hades\"\n").expect("parses");
        assert_eq!(profile.schema_version, PROFILE_SCHEMA_VERSION);
        assert_eq!(warnings[0].code, WARNING_MISSING_SCHEMA);
        assert_eq!(profile.meta.title, "Hades");
    }

    #[test]
    fn an_unknown_prerequisite_is_a_warning_not_a_failure() {
        let incoming = GameProfile {
            prerequisites: vec!["dotnet".into(), "steamworks".into()],
            ..GameProfile::default()
        };
        let proposal = propose(&GameProfile::default(), incoming, &[], None);
        assert!(proposal.errors.is_empty());
        assert!(proposal
            .warnings
            .iter()
            .any(|warning| warning.code == WARNING_UNKNOWN_PREREQUISITE));
    }

    #[test]
    fn an_invalid_display_profile_blocks_the_import() {
        let mut incoming = GameProfile::default();
        incoming.display.mode = DisplayMode::Gamescope;
        incoming.display.width = 10;
        incoming.display.height = 10;
        let proposal = propose(&GameProfile::default(), incoming, &[], None);
        assert!(!proposal.view().can_apply);
        assert!(proposal.errors[0].contains("width"));
    }

    #[test]
    fn the_document_contains_the_build_stamps() {
        let root = install_root();
        let exported = export(request(&root, &options(), &[], None)).expect("export succeeds");
        assert!(exported.toml.contains("app_version = \"0.4.25\""));
        assert!(exported
            .toml
            .contains(&format!("rules_version = \"{PROFILE_RULES_VERSION}\"")));
    }

    #[test]
    fn relative_paths_reject_traversal_and_absolute_values() {
        assert_eq!(
            relative_path(Path::new("bin/sub")),
            Some("bin/sub".to_string())
        );
        assert_eq!(relative_path(Path::new("./bin")), Some("bin".to_string()));
        assert_eq!(relative_path(Path::new("../bin")), None);
        assert_eq!(relative_path(Path::new("/bin")), None);
        assert_eq!(relative_path(Path::new("")), None);
    }

    #[test]
    fn prerequisite_ids_stay_in_sync_with_the_planner() {
        assert_eq!(
            known_prerequisites(),
            crate::runtime::plan::PREREQUISITE_IDS
        );
    }
}
